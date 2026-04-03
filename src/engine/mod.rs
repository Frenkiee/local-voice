pub mod piper;

use anyhow::Result;

/// Audio output from TTS synthesis
pub struct AudioOutput {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// TTS engine trait
pub trait TtsEngine: Send {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput>;
    fn model_id(&self) -> &str;
}
