use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use crate::phonemize::Phonemizer;
use crate::registry::kokoro::VOICES;
use ort::value::Value;

const MAX_PHONEME_LENGTH: usize = 510;

pub struct KokoroEngine {
    session: ort::session::Session,
    vocab: HashMap<String, i64>,
    voice_style: Vec<f32>,
    voice_id: String,
    model_id: String,
    model_dir: std::path::PathBuf,
    speed: f32,
    phonemizer: Phonemizer,
}

impl KokoroEngine {
    pub fn load(model_dir: &Path, model_id: &str, voice_id: &str, speed: f32) -> Result<Self> {
        let onnx_path = model_dir.join("model.onnx");
        if !onnx_path.exists() {
            bail!("Kokoro model not found: {}", onnx_path.display());
        }

        // Load vocab from config.json (embedded)
        let vocab = build_vocab();

        // Load ONNX session
        let session = ort::session::Session::builder()
            .with_context(|| "Failed to create ONNX session builder")?
            .commit_from_file(&onnx_path)
            .with_context(|| format!("Failed to load Kokoro model from {}", onnx_path.display()))?;

        // Load voice style vector
        let voice_style = load_voice_style(model_dir, voice_id)?;

        let phonemizer = Phonemizer::new()?;

        Ok(Self {
            session,
            vocab,
            voice_style,
            voice_id: voice_id.to_string(),
            model_id: model_id.to_string(),
            model_dir: model_dir.to_path_buf(),
            speed,
            phonemizer,
        })
    }

    fn tokenize(&self, phonemes: &str) -> Vec<i64> {
        let mut tokens = Vec::new();

        for ch in phonemes.chars() {
            let key = ch.to_string();
            if let Some(&id) = self.vocab.get(&key) {
                tokens.push(id);
            }
            // Skip unknown characters silently
        }

        tokens
    }

    fn run_inference(&mut self, tokens: &[i64]) -> Result<Vec<f32>> {
        // Pad with 0 at start and end
        let mut input_ids: Vec<i64> = Vec::with_capacity(tokens.len() + 2);
        input_ids.push(0);
        input_ids.extend_from_slice(tokens);
        input_ids.push(0);

        let seq_len = input_ids.len();

        let input_value = Value::from_array(([1usize, seq_len], input_ids))
            .with_context(|| "Failed to create input_ids tensor")?;

        // Style vector: shape [1, 256] — select the appropriate segment
        // Voice .bin files contain float32 data with shape [-1, 1, 256]
        // We pick the style for a token index (clamped to available range)
        let style_idx = (seq_len / 2).min(self.voice_style.len() / 256).saturating_sub(1);
        let style_offset = style_idx * 256;
        let style_slice = if style_offset + 256 <= self.voice_style.len() {
            &self.voice_style[style_offset..style_offset + 256]
        } else {
            // Fallback to first style
            &self.voice_style[..256]
        };

        let style_value = Value::from_array(([1usize, 256usize], style_slice.to_vec()))
            .with_context(|| "Failed to create style tensor")?;

        let speed_value = Value::from_array(([1usize], vec![self.speed as i64]))
            .with_context(|| "Failed to create speed tensor")?;

        let outputs = self
            .session
            .run(ort::inputs![input_value, style_value, speed_value])
            .with_context(|| "Kokoro ONNX inference failed")?;

        let (_, audio_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .with_context(|| "Failed to extract audio from Kokoro output")?;

        Ok(audio_data.to_vec())
    }
}

impl TtsEngine for KokoroEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        // Phonemize the text using espeak-ng
        let phonemes = self.phonemizer.phonemize(text, "en-us")?;
        if phonemes.is_empty() {
            bail!("No phonemes generated for input text");
        }

        // Split into chunks at punctuation boundaries, respecting max length
        let chunks = split_phonemes(&phonemes, MAX_PHONEME_LENGTH);
        let mut all_samples = Vec::new();

        for chunk in &chunks {
            let tokens = self.tokenize(chunk);
            if tokens.is_empty() {
                continue;
            }

            let samples = self.run_inference(&tokens)?;
            all_samples.extend(samples);

            // Add silence between chunks
            if chunks.len() > 1 {
                let silence = (24000.0 * 0.15) as usize;
                all_samples.extend(std::iter::repeat_n(0.0f32, silence));
            }
        }

        Ok(AudioOutput {
            samples: all_samples,
            sample_rate: 24000,
            channels: 1,
        })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn engine_kind(&self) -> EngineKind {
        EngineKind::Kokoro
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
        self.voice_style = new_style;
        self.voice_id = voice_id.to_string();
        Ok(())
    }
}

/// Load a voice style vector from a .bin file
fn load_voice_style(model_dir: &Path, voice_id: &str) -> Result<Vec<f32>> {
    let voice_path = model_dir.join("voices").join(format!("{voice_id}.bin"));

    if !voice_path.exists() {
        bail!(
            "Voice '{voice_id}' not found at {}.\n  Install it: local-voice voices install {voice_id}",
            voice_path.display()
        );
    }

    let data = std::fs::read(&voice_path)
        .with_context(|| format!("Failed to read voice file: {}", voice_path.display()))?;

    // Voice .bin files are raw float32 arrays
    if data.len() % 4 != 0 {
        bail!("Invalid voice file: size {} is not aligned to 4 bytes", data.len());
    }

    let floats: Vec<f32> = data
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    if floats.len() < 256 {
        bail!(
            "Voice file too small: got {} floats, need at least 256",
            floats.len()
        );
    }

    Ok(floats)
}

/// Split phonemes at punctuation boundaries, keeping chunks under max_len
fn split_phonemes(phonemes: &str, max_len: usize) -> Vec<String> {
    let parts: Vec<&str> = phonemes.split_inclusive(&['.', '!', '?', ';', ','][..]).collect();
    let mut chunks = Vec::new();
    let mut current = String::new();

    for part in parts {
        if current.len() + part.len() > max_len && !current.is_empty() {
            chunks.push(current.clone());
            current.clear();
        }
        current.push_str(part);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(phonemes.to_string());
    }

    chunks
}

/// Build the Kokoro vocabulary mapping (phoneme char -> token ID)
fn build_vocab() -> HashMap<String, i64> {
    let mut v = HashMap::new();
    // Punctuation
    v.insert(";".into(), 1);
    v.insert(":".into(), 2);
    v.insert(",".into(), 3);
    v.insert(".".into(), 4);
    v.insert("!".into(), 5);
    v.insert("?".into(), 6);
    v.insert("—".into(), 9);
    v.insert("…".into(), 10);
    v.insert("\"".into(), 11);
    v.insert("(".into(), 12);
    v.insert(")".into(), 13);
    v.insert("\u{201c}".into(), 14); // "
    v.insert("\u{201d}".into(), 15); // "
    v.insert(" ".into(), 16);
    v.insert("\u{0303}".into(), 17); // combining tilde
    // Phonetic affricates
    v.insert("ʣ".into(), 18);
    v.insert("ʥ".into(), 19);
    v.insert("ʦ".into(), 20);
    v.insert("ʨ".into(), 21);
    v.insert("ᵝ".into(), 22);
    v.insert("\u{AB67}".into(), 23);
    // Latin uppercase
    v.insert("A".into(), 24);
    v.insert("I".into(), 25);
    v.insert("O".into(), 31);
    v.insert("Q".into(), 33);
    v.insert("S".into(), 35);
    v.insert("T".into(), 36);
    v.insert("W".into(), 39);
    v.insert("Y".into(), 41);
    v.insert("ᵊ".into(), 42);
    // Latin lowercase
    v.insert("a".into(), 43);
    v.insert("b".into(), 44);
    v.insert("c".into(), 45);
    v.insert("d".into(), 46);
    v.insert("e".into(), 47);
    v.insert("f".into(), 48);
    v.insert("h".into(), 50);
    v.insert("i".into(), 51);
    v.insert("j".into(), 52);
    v.insert("k".into(), 53);
    v.insert("l".into(), 54);
    v.insert("m".into(), 55);
    v.insert("n".into(), 56);
    v.insert("o".into(), 57);
    v.insert("p".into(), 58);
    v.insert("q".into(), 59);
    v.insert("r".into(), 60);
    v.insert("s".into(), 61);
    v.insert("t".into(), 62);
    v.insert("u".into(), 63);
    v.insert("v".into(), 64);
    v.insert("w".into(), 65);
    v.insert("x".into(), 66);
    v.insert("y".into(), 67);
    v.insert("z".into(), 68);
    // IPA vowels
    v.insert("ɑ".into(), 69);
    v.insert("ɐ".into(), 70);
    v.insert("ɒ".into(), 71);
    v.insert("æ".into(), 72);
    v.insert("β".into(), 75);
    v.insert("ɔ".into(), 76);
    v.insert("ɕ".into(), 77);
    v.insert("ç".into(), 78);
    v.insert("ɖ".into(), 80);
    v.insert("ð".into(), 81);
    v.insert("ʤ".into(), 82);
    v.insert("ə".into(), 83);
    v.insert("ɚ".into(), 85);
    v.insert("ɛ".into(), 86);
    v.insert("ɜ".into(), 87);
    v.insert("ɟ".into(), 90);
    v.insert("ɡ".into(), 92);
    v.insert("ɥ".into(), 99);
    v.insert("ɨ".into(), 101);
    v.insert("ɪ".into(), 102);
    v.insert("ʝ".into(), 103);
    v.insert("ɯ".into(), 110);
    v.insert("ɰ".into(), 111);
    v.insert("ŋ".into(), 112);
    v.insert("ɳ".into(), 113);
    v.insert("ɲ".into(), 114);
    v.insert("ɴ".into(), 115);
    v.insert("ø".into(), 116);
    v.insert("ɸ".into(), 118);
    v.insert("θ".into(), 119);
    v.insert("œ".into(), 120);
    v.insert("ɹ".into(), 123);
    v.insert("ɾ".into(), 125);
    v.insert("ɻ".into(), 126);
    v.insert("ʁ".into(), 128);
    v.insert("ɽ".into(), 129);
    v.insert("ʂ".into(), 130);
    v.insert("ʃ".into(), 131);
    v.insert("ʈ".into(), 132);
    v.insert("ʧ".into(), 133);
    v.insert("ʊ".into(), 135);
    v.insert("ʋ".into(), 136);
    v.insert("ʌ".into(), 138);
    v.insert("ɣ".into(), 139);
    v.insert("ɤ".into(), 140);
    v.insert("χ".into(), 142);
    v.insert("ʎ".into(), 143);
    v.insert("ʒ".into(), 147);
    v.insert("ʔ".into(), 148);
    // Prosodic markers
    v.insert("ˈ".into(), 156);
    v.insert("ˌ".into(), 157);
    v.insert("ː".into(), 158);
    v.insert("ʰ".into(), 162);
    v.insert("ʲ".into(), 164);
    // Intonation
    v.insert("↓".into(), 169);
    v.insert("→".into(), 171);
    v.insert("↗".into(), 172);
    v.insert("↘".into(), 173);
    v.insert("ᵻ".into(), 177);
    v
}
