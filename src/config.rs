use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::engine::EngineKind;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    pub default_voice: Option<String>,
    pub default_engine: Option<EngineKind>,
    pub default_model: Option<String>,
    pub output_dir: Option<String>,
    pub kokoro: Option<KokoroConfig>,
    pub chatterbox: Option<ChatterboxConfig>,
    pub supertonic: Option<SupertonicConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KokoroConfig {
    pub variant: Option<String>,
    pub speed: Option<f32>,
    pub default_voice: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatterboxConfig {
    pub quantized: Option<bool>,
    pub reference_audio: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SupertonicConfig {
    pub speed: Option<f32>,
    pub steps: Option<u32>,
    pub default_voice: Option<String>,
}

impl Config {
    pub fn dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("local-voice")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config from {}", path.display()))?;
            let mut config: Config =
                toml::from_str(&content).with_context(|| "Failed to parse config")?;

            // Migration: if default_voice is set but default_engine is not,
            // try to detect engine from the voice ID
            if config.default_engine.is_none() {
                if let Some(ref voice) = config.default_voice {
                    config.default_engine = detect_engine_from_voice(voice);
                }
            }

            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn models_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from(".local/share"))
            .join("local-voice")
            .join("models")
    }

    /// Model path scoped by engine: models/{engine}/{model_id}/
    pub fn model_path_for(engine: EngineKind, model_id: &str) -> PathBuf {
        Self::models_dir().join(engine.as_str()).join(model_id)
    }

    /// Legacy flat model path (for backward compat check)
    pub fn model_path_legacy(model_id: &str) -> PathBuf {
        Self::models_dir().join(model_id)
    }

    /// Check if a model is installed (checks engine-scoped path first, then legacy)
    pub fn is_model_installed(model_id: &str) -> bool {
        // Check engine-scoped paths
        for engine in EngineKind::all() {
            let dir = Self::model_path_for(*engine, model_id);
            if dir.exists() && has_model_files(&dir, *engine) {
                return true;
            }
        }
        // Legacy flat path (Piper models from old installs)
        let legacy = Self::model_path_legacy(model_id);
        legacy.join("model.onnx").exists()
    }

    /// Find which engine a model is installed under
    pub fn installed_engine_for(model_id: &str) -> Option<EngineKind> {
        for engine in EngineKind::all() {
            let dir = Self::model_path_for(*engine, model_id);
            if dir.exists() && has_model_files(&dir, *engine) {
                return Some(*engine);
            }
        }
        // Legacy Piper
        let legacy = Self::model_path_legacy(model_id);
        if legacy.join("model.onnx").exists() {
            return Some(EngineKind::Piper);
        }
        None
    }

    /// Get the actual path for an installed model (handles legacy paths)
    pub fn resolve_model_path(engine: EngineKind, model_id: &str) -> PathBuf {
        let scoped = Self::model_path_for(engine, model_id);
        if scoped.exists() {
            return scoped;
        }
        // Fallback to legacy
        Self::model_path_legacy(model_id)
    }

    /// List installed models, optionally filtered by engine
    pub fn installed_models(engine: Option<EngineKind>) -> Vec<String> {
        let mut models = Vec::new();

        let engines = match engine {
            Some(e) => vec![e],
            None => EngineKind::all().to_vec(),
        };

        for eng in engines {
            let dir = Self::models_dir().join(eng.as_str());
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if has_model_files(&entry.path(), eng) {
                        if let Some(name) = entry.file_name().to_str() {
                            models.push(name.to_string());
                        }
                    }
                }
            }
        }

        // Also check legacy flat layout for Piper
        if engine.is_none() || engine == Some(EngineKind::Piper) {
            let base = Self::models_dir();
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && path.join("model.onnx").exists() {
                        if let Some(name) = entry.file_name().to_str() {
                            // Skip engine subdirectories
                            if !matches!(name, "kokoro" | "piper" | "chatterbox" | "supertonic")
                                && !models.contains(&name.to_string())
                            {
                                models.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        models
    }

    /// Resolve which model to use for an engine: default_model (if it belongs to engine) > first installed
    pub fn resolve_model(&self, engine: EngineKind) -> Option<String> {
        // Check if default_model is set and belongs to this engine
        if let Some(ref model_id) = self.default_model {
            if Self::installed_engine_for(model_id) == Some(engine) {
                return Some(model_id.clone());
            }
        }
        // Fall back to first installed for this engine
        Self::installed_models(Some(engine)).into_iter().next()
    }

    /// Resolve which voice to use: explicit > config default > first installed
    pub fn resolve_voice(&self, explicit: Option<&str>) -> Option<String> {
        explicit
            .map(String::from)
            .or_else(|| self.default_voice.clone())
            .or_else(|| Self::installed_models(None).into_iter().next())
    }

    /// Kokoro speed setting
    pub fn kokoro_speed(&self) -> f32 {
        self.kokoro.as_ref().and_then(|k| k.speed).unwrap_or(1.0)
    }

    /// Kokoro default voice
    pub fn kokoro_voice(&self) -> &str {
        self.kokoro
            .as_ref()
            .and_then(|k| k.default_voice.as_deref())
            .unwrap_or("af_alloy")
    }

    /// Supertonic speed setting
    pub fn supertonic_speed(&self) -> f32 {
        self.supertonic
            .as_ref()
            .and_then(|s| s.speed)
            .unwrap_or(1.05)
    }

    /// Supertonic denoising steps
    pub fn supertonic_steps(&self) -> u32 {
        self.supertonic.as_ref().and_then(|s| s.steps).unwrap_or(5)
    }

    /// Supertonic default voice
    pub fn supertonic_voice(&self) -> &str {
        self.supertonic
            .as_ref()
            .and_then(|s| s.default_voice.as_deref())
            .unwrap_or("F1")
    }

    /// List installed voice files for an engine+model combo
    pub fn installed_voices(engine: EngineKind, model_id: &str) -> Vec<String> {
        let voices_dir = Self::resolve_model_path(engine, model_id).join("voices");
        let mut voices = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&voices_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(stem) = name
                        .strip_suffix(".bin")
                        .or_else(|| name.strip_suffix(".json"))
                    {
                        voices.push(stem.to_string());
                    }
                }
            }
        }
        voices
    }
}

fn has_model_files(dir: &std::path::Path, engine: EngineKind) -> bool {
    match engine {
        EngineKind::Piper => {
            dir.join("model.onnx").exists() && dir.join("model.onnx.json").exists()
        }
        EngineKind::Kokoro => dir.join("model.onnx").exists(),
        EngineKind::Chatterbox => {
            dir.join("conditional_decoder.onnx").exists()
                && dir.join("speech_encoder.onnx").exists()
                && (dir.join("language_model.onnx").exists()
                    || dir.join("language_model_q4.onnx").exists())
        }
        EngineKind::Supertonic => {
            dir.join("duration_predictor.onnx").exists()
                && dir.join("text_encoder.onnx").exists()
                && dir.join("vector_estimator.onnx").exists()
                && dir.join("vocoder.onnx").exists()
        }
    }
}

fn detect_engine_from_voice(voice: &str) -> Option<EngineKind> {
    if voice.starts_with("kokoro") {
        Some(EngineKind::Kokoro)
    } else if voice.starts_with("chatterbox") {
        Some(EngineKind::Chatterbox)
    } else if voice.starts_with("supertonic") {
        Some(EngineKind::Supertonic)
    } else {
        Some(EngineKind::Piper)
    }
}
