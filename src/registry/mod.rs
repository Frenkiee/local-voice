pub mod chatterbox;
pub mod kokoro;
pub mod piper;
pub mod supertonic;

use crate::engine::EngineKind;
use std::path::PathBuf;

/// A file to download as part of model installation
#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub url: String,
    pub dest_relative: PathBuf,
    pub size_hint_mb: Option<u32>,
}

/// A voice entry within a model
#[derive(Debug, Clone)]
pub struct VoiceEntry {
    pub id: &'static str,
    pub name: &'static str,
    #[allow(dead_code)]
    pub language: &'static str,
    pub gender: &'static str,
}

/// A model entry in the registry
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: &'static str,
    pub engine: EngineKind,
    pub name: &'static str,
    pub language: &'static str,
    pub quality: &'static str,
    pub description: &'static str,
    pub size_mb: u32,
    #[allow(dead_code)]
    pub sample_rate: u32,
}

/// Trait for per-engine model registries
pub trait EngineRegistry {
    fn engine_kind(&self) -> EngineKind;
    fn list_models(&self, language: Option<&str>) -> Vec<&'static ModelEntry>;
    fn find_model(&self, id: &str) -> Option<&'static ModelEntry>;
    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>>;

    /// List voices available for this engine (separate from models)
    fn list_voices(&self) -> Vec<&'static VoiceEntry> {
        vec![]
    }

    /// Find a voice by ID
    fn find_voice(&self, _voice_id: &str) -> Option<&'static VoiceEntry> {
        None
    }

    /// Download plan for a single voice file
    fn voice_download_plan(&self, _voice_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        anyhow::bail!("This engine does not support separate voice downloads")
    }
}

// ── Convenience functions for cross-engine lookups ──

/// Find a model in any engine's registry
pub fn find_model_any_engine(id: &str) -> Option<(EngineKind, &'static ModelEntry)> {
    for reg in all_registries() {
        if let Some(entry) = reg.find_model(id) {
            return Some((reg.engine_kind(), entry));
        }
    }
    None
}

/// Search all engines for models, optionally filtered by language
pub fn search_all(language: Option<&str>, engine: Option<EngineKind>) -> Vec<&'static ModelEntry> {
    let mut results = Vec::new();
    for reg in all_registries() {
        if let Some(e) = engine {
            if reg.engine_kind() != e {
                continue;
            }
        }
        results.extend(reg.list_models(language));
    }
    results
}

/// Get download plan for a model (auto-detects engine)
pub fn download_plan(model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
    for reg in all_registries() {
        if reg.find_model(model_id).is_some() {
            return reg.download_plan(model_id);
        }
    }
    anyhow::bail!(
        "Unknown model '{model_id}'. Run 'local-voice models list' to see available models."
    )
}

/// Get all registries
fn all_registries() -> Vec<Box<dyn EngineRegistry>> {
    vec![
        Box::new(kokoro::KokoroRegistry),
        Box::new(piper::PiperRegistry),
        Box::new(chatterbox::ChatterboxRegistry),
        Box::new(supertonic::SupertonicRegistry),
    ]
}

/// Get the registry for a specific engine
pub fn registry_for(engine: EngineKind) -> Box<dyn EngineRegistry> {
    match engine {
        EngineKind::Kokoro => Box::new(kokoro::KokoroRegistry),
        EngineKind::Piper => Box::new(piper::PiperRegistry),
        EngineKind::Chatterbox => Box::new(chatterbox::ChatterboxRegistry),
        EngineKind::Supertonic => Box::new(supertonic::SupertonicRegistry),
    }
}

/// List voices for a specific engine
pub fn voices_for_engine(engine: EngineKind) -> Vec<&'static VoiceEntry> {
    registry_for(engine).list_voices()
}

/// Find which engine a voice belongs to
pub fn find_voice_any_engine(voice_id: &str) -> Option<(EngineKind, &'static VoiceEntry)> {
    for reg in all_registries() {
        if let Some(entry) = reg.find_voice(voice_id) {
            return Some((reg.engine_kind(), entry));
        }
    }
    None
}

/// Get voice download plan (auto-detects engine)
pub fn voice_download_plan(voice_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
    for reg in all_registries() {
        if reg.find_voice(voice_id).is_some() {
            return reg.voice_download_plan(voice_id);
        }
    }
    anyhow::bail!(
        "Unknown voice '{voice_id}'. Run 'local-voice voices list' to see available voices."
    )
}
