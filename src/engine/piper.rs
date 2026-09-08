use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use crate::phonemize::Phonemizer;
use ort::value::Value;

/// Piper model configuration (from .onnx.json)
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PiperConfig {
    pub audio: AudioConfig,
    pub espeak: Option<EspeakConfig>,
    pub inference: InferenceConfig,
    pub phoneme_id_map: HashMap<String, Vec<i64>>,
    /// `"espeak"` (default) or `"text"`: text models map raw (case-folded)
    /// characters straight to IDs without running espeak-ng.
    #[serde(default)]
    pub phoneme_type: String,
    /// Multi-speaker models (> 1) require an extra `sid` input tensor.
    #[serde(default)]
    pub num_speakers: u32,
}

impl PiperConfig {
    /// Text models (e.g. `uk_UA-ukrainian_tts-medium`) take characters, not IPA.
    pub fn uses_raw_text(&self) -> bool {
        self.phoneme_type == "text"
    }

    pub fn is_multi_speaker(&self) -> bool {
        self.num_speakers > 1
    }
}

#[derive(Debug, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
}

#[derive(Debug, Deserialize)]
pub struct EspeakConfig {
    pub voice: String,
}

#[derive(Debug, Deserialize)]
pub struct InferenceConfig {
    pub noise_scale: f32,
    pub length_scale: f32,
    pub noise_w: f32,
}

pub struct PiperEngine {
    session: ort::session::Session,
    config: PiperConfig,
    model_id: String,
    /// `None` for `phoneme_type: "text"` models, which never call espeak-ng.
    phonemizer: Option<Phonemizer>,
    /// Speaker index for multi-speaker models (always 0 for now).
    speaker_id: i64,
}

impl PiperEngine {
    pub fn load(model_dir: &Path, model_id: &str) -> Result<Self> {
        let onnx_path = model_dir.join("model.onnx");
        let config_path = model_dir.join("model.onnx.json");

        if !onnx_path.exists() {
            bail!("Model file not found: {}", onnx_path.display());
        }
        if !config_path.exists() {
            bail!("Config file not found: {}", config_path.display());
        }

        let config_str = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let config: PiperConfig =
            serde_json::from_str(&config_str).with_context(|| "Failed to parse model config")?;

        let session = ort::session::Session::builder()
            .with_context(|| "Failed to create ONNX session builder")?
            .commit_from_file(&onnx_path)
            .with_context(|| format!("Failed to load ONNX model from {}", onnx_path.display()))?;

        let phonemizer = if config.uses_raw_text() {
            None
        } else {
            Some(Phonemizer::new()?)
        };

        Ok(Self {
            session,
            config,
            model_id: model_id.to_string(),
            phonemizer,
            speaker_id: 0,
        })
    }

    /// Turn a sentence into the phoneme string the model's `phoneme_id_map` expects.
    fn phonemize(&self, sentence: &str) -> Result<String> {
        match &self.phonemizer {
            None => Ok(sentence.to_lowercase()),
            Some(p) => {
                let voice = self
                    .config
                    .espeak
                    .as_ref()
                    .map(|e| e.voice.as_str())
                    .unwrap_or("en-us");
                p.phonemize(sentence, voice)
            }
        }
    }

    fn phonemes_to_ids(&self, phonemes: &str) -> Vec<i64> {
        let map = &self.config.phoneme_id_map;

        let pad = map.get("_").and_then(|v| v.first().copied()).unwrap_or(0);
        let bos = map.get("^").and_then(|v| v.first().copied()).unwrap_or(1);
        let eos = map.get("$").and_then(|v| v.first().copied()).unwrap_or(2);

        let mut ids = vec![bos, pad];

        for ch in phonemes.chars() {
            let key = ch.to_string();
            if let Some(phoneme_ids) = map.get(&key) {
                ids.extend(phoneme_ids);
                ids.push(pad);
            }
        }

        ids.push(eos);
        ids
    }

    fn run_inference(&mut self, phoneme_ids: &[i64]) -> Result<Vec<f32>> {
        let len = phoneme_ids.len();

        let input_value = Value::from_array(([1usize, len], phoneme_ids.to_vec()))
            .with_context(|| "Failed to create input value")?;

        let lengths_value = Value::from_array(([1usize], vec![len as i64]))
            .with_context(|| "Failed to create lengths value")?;

        let scales_value = Value::from_array((
            [3usize],
            vec![
                self.config.inference.noise_scale,
                self.config.inference.length_scale,
                self.config.inference.noise_w,
            ],
        ))
        .with_context(|| "Failed to create scales value")?;

        let outputs = if self.config.is_multi_speaker() {
            let sid_value = Value::from_array(([1usize], vec![self.speaker_id]))
                .with_context(|| "Failed to create speaker id value")?;
            self.session
                .run(ort::inputs![
                    input_value,
                    lengths_value,
                    scales_value,
                    sid_value
                ])
                .with_context(|| "ONNX inference failed")?
        } else {
            self.session
                .run(ort::inputs![input_value, lengths_value, scales_value])
                .with_context(|| "ONNX inference failed")?
        };

        let (_, audio_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .with_context(|| "Failed to extract audio from model output")?;

        Ok(audio_data.to_vec())
    }
}

impl TtsEngine for PiperEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        let sentences = split_sentences(text);
        let mut all_samples = Vec::new();

        for sentence in &sentences {
            let trimmed = sentence.trim();
            if trimmed.is_empty() {
                continue;
            }

            let phonemes = self.phonemize(trimmed)?;
            if phonemes.is_empty() {
                continue;
            }

            let ids = self.phonemes_to_ids(&phonemes);
            let samples = self.run_inference(&ids)?;
            all_samples.extend(samples);

            if sentences.len() > 1 {
                let silence_samples = (self.config.audio.sample_rate as f32 * 0.15) as usize;
                all_samples.extend(std::iter::repeat_n(0.0f32, silence_samples));
            }
        }

        Ok(AudioOutput {
            samples: all_samples,
            sample_rate: self.config.audio.sample_rate,
            channels: 1,
        })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn engine_kind(&self) -> EngineKind {
        EngineKind::Piper
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        // For Piper, the model IS the voice; the language is encoded in the id
        // (`sl_SI-artur-medium` -> `sl-SI`).
        let language = self
            .model_id
            .split_once('-')
            .map(|(l, _)| l.replace('_', "-"))
            .unwrap_or_else(|| "en".to_string());
        vec![VoiceInfo {
            id: self.model_id.clone(),
            name: self.model_id.clone(),
            language,
            description: "Piper voice".to_string(),
        }]
    }

    fn set_voice(&mut self, _voice_id: &str) -> Result<()> {
        bail!("Piper requires a full model reload to change voice. Use a different model ID.")
    }
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';') {
            sentences.push(current.clone());
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        sentences.push(current);
    }

    if sentences.is_empty() {
        sentences.push(text.to_string());
    }

    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_multi_speaker_config() {
        let json = r#"{
            "audio": {"sample_rate": 22050},
            "espeak": {"voice": "uk"},
            "inference": {"noise_scale": 0.667, "length_scale": 1, "noise_w": 0.8},
            "phoneme_type": "text",
            "num_speakers": 3,
            "phoneme_id_map": {"_": [0], "^": [1], "$": [2], "а": [5]}
        }"#;
        let cfg: PiperConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.uses_raw_text());
        assert!(cfg.is_multi_speaker());
        assert_eq!(cfg.espeak.as_ref().unwrap().voice, "uk");
    }

    #[test]
    fn espeak_config_defaults() {
        let json = r#"{
            "audio": {"sample_rate": 22050},
            "espeak": {"voice": "sl"},
            "inference": {"noise_scale": 0.667, "length_scale": 1, "noise_w": 0.8},
            "phoneme_id_map": {}
        }"#;
        let cfg: PiperConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.uses_raw_text());
        assert!(!cfg.is_multi_speaker());
    }

    #[test]
    fn splits_on_sentence_punctuation() {
        assert_eq!(
            split_sentences("Dober dan. Kako si?"),
            vec!["Dober dan.", " Kako si?"]
        );
        assert_eq!(split_sentences("no punctuation"), vec!["no punctuation"]);
    }
}
