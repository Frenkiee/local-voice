use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    /// Default voice/model ID for speak commands
    pub default_voice: Option<String>,
    /// Directory to save audio files
    pub output_dir: Option<String>,
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
            toml::from_str(&content).with_context(|| "Failed to parse config")
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

    pub fn model_path(model_id: &str) -> PathBuf {
        Self::models_dir().join(model_id)
    }

    pub fn is_model_installed(model_id: &str) -> bool {
        let dir = Self::model_path(model_id);
        dir.join("model.onnx").exists() && dir.join("model.onnx.json").exists()
    }

    pub fn installed_models() -> Vec<String> {
        let dir = Self::models_dir();
        if !dir.exists() {
            return vec![];
        }
        std::fs::read_dir(dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().join("model.onnx").exists())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve which voice to use: explicit > config default > first installed
    pub fn resolve_voice(&self, explicit: Option<&str>) -> Option<String> {
        explicit
            .map(String::from)
            .or_else(|| self.default_voice.clone())
            .or_else(|| Self::installed_models().into_iter().next())
    }
}
