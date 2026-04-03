use anyhow::{bail, Context, Result};
use ort::value::Value;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use crate::registry::supertonic::VOICES;

const MAX_TEXT_LEN: usize = 300;

// ── Config structs (from tts.json) ──

#[derive(Deserialize)]
struct TtsConfig {
    ae: AEConfig,
    ttl: TTLConfig,
}

#[derive(Deserialize)]
struct AEConfig {
    sample_rate: i32,
    base_chunk_size: i32,
}

#[derive(Deserialize)]
struct TTLConfig {
    latent_dim: i32,
    chunk_compress_factor: i32,
}

// ── Voice style structs ──

#[derive(Deserialize)]
struct VoiceStyleData {
    style_ttl: StyleComponent,
    style_dp: StyleComponent,
}

#[derive(Deserialize)]
struct StyleComponent {
    data: Vec<Vec<Vec<f32>>>,
    dims: Vec<usize>,
}

struct Style {
    /// Flat f32 data + shape [batch, dim1, dim2]
    ttl_data: Vec<f32>,
    ttl_shape: [usize; 3],
    dp_data: Vec<f32>,
    dp_shape: [usize; 3],
}

// ── Engine ──

pub struct SupertonicEngine {
    dp_session: ort::session::Session,
    text_enc_session: ort::session::Session,
    vector_est_session: ort::session::Session,
    vocoder_session: ort::session::Session,
    indexer: Vec<i64>,
    style: Style,
    voice_id: String,
    model_id: String,
    model_dir: std::path::PathBuf,
    sample_rate: i32,
    base_chunk_size: i32,
    latent_dim: i32,
    chunk_compress_factor: i32,
    speed: f32,
    total_step: usize,
}

impl SupertonicEngine {
    pub fn load(
        model_dir: &Path,
        model_id: &str,
        voice_id: &str,
        speed: f32,
        total_step: u32,
    ) -> Result<Self> {
        // Load config
        let cfg: TtsConfig = {
            let path = model_dir.join("tts.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };

        // Load unicode indexer
        let indexer: Vec<i64> = {
            let path = model_dir.join("unicode_indexer.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };

        // Load ONNX sessions
        let load_session = |name: &str| -> Result<ort::session::Session> {
            let path = model_dir.join(name);
            if !path.exists() {
                bail!("Supertonic model file not found: {}", path.display());
            }
            ort::session::Session::builder()
                .with_context(|| format!("Failed to create session builder for {name}"))?
                .commit_from_file(&path)
                .with_context(|| format!("Failed to load {name}"))
        };

        let dp_session = load_session("duration_predictor.onnx")?;
        let text_enc_session = load_session("text_encoder.onnx")?;
        let vector_est_session = load_session("vector_estimator.onnx")?;
        let vocoder_session = load_session("vocoder.onnx")?;

        // Load voice style
        let style = load_voice_style(model_dir, voice_id)?;

        Ok(Self {
            dp_session,
            text_enc_session,
            vector_est_session,
            vocoder_session,
            indexer,
            style,
            voice_id: voice_id.to_string(),
            model_id: model_id.to_string(),
            model_dir: model_dir.to_path_buf(),
            sample_rate: cfg.ae.sample_rate,
            base_chunk_size: cfg.ae.base_chunk_size,
            latent_dim: cfg.ttl.latent_dim,
            chunk_compress_factor: cfg.ttl.chunk_compress_factor,
            speed,
            total_step: total_step as usize,
        })
    }

    fn preprocess_text(&self, text: &str, lang: &str) -> String {
        let mut s: String = text.nfkd().collect();

        // Remove emojis (keep basic ASCII + extended Latin + CJK + Korean)
        s = s
            .chars()
            .filter(|c| {
                let cp = *c as u32;
                cp < 0x1F600
                    || (0x2E80..=0x9FFF).contains(&cp)  // CJK
                    || (0xAC00..=0xD7AF).contains(&cp)   // Korean
                    || (0x00C0..=0x024F).contains(&cp)    // Extended Latin
            })
            .collect();

        // Normalize dashes and quotes
        s = s.replace('\u{2014}', "-").replace('\u{2013}', "-");
        s = s.replace('\u{201c}', "\"").replace('\u{201d}', "\"");
        s = s.replace('\u{2018}', "'").replace('\u{2019}', "'");

        // Ensure text ends with a period
        let trimmed = s.trim();
        if !trimmed.ends_with('.') && !trimmed.ends_with('!') && !trimmed.ends_with('?') {
            s = format!("{trimmed}.");
        } else {
            s = trimmed.to_string();
        }

        // Wrap in language tags
        format!("<{lang}>{s}</{lang}>")
    }

    fn tokenize(&self, text: &str) -> Vec<i64> {
        text.chars()
            .map(|c| {
                let cp = c as usize;
                if cp < self.indexer.len() {
                    self.indexer[cp]
                } else {
                    -1
                }
            })
            .collect()
    }

    fn infer(&mut self, text: &str, lang: &str) -> Result<Vec<f32>> {
        let processed = self.preprocess_text(text, lang);
        let text_ids_raw = self.tokenize(&processed);
        let text_len = text_ids_raw.len();

        // Build text_mask: [1, 1, text_len] — all 1s
        let text_mask: Vec<f32> = vec![1.0; text_len];

        // 1. Duration prediction
        let dp_text_ids =
            Value::from_array(([1usize, text_len], text_ids_raw.clone()))?;
        let dp_style =
            Value::from_array((self.style.dp_shape, self.style.dp_data.clone()))?;
        let dp_mask =
            Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;

        let dp_outputs = self
            .dp_session
            .run(ort::inputs![dp_text_ids, dp_style, dp_mask])?;

        let (_, duration_raw) = dp_outputs[0].try_extract_tensor::<f32>()?;
        let duration = duration_raw.iter().next().copied().unwrap_or(1.0) / self.speed;

        // 2. Text encoding
        let te_text_ids =
            Value::from_array(([1usize, text_len], text_ids_raw))?;
        let te_style =
            Value::from_array((self.style.ttl_shape, self.style.ttl_data.clone()))?;
        let te_mask =
            Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;

        let te_outputs = self
            .text_enc_session
            .run(ort::inputs![te_text_ids, te_style, te_mask])?;

        let (te_shape, text_emb_raw) = te_outputs[0].try_extract_tensor::<f32>()?;
        let text_emb_data = text_emb_raw.to_vec();
        let text_emb_shape = [te_shape[0], te_shape[1], te_shape[2]];

        // 3. Sample noisy latent
        let chunk_size = self.base_chunk_size * self.chunk_compress_factor;
        let wav_len = (duration * self.sample_rate as f32) as i32;
        let latent_len = ((wav_len + chunk_size - 1) / chunk_size).max(1) as usize;
        let latent_dim_val = (self.latent_dim * self.chunk_compress_factor) as usize;

        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let normal = Normal::new(0.0f32, 1.0).unwrap();

        let mut noisy_latent: Vec<f32> = (0..latent_dim_val * latent_len)
            .map(|_| normal.sample(&mut rng))
            .collect();

        // Latent mask (all 1s for single-batch, no padding)
        let latent_mask: Vec<f32> = vec![1.0; latent_len];

        // 4. Denoising loop
        for step in 0..self.total_step {
            let v_latent =
                Value::from_array(([1usize, latent_dim_val, latent_len], noisy_latent.clone()))?;
            let v_text_emb =
                Value::from_array((text_emb_shape, text_emb_data.clone()))?;
            let v_style_ttl =
                Value::from_array((self.style.ttl_shape, self.style.ttl_data.clone()))?;
            let v_latent_mask =
                Value::from_array(([1usize, 1, latent_len], latent_mask.clone()))?;
            let v_text_mask =
                Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;
            let v_current =
                Value::from_array(([1usize], vec![step as f32]))?;
            let v_total =
                Value::from_array(([1usize], vec![self.total_step as f32]))?;

            let ve_outputs = self.vector_est_session.run(ort::inputs![
                v_latent,
                v_text_emb,
                v_style_ttl,
                v_latent_mask,
                v_text_mask,
                v_current,
                v_total
            ])?;

            let (_, denoised_raw) = ve_outputs[0].try_extract_tensor::<f32>()?;
            noisy_latent = denoised_raw.to_vec();
        }

        // 5. Vocoder
        let v_latent =
            Value::from_array(([1usize, latent_dim_val, latent_len], noisy_latent))?;
        let voc_outputs = self.vocoder_session.run(ort::inputs![v_latent])?;

        let (_, wav_raw) = voc_outputs[0].try_extract_tensor::<f32>()?;
        let mut samples = wav_raw.to_vec();

        // Trim to actual duration
        let actual_len = (duration * self.sample_rate as f32) as usize;
        if actual_len < samples.len() {
            samples.truncate(actual_len);
        }

        Ok(samples)
    }
}

impl TtsEngine for SupertonicEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        let chunks = chunk_text(text, MAX_TEXT_LEN);
        let mut all_samples = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let samples = self.infer(chunk, "en")?;
            all_samples.extend(samples);

            if i < chunks.len() - 1 {
                let silence = (self.sample_rate as f32 * 0.3) as usize;
                all_samples.extend(std::iter::repeat_n(0.0f32, silence));
            }
        }

        Ok(AudioOutput {
            samples: all_samples,
            sample_rate: self.sample_rate as u32,
            channels: 1,
        })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn engine_kind(&self) -> EngineKind {
        EngineKind::Supertonic
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        VOICES
            .iter()
            .map(|v| VoiceInfo {
                id: v.id.to_string(),
                name: v.name.to_string(),
                language: v.language.to_string(),
                description: format!("{} ({})", v.name, v.gender),
            })
            .collect()
    }

    fn set_voice(&mut self, voice_id: &str) -> Result<()> {
        let new_style = load_voice_style(&self.model_dir, voice_id)?;
        self.style = new_style;
        self.voice_id = voice_id.to_string();
        Ok(())
    }
}

// ── Voice style loading ──

fn load_voice_style(model_dir: &Path, voice_id: &str) -> Result<Style> {
    let voice_path = model_dir.join("voices").join(format!("{voice_id}.json"));

    if !voice_path.exists() {
        bail!(
            "Voice '{voice_id}' not found at {}.\n  Install it: local-voice voices install {voice_id}",
            voice_path.display()
        );
    }

    let data = std::fs::read_to_string(&voice_path)
        .with_context(|| format!("Failed to read voice file: {}", voice_path.display()))?;

    let vsd: VoiceStyleData = serde_json::from_str(&data)
        .with_context(|| "Failed to parse voice style JSON")?;

    let (ttl_data, ttl_shape) = flatten_style_component(&vsd.style_ttl)?;
    let (dp_data, dp_shape) = flatten_style_component(&vsd.style_dp)?;

    Ok(Style {
        ttl_data,
        ttl_shape,
        dp_data,
        dp_shape,
    })
}

fn flatten_style_component(sc: &StyleComponent) -> Result<(Vec<f32>, [usize; 3])> {
    let dims = &sc.dims;
    if dims.len() != 3 {
        bail!("Expected 3D style component, got {} dims", dims.len());
    }

    let mut flat = Vec::with_capacity(dims[0] * dims[1] * dims[2]);
    for d0 in &sc.data {
        for d1 in d0 {
            flat.extend_from_slice(d1);
        }
    }

    Ok((flat, [dims[0], dims[1], dims[2]]))
}

// ── Text chunking ──

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks = Vec::new();

    for para in paragraphs {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.len() <= max_len {
            chunks.push(trimmed.to_string());
            continue;
        }

        let sentences = split_sentences(trimmed);
        let mut current = String::new();

        for sent in sentences {
            if current.len() + sent.len() + 1 > max_len && !current.is_empty() {
                chunks.push(current.trim().to_string());
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&sent);
        }

        if !current.is_empty() {
            chunks.push(current.trim().to_string());
        }
    }

    if chunks.is_empty() {
        chunks.push(text.to_string());
    }

    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    let abbreviations = [
        "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.", "Jr.", "St.",
        "Ave.", "Rd.", "Blvd.", "Dept.", "Inc.", "Ltd.", "Co.", "Corp.",
        "etc.", "vs.", "i.e.", "e.g.", "Ph.D.",
    ];

    let mut i = 0;
    while i < len {
        current.push(chars[i]);

        if matches!(chars[i], '.' | '!' | '?') {
            let is_abbr = abbreviations.iter().any(|a| current.ends_with(a));

            if !is_abbr && (i + 1 >= len || chars[i + 1] == ' ') {
                sentences.push(current.trim().to_string());
                current.clear();
            }
        }

        i += 1;
    }

    if !current.is_empty() {
        sentences.push(current.trim().to_string());
    }

    sentences
}
