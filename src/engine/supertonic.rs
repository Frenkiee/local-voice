use anyhow::{Context, Result, bail};
use ort::value::Value;
use rand::SeedableRng;
use serde::Deserialize;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use crate::registry::supertonic::{LANGS_V1, LANGS_V3, VOICES, supported_languages};

/// Silence inserted between text chunks (matches the upstream reference).
const CHUNK_SILENCE_SECS: f32 = 0.3;
/// Max characters per chunk for Latin-script languages / for ko, ja.
const MAX_CHUNK_CHARS: usize = 300;
const MAX_CHUNK_CHARS_CJK: usize = 120;

// ── Config structs (from tts.json) ──

#[derive(Deserialize)]
struct TtsConfig {
    #[serde(default)]
    tts_version: Option<String>,
    #[serde(default)]
    split: Option<String>,
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

/// Model generation, derived from `tts.json`.
///
/// * `V1` (`tts_version` v1.5.x, split `opensource-en`): English only. The
///   reference preprocessor strips combining diacritics and passes bare text.
/// * `Multilingual` (v1.6+: Supertonic 2 and 3): text is wrapped in
///   `<lang>…</lang>` tags and diacritics are kept (they carry meaning in
///   most of the 31 languages). The ONNX graphs take no language input —
///   the language is expressed purely through those tag characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Generation {
    V1,
    Multilingual,
}

fn detect_generation(cfg: &TtsConfig) -> Generation {
    if cfg.split.as_deref() == Some("opensource-en") {
        return Generation::V1;
    }
    if let Some(v) = cfg.tts_version.as_deref() {
        let mut parts = v
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.parse::<u32>().unwrap_or(0));
        let major = parts.next().unwrap_or(1);
        let minor = parts.next().unwrap_or(0);
        if (major, minor) < (1, 6) {
            return Generation::V1;
        }
    }
    Generation::Multilingual
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
    #[allow(dead_code)]
    model_id: String,
    model_dir: std::path::PathBuf,
    generation: Generation,
    language: String,
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
        language: &str,
    ) -> Result<Self> {
        let cfg: TtsConfig = {
            let path = model_dir.join("tts.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };
        let generation = detect_generation(&cfg);

        // Validate the language against what this model was trained on.
        let supported: &[&str] = match generation {
            Generation::V1 => LANGS_V1,
            Generation::Multilingual => supported_languages(model_id).unwrap_or(LANGS_V3),
        };
        let language = language.trim().to_lowercase();
        // "na" is the upstream escape hatch for "no specific language".
        let lang_ok = supported.contains(&language.as_str())
            || (generation == Generation::Multilingual && language == "na");
        if !lang_ok {
            let hint = if generation == Generation::V1 {
                " Install a multilingual model: local-voice models install supertonic-3".to_string()
            } else {
                String::new()
            };
            bail!(
                "Language '{language}' is not supported by model '{model_id}' (supports: {}).{hint}",
                supported.join(", ")
            );
        }

        let indexer: Vec<i64> = {
            let path = model_dir.join("unicode_indexer.json");
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&data)?
        };

        let load_session =
            |name: &str, expected_inputs: &[&str]| -> Result<ort::session::Session> {
                let path = model_dir.join(name);
                if !path.exists() {
                    bail!("Supertonic model file not found: {}", path.display());
                }
                let session = ort::session::Session::builder()
                    .with_context(|| format!("Failed to create session builder for {name}"))?
                    .commit_from_file(&path)
                    .with_context(|| format!("Failed to load {name}"))?;
                check_inputs(&session, name, expected_inputs)?;
                Ok(session)
            };

        // All released generations (v1, 2, 3) share the same graph interface;
        // fail loudly if a future export changes it instead of erroring mid-run.
        let dp_session = load_session(
            "duration_predictor.onnx",
            &["text_ids", "style_dp", "text_mask"],
        )?;
        let text_enc_session =
            load_session("text_encoder.onnx", &["text_ids", "style_ttl", "text_mask"])?;
        let vector_est_session = load_session(
            "vector_estimator.onnx",
            &[
                "noisy_latent",
                "text_emb",
                "style_ttl",
                "latent_mask",
                "text_mask",
                "current_step",
                "total_step",
            ],
        )?;
        let vocoder_session = load_session("vocoder.onnx", &["latent"])?;

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
            generation,
            language,
            sample_rate: cfg.ae.sample_rate,
            base_chunk_size: cfg.ae.base_chunk_size,
            latent_dim: cfg.ttl.latent_dim,
            chunk_compress_factor: cfg.ttl.chunk_compress_factor,
            speed,
            total_step: total_step as usize,
        })
    }

    /// Language code currently used for synthesis (ignored by v1 models).
    #[allow(dead_code)]
    pub fn language(&self) -> &str {
        &self.language
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

    /// Run the 4-model pipeline on one (already chunked) piece of text.
    /// Returns samples trimmed to the predicted duration.
    fn infer_chunk(&mut self, chunk: &str) -> Result<Vec<f32>> {
        let processed = match self.generation {
            Generation::V1 => preprocess_text(chunk, true, None),
            Generation::Multilingual => preprocess_text(chunk, false, Some(&self.language)),
        };
        let text_ids_raw = self.tokenize(&processed);
        let text_len = text_ids_raw.len();
        if text_len == 0 {
            return Ok(Vec::new());
        }

        // Build text_mask: [1, 1, text_len] — all 1s (single batch, no padding)
        let text_mask: Vec<f32> = vec![1.0; text_len];

        // 1. Duration prediction
        let dp_text_ids = Value::from_array(([1usize, text_len], text_ids_raw.clone()))?;
        let dp_style = Value::from_array((self.style.dp_shape, self.style.dp_data.clone()))?;
        let dp_mask = Value::from_array(([1usize, 1, text_len], text_mask.clone()))?;

        let dp_outputs = self.dp_session.run(ort::inputs![
            "text_ids" => dp_text_ids,
            "style_dp" => dp_style,
            "text_mask" => dp_mask
        ])?;

        let (_, duration_raw) = dp_outputs[0].try_extract_tensor::<f32>()?;
        let duration = duration_raw.as_ref().first().copied().unwrap_or(1.0) / self.speed;

        // 2. Text encoding
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
        let text_emb_shape = [
            te_shape[0] as usize,
            te_shape[1] as usize,
            te_shape[2] as usize,
        ];

        // 3. Sample noisy latent
        let chunk_size = self.base_chunk_size * self.chunk_compress_factor;
        let wav_len = (duration * self.sample_rate as f32) as i32;
        let latent_len = ((wav_len + chunk_size - 1) / chunk_size).max(1) as usize;
        let latent_dim_val = (self.latent_dim * self.chunk_compress_factor) as usize;

        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let normal = rand_distr::Normal::new(0.0f32, 1.0).unwrap();
        let xt_init: Vec<f32> = (0..latent_dim_val * latent_len)
            .map(|_| rand_distr::Distribution::sample(&normal, &mut rng))
            .collect();

        let latent_mask: Vec<f32> = vec![1.0; latent_len];

        // 4. Denoising loop
        let mut xt = xt_init;
        for step in 0..self.total_step {
            let v_latent = Value::from_array(([1usize, latent_dim_val, latent_len], xt.clone()))?;
            let v_text_emb = Value::from_array((text_emb_shape, text_emb_data.clone()))?;
            let v_style_ttl =
                Value::from_array((self.style.ttl_shape, self.style.ttl_data.clone()))?;
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

            let (_, denoised_raw) = ve_outputs[0].try_extract_tensor::<f32>()?;
            xt = denoised_raw.to_vec();
        }

        // 5. Vocoder
        let v_latent = Value::from_array(([1usize, latent_dim_val, latent_len], xt))?;
        let voc_outputs = self.vocoder_session.run(ort::inputs![
            "latent" => v_latent
        ])?;

        let (_, wav_raw) = voc_outputs[0].try_extract_tensor::<f32>()?;
        let mut samples = wav_raw.to_vec();

        // Trim to actual duration
        let actual_len = (duration * self.sample_rate as f32) as usize;
        if actual_len < samples.len() {
            samples.truncate(actual_len);
        }

        Ok(samples)
    }

    fn infer(&mut self, text: &str) -> Result<Vec<f32>> {
        let max_chars = if self.generation == Generation::Multilingual
            && matches!(self.language.as_str(), "ko" | "ja")
        {
            MAX_CHUNK_CHARS_CJK
        } else {
            MAX_CHUNK_CHARS
        };

        let chunks = chunk_text(text, max_chars);
        if chunks.is_empty() {
            bail!("No text to synthesize");
        }

        let silence_len = (CHUNK_SILENCE_SECS * self.sample_rate as f32) as usize;
        let mut out: Vec<f32> = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let samples = self.infer_chunk(chunk)?;
            if samples.is_empty() {
                continue;
            }
            if i > 0 && !out.is_empty() {
                out.extend(std::iter::repeat_n(0.0f32, silence_len));
            }
            out.extend(samples);
        }

        Ok(out)
    }
}

/// Verify that a session exposes the inputs we are going to feed it.
fn check_inputs(session: &ort::session::Session, file: &str, expected: &[&str]) -> Result<()> {
    let names: Vec<&str> = session.inputs().iter().map(|o| o.name()).collect();
    for e in expected {
        if !names.contains(e) {
            bail!(
                "{file}: expected ONNX input '{e}' but the model exposes {names:?}. \
                 This Supertonic export is not supported by this version of local-voice."
            );
        }
    }
    Ok(())
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

// ── Text preprocessing (port of upstream py/helper.py `_preprocess_text`) ──

/// Normalize text the way the upstream reference does.
///
/// `strip_diacritics` reproduces the v1 (English-only) behaviour of dropping
/// combining marks after NFKD. `lang` wraps the result in `<lang>…</lang>`
/// tags, which is how Supertonic 2/3 are told which language to read.
fn preprocess_text(text: &str, strip_diacritics: bool, lang: Option<&str>) -> String {
    // NFKD normalization
    let mut s: String = text.nfkd().collect();

    // Remove emojis — match the reference regex ranges
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

    // Replace dashes, quotes, symbols — match the reference exactly.
    // Note: this runs before the language tags are added, so `/` → ` ` does
    // not eat the closing `</lang>` tag.
    let replacements: &[(&str, &str)] = &[
        ("\u{2013}", "-"),
        ("\u{2011}", "-"),
        ("\u{2014}", "-"),
        ("\u{00AF}", " "),
        ("_", " "),
        ("\u{201C}", "\""),
        ("\u{201D}", "\""),
        ("\u{2018}", "'"),
        ("\u{2019}", "'"),
        ("\u{00B4}", "'"),
        ("`", "'"),
        ("[", " "),
        ("]", " "),
        ("|", " "),
        ("/", " "),
        ("#", " "),
        ("\u{2192}", " "),
        ("\u{2190}", " "),
    ];
    for (from, to) in replacements {
        s = s.replace(from, to);
    }

    // v1 reference removes combining diacritics after NFKD (English-only
    // model). Multilingual models keep them: č, š, ž, ü, ñ ... are decomposed
    // by NFKD into base letter + combining mark and the indexer maps both.
    if strip_diacritics {
        s = s
            .chars()
            .filter(|c| {
                let cp = *c as u32;
                !matches!(
                    cp,
                    0x0302
                        | 0x0303
                        | 0x0304
                        | 0x0305
                        | 0x0306
                        | 0x0307
                        | 0x0308
                        | 0x030A
                        | 0x030B
                        | 0x030C
                        | 0x0327
                        | 0x0328
                        | 0x0329
                        | 0x032A
                        | 0x032B
                        | 0x032C
                        | 0x032D
                        | 0x032E
                        | 0x032F
                )
            })
            .collect();
    }

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
    while s.contains("\"\"") {
        s = s.replace("\"\"", "\"");
    }
    while s.contains("''") {
        s = s.replace("''", "'");
    }
    while s.contains("``") {
        s = s.replace("``", "`");
    }

    // Collapse all whitespace runs (incl. newlines/tabs) to a single space
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");

    // Add terminal period if needed — match the reference regex check
    if let Some(last) = s.chars().last()
        && !matches!(
            last,
            '.' | '!'
                | '?'
                | ';'
                | ':'
                | ','
                | '\''
                | '"'
                | ')'
                | ']'
                | '}'
                | '\u{2026}'
                | '\u{3002}'
                | '\u{300D}'
                | '\u{300F}'
                | '\u{3011}'
                | '\u{3009}'
                | '\u{300B}'
                | '\u{203A}'
                | '\u{00BB}'
        )
    {
        s.push('.');
    }

    if let Some(lang) = lang {
        s = format!("<{lang}>{s}</{lang}>");
    }

    s
}

// ── Text chunking (port of upstream `chunk_text`) ──

/// Trailing tokens after which a `.` does not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "Mr.", "Mrs.", "Ms.", "Dr.", "Prof.", "Sr.", "Jr.", "Ph.D.", "etc.", "e.g.", "i.e.", "vs.",
    "Inc.", "Ltd.", "Co.", "Corp.", "St.", "Ave.", "Blvd.",
];

fn ends_with_abbreviation(s: &str) -> bool {
    if ABBREVIATIONS.iter().any(|a| s.ends_with(a)) {
        return true;
    }
    // Single capital initial, e.g. "John F. Kennedy"
    let mut it = s.chars().rev();
    matches!(
        (it.next(), it.next(), it.next()),
        (Some('.'), Some(c), prev) if c.is_ascii_uppercase() && prev.is_none_or(|p| !p.is_alphanumeric())
    )
}

/// Split a paragraph into sentences on `.`, `!`, `?` followed by whitespace.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?')
            && i + 1 < chars.len()
            && chars[i + 1].is_whitespace()
        {
            let candidate: String = chars[start..=i].iter().collect();
            if !ends_with_abbreviation(&candidate) {
                let trimmed = candidate.trim();
                if !trimmed.is_empty() {
                    sentences.push(trimmed.to_string());
                }
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < chars.len() {
        let rest: String = chars[start..].iter().collect();
        let trimmed = rest.trim();
        if !trimmed.is_empty() {
            sentences.push(trimmed.to_string());
        }
    }
    sentences
}

/// Split long text into chunks of at most `max_chars` characters, by paragraph
/// (blank line) and then sentence boundaries. Sentences longer than
/// `max_chars` are split on whitespace as a last resort.
fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();

    // Paragraphs: separated by one or more blank lines
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current_para = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current_para.trim().is_empty() {
                paragraphs.push(current_para.trim().to_string());
            }
            current_para.clear();
        } else {
            if !current_para.is_empty() {
                current_para.push(' ');
            }
            current_para.push_str(line.trim());
        }
    }
    if !current_para.trim().is_empty() {
        paragraphs.push(current_para.trim().to_string());
    }

    for paragraph in paragraphs {
        let mut current = String::new();
        let mut current_len = 0usize;

        let push_piece = |piece: &str,
                          chunks: &mut Vec<String>,
                          current: &mut String,
                          current_len: &mut usize| {
            let piece_len = piece.chars().count();
            if *current_len + piece_len < max_chars || current.is_empty() {
                if !current.is_empty() {
                    current.push(' ');
                    *current_len += 1;
                }
                current.push_str(piece);
                *current_len += piece_len;
            } else {
                chunks.push(std::mem::take(current));
                *current = piece.to_string();
                *current_len = piece_len;
            }
        };

        for sentence in split_sentences(&paragraph) {
            if sentence.chars().count() > max_chars {
                // Overlong sentence: fall back to word-level pieces
                for word in sentence.split_whitespace() {
                    push_piece(word, &mut chunks, &mut current, &mut current_len);
                }
            } else {
                push_piece(&sentence, &mut chunks, &mut current, &mut current_len);
            }
        }

        if !current.is_empty() {
            chunks.push(current);
        }
    }

    chunks
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

    let vsd: VoiceStyleData =
        serde_json::from_str(&data).with_context(|| "Failed to parse voice style JSON")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(version: Option<&str>, split: Option<&str>) -> TtsConfig {
        TtsConfig {
            tts_version: version.map(String::from),
            split: split.map(String::from),
            ae: AEConfig {
                sample_rate: 44100,
                base_chunk_size: 512,
            },
            ttl: TTLConfig {
                latent_dim: 24,
                chunk_compress_factor: 6,
            },
        }
    }

    #[test]
    fn generation_detection() {
        assert_eq!(
            detect_generation(&cfg(Some("v1.5.0"), Some("opensource-en"))),
            Generation::V1
        );
        assert_eq!(
            detect_generation(&cfg(Some("v1.6.0"), Some("opensource-multilingual"))),
            Generation::Multilingual
        );
        assert_eq!(
            detect_generation(&cfg(Some("v1.7.3"), Some("opensource-multilingual"))),
            Generation::Multilingual
        );
        assert_eq!(
            detect_generation(&cfg(None, None)),
            Generation::Multilingual
        );
        assert_eq!(
            detect_generation(&cfg(None, Some("opensource-en"))),
            Generation::V1
        );
    }

    #[test]
    fn multilingual_preprocess_wraps_in_lang_tags_and_keeps_diacritics() {
        let out = preprocess_text("Dober dan, kako si danes", false, Some("sl"));
        assert!(out.starts_with("<sl>"), "{out}");
        assert!(out.ends_with("</sl>"), "{out}");
        // Period added before the closing tag
        assert!(out.contains("danes.</sl>"), "{out}");

        let out = preprocess_text("čšž", false, Some("sl"));
        // NFKD decomposes č into c + U+030C; the combining mark must survive.
        assert!(out.contains('\u{030C}'), "{out}");
        assert_eq!(out, "<sl>c\u{030C}s\u{030C}z\u{030C}.</sl>");
    }

    #[test]
    fn v1_preprocess_strips_diacritics_and_has_no_tags() {
        // ï (U+0308) and č (U+030C) are in the v1 strip list; é (U+0301) is not
        let out = preprocess_text("naïve čas café", true, None);
        assert_eq!(out, "naive cas cafe\u{0301}.");
        assert!(!out.contains('<'));
    }

    #[test]
    fn preprocess_collapses_all_whitespace_and_keeps_expression_tags() {
        let out = preprocess_text("Hello\n\tworld  <laugh> ok!", false, Some("en"));
        assert_eq!(out, "<en>Hello world <laugh> ok!</en>");
    }

    #[test]
    fn slash_replacement_does_not_break_closing_tag() {
        let out = preprocess_text("a/b", false, Some("en"));
        assert_eq!(out, "<en>a b.</en>");
    }

    #[test]
    fn chunking_splits_on_sentences_and_respects_max_len() {
        let text = "First sentence here. Second one! Third? Fourth.";
        let chunks = chunk_text(text, 25);
        assert_eq!(
            chunks,
            vec!["First sentence here.", "Second one! Third?", "Fourth."]
        );
        // Short text: single chunk
        assert_eq!(chunk_text(text, 300), vec![text.to_string()]);
    }

    #[test]
    fn chunking_keeps_abbreviations_and_initials_together() {
        let s = split_sentences("Dr. Smith met J. Doe. Then left.");
        assert_eq!(s, vec!["Dr. Smith met J. Doe.", "Then left."]);
    }

    #[test]
    fn chunking_splits_paragraphs_and_overlong_sentences() {
        let chunks = chunk_text("Para one.\n\n\nPara two.", 300);
        assert_eq!(chunks, vec!["Para one.", "Para two."]);

        let long = "word ".repeat(20).trim().to_string(); // 99 chars, no punctuation
        let chunks = chunk_text(&long, 30);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 30), "{chunks:?}");
        assert_eq!(chunks.join(" "), long);
    }

    #[test]
    fn chunking_counts_chars_not_bytes() {
        let sl = "Čas je zlato. Šola je super. Žoga je okrogla.";
        let chunks = chunk_text(sl, 16);
        assert_eq!(
            chunks,
            vec!["Čas je zlato.", "Šola je super.", "Žoga je okrogla."]
        );
    }
}
