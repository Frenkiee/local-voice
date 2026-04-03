pub mod kokoro;
pub mod piper;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Supported TTS engine types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Kokoro,
    Piper,
    Chatterbox,
}

impl EngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kokoro => "kokoro",
            Self::Piper => "piper",
            Self::Chatterbox => "chatterbox",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Kokoro => "82M params, near-human quality, CPU-friendly",
            Self::Piper => "Tiny models, fastest inference, runs anywhere",
            Self::Chatterbox => "500M params, voice cloning, best with GPU",
        }
    }

    pub fn all() -> &'static [EngineKind] {
        &[Self::Kokoro, Self::Piper, Self::Chatterbox]
    }
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EngineKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "kokoro" => Ok(Self::Kokoro),
            "piper" => Ok(Self::Piper),
            "chatterbox" => Ok(Self::Chatterbox),
            _ => anyhow::bail!("Unknown engine '{s}'. Valid engines: kokoro, piper, chatterbox"),
        }
    }
}

/// Audio output from TTS synthesis
pub struct AudioOutput {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Information about a voice available in an engine
#[derive(Debug, Clone)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub description: String,
}

/// TTS engine trait — implemented by each engine backend
pub trait TtsEngine: Send {
    /// Synthesize text into audio samples
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput>;

    /// Current model/voice identifier
    fn model_id(&self) -> &str;

    /// Engine type
    fn engine_kind(&self) -> EngineKind;

    /// List voices this engine can produce (with current model loaded)
    fn available_voices(&self) -> Vec<VoiceInfo>;

    /// Switch to a different voice without full reload (if supported)
    fn set_voice(&mut self, voice_id: &str) -> Result<()>;
}
