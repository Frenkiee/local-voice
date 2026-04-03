pub mod chatterbox;
pub mod kokoro;
pub mod piper;

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
    pub sample_rate: u32,
}

/// Trait for per-engine model registries
pub trait EngineRegistry {
    fn engine_kind(&self) -> EngineKind;
    fn list_models(&self, language: Option<&str>) -> Vec<&'static ModelEntry>;
    fn find_model(&self, id: &str) -> Option<&'static ModelEntry>;
    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>>;
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
    anyhow::bail!("Unknown model '{model_id}'. Run 'local-voice models list' to see available models.")
}

/// Get all registries
fn all_registries() -> Vec<Box<dyn EngineRegistry>> {
    vec![
        Box::new(kokoro::KokoroRegistry),
        Box::new(piper::PiperRegistry),
        Box::new(chatterbox::ChatterboxRegistry),
    ]
}

/// Get the registry for a specific engine
pub fn registry_for(engine: EngineKind) -> Box<dyn EngineRegistry> {
    match engine {
        EngineKind::Kokoro => Box::new(kokoro::KokoroRegistry),
        EngineKind::Piper => Box::new(piper::PiperRegistry),
        EngineKind::Chatterbox => Box::new(chatterbox::ChatterboxRegistry),
    }
}

/// List voices for a specific engine (Kokoro has separate voices from models)
pub fn voices_for_engine(engine: EngineKind) -> Vec<&'static VoiceEntry> {
    match engine {
        EngineKind::Kokoro => kokoro::VOICES.iter().collect(),
        _ => vec![],
    }
}
