use anyhow::{bail, Context, Result};
use ort::value::Value;
use rand::SeedableRng;
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
        let cfg: TtsConfig = {
            let path = model_dir.join("tts.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };

        let indexer: Vec<i64> = {
            let path = model_dir.join("unicode_indexer.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };

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

        // Log ONNX model input names for debugging
        eprintln!("[supertonic] DP inputs: {:?}", dp_session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>());
        eprintln!("[supertonic] TE inputs: {:?}", text_enc_session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>());
        eprintln!("[supertonic] VE inputs: {:?}", vector_est_session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>());
        eprintln!("[supertonic] VOC inputs: {:?}", vocoder_session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>());

        let style = load_voice_style(model_dir, voice_id)?;
        eprintln!("[supertonic] style_ttl shape={:?} len={}, style_dp shape={:?} len={}", style.ttl_shape, style.ttl_data.len(), style.dp_shape, style.dp_data.len());

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

    fn preprocess_text(&self, text: &str) -> String {
        // NFKD normalization
        let mut s: String = text.nfkd().collect();

        // Remove emojis — match the Python reference regex ranges
        s = s
            .chars()
            .filter(|c| {
                let cp = *c as u32;
                !matches!(cp,
                    0x1F600..=0x1F64F |  // emoticons
                    0x1F300..=0x1F5FF |  // misc symbols & pictographs
                    0x1F680..=0x1F6FF |  // transport & map
                    0x1F700..=0x1F77F |
                    0x1F780..=0x1F7FF |
                    0x1F800..=0x1F8FF |
                    0x1F900..=0x1F9FF |
                    0x1FA00..=0x1FA6F |
                    0x1FA70..=0x1FAFF |
                    0x2600..=0x26FF   |  // misc symbols
                    0x2700..=0x27BF   |  // dingbats
                    0x1F1E6..=0x1F1FF    // flags
                )
            })
            .collect();

        // Replace dashes, quotes, symbols — match Python reference exactly
        let replacements: &[(&str, &str)] = &[
            ("\u{2013}", "-"), ("\u{2011}", "-"), ("\u{2014}", "-"),
            ("\u{00AF}", " "), ("_", " "),
            ("\u{201C}", "\""), ("\u{201D}", "\""),
            ("\u{2018}", "'"), ("\u{2019}", "'"),
            ("\u{00B4}", "'"), ("`", "'"),
            ("[", " "), ("]", " "),
            ("|", " "), ("/", " "),
            ("#", " "), ("\u{2192}", " "), ("\u{2190}", " "),
        ];
        for (from, to) in replacements {
            s = s.replace(from, to);
        }

        // Remove combining diacritics (Python reference does this after NFKD)
        s = s.chars().filter(|c| {
            let cp = *c as u32;
            !matches!(cp,
                0x0302 | 0x0303 | 0x0304 | 0x0305 | 0x0306 | 0x0307 | 0x0308 |
                0x030A | 0x030B | 0x030C | 0x0327 | 0x0328 | 0x0329 | 0x032A |
                0x032B | 0x032C | 0x032D | 0x032E | 0x032F
            )
        }).collect();

        // Remove special symbols
        s = s.replace('\u{2665}', ""); // ♥
        s = s.replace('\u{2606}', ""); // ☆
        s = s.replace('\u{2661}', ""); // ♡
        s = s.replace('\u{00A9}', ""); // ©
        s = s.replace('\\', "");

        // Expression replacements
        s = s.replace('@', " at ");
        s = s.replace("e.g.,", "for example, ");
        s = s.replace("i.e.,", "that is, ");

        // Fix spacing around punctuation
        s = s.replace(" ,", ",");
        s = s.replace(" .", ".");
        s = s.replace(" !", "!");
        s = s.replace(" ?", "?");
        s = s.replace(" ;", ";");
        s = s.replace(" :", ":");
        s = s.replace(" '", "'");

        // Remove duplicate quotes
        while s.contains("\"\"") { s = s.replace("\"\"", "\""); }
        while s.contains("''") { s = s.replace("''", "'"); }
        while s.contains("``") { s = s.replace("``", "`"); }

        // Collapse whitespace
        while s.contains("  ") { s = s.replace("  ", " "); }
        s = s.trim().to_string();

        // Add terminal period if needed — match Python regex check
        if !s.is_empty() {
            let last = s.chars().last().unwrap();
            if !matches!(last,
                '.' | '!' | '?' | ';' | ':' | ',' | '\'' | '"' |
                ')' | ']' | '}' | '\u{2026}' | '\u{3002}' | '\u{300D}' |
                '\u{300F}' | '\u{3011}' | '\u{3009}' | '\u{300B}' |
                '\u{203A}' | '\u{00BB}'
            ) {
                s.push('.');
            }
        }

        s
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

    fn infer(&mut self, text: &str) -> Result<Vec<f32>> {
        let processed = self.preprocess_text(text);
        let text_ids_raw = self.tokenize(&processed);
        let text_len = text_ids_raw.len();

        eprintln!("[supertonic] preprocessed: {processed}");
        eprintln!("[supertonic] text_ids len={text_len}, first 20: {:?}", &text_ids_raw[..text_len.min(20)]);

        // Build text_mask: [1, 1, text_len] — all 1s (single batch, no padding)
        let text_mask: Vec<f32> = vec![1.0; text_len];

        // 1. Duration prediction — use NAMED inputs
        let dp_text_ids = Value::from_array(([1usize, text_len], text_ids_raw.clone()))?;
        let dp_style = Value::from_array((self.style.dp_shape, self.style.dp_data.clone()))?;
        let dp_mask = Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;

        let dp_outputs = self.dp_session.run(ort::inputs![
            "text_ids" => dp_text_ids,
            "style_dp" => dp_style,
            "text_mask" => dp_mask
        ])?;

        let (dp_shape, duration_raw) = dp_outputs[0].try_extract_tensor::<f32>()?;
        let dur_slice = duration_raw.as_ref();
        eprintln!("[supertonic] duration shape={dp_shape:?}, values={dur_slice:?}");
        let duration = dur_slice.first().copied().unwrap_or(1.0) / self.speed;
        eprintln!("[supertonic] duration after speed={duration:.4}s");

        // 2. Text encoding — use NAMED inputs
        let te_text_ids = Value::from_array(([1usize, text_len], text_ids_raw))?;
        let te_style = Value::from_array((self.style.ttl_shape, self.style.ttl_data.clone()))?;
        let te_mask = Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;

        let te_outputs = self.text_enc_session.run(ort::inputs![
            "text_ids" => te_text_ids,
            "style_ttl" => te_style,
            "text_mask" => te_mask
        ])?;

        let (te_shape, text_emb_raw) = te_outputs[0].try_extract_tensor::<f32>()?;
        let text_emb_data = text_emb_raw.to_vec();
        let text_emb_shape = [te_shape[0] as usize, te_shape[1] as usize, te_shape[2] as usize];
        eprintln!("[supertonic] text_emb shape={text_emb_shape:?}");

        // 3. Sample noisy latent
        let chunk_size = self.base_chunk_size * self.chunk_compress_factor;
        let wav_len = (duration * self.sample_rate as f32) as i32;
        let latent_len = ((wav_len + chunk_size - 1) / chunk_size).max(1) as usize;
        let latent_dim_val = (self.latent_dim * self.chunk_compress_factor) as usize;

        eprintln!("[supertonic] latent: dim={latent_dim_val}, len={latent_len}, wav_len={wav_len}");

        // Use fixed-seed noise for deterministic output
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let normal = rand_distr::Normal::new(0.0f32, 1.0).unwrap();
        let xt_init: Vec<f32> = (0..latent_dim_val * latent_len)
            .map(|_| rand_distr::Distribution::sample(&normal, &mut rng))
            .collect();

        // Latent mask: [1, 1, latent_len]
        let latent_mask: Vec<f32> = vec![1.0; latent_len];

        // 4. Denoising loop — use NAMED inputs
        let mut xt = xt_init;
        for step in 0..self.total_step {
            let v_latent = Value::from_array(([1usize, latent_dim_val, latent_len], xt.clone()))?;
            let v_text_emb = Value::from_array((text_emb_shape, text_emb_data.clone()))?;
            let v_style_ttl = Value::from_array((self.style.ttl_shape, self.style.ttl_data.clone()))?;
            let v_latent_mask = Value::from_array(([1usize, 1, latent_len], latent_mask.clone()))?;
            let v_text_mask = Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;
            let v_current = Value::from_array(([1usize], vec![step as f32]))?;
            let v_total = Value::from_array(([1usize], vec![self.total_step as f32]))?;

            let ve_outputs = self.vector_est_session.run(ort::inputs![
                "noisy_latent" => v_latent,
                "text_emb" => v_text_emb,
                "style_ttl" => v_style_ttl,
                "latent_mask" => v_latent_mask,
                "text_mask" => v_text_mask,
                "current_step" => v_current,
                "total_step" => v_total
            ])?;

            let (ve_shape, denoised_raw) = ve_outputs[0].try_extract_tensor::<f32>()?;
            xt = denoised_raw.to_vec();
            let xt_min = xt.iter().cloned().fold(f32::INFINITY, f32::min);
            let xt_max = xt.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let xt_mean = xt.iter().sum::<f32>() / xt.len() as f32;
            eprintln!("[supertonic] step {step}/{}: shape={ve_shape:?} min={xt_min:.4} max={xt_max:.4} mean={xt_mean:.6}", self.total_step);
        }

        // 5. Vocoder — use NAMED input
        let v_latent = Value::from_array(([1usize, latent_dim_val, latent_len], xt))?;
        let voc_outputs = self.vocoder_session.run(ort::inputs![
            "latent" => v_latent
        ])?;

        let (voc_shape, wav_raw) = voc_outputs[0].try_extract_tensor::<f32>()?;
        let mut samples = wav_raw.to_vec();
        let wav_min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let wav_max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let wav_mean = samples.iter().sum::<f32>() / samples.len() as f32;
        eprintln!("[supertonic] vocoder output shape={voc_shape:?}, samples={}, min={wav_min:.4} max={wav_max:.4} mean={wav_mean:.6}", samples.len());

        // Trim to actual duration
        let actual_len = (duration * self.sample_rate as f32) as usize;
        eprintln!("[supertonic] trimming to {actual_len} samples (from {})", samples.len());
        if actual_len < samples.len() {
            samples.truncate(actual_len);
        }

        Ok(samples)
    }
}

impl TtsEngine for SupertonicEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        let samples = self.infer(text)?;

        Ok(AudioOutput {
            samples,
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
