use super::{DownloadItem, EngineRegistry, ModelEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

const HF_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0";

pub struct PiperRegistry;

impl EngineRegistry for PiperRegistry {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Piper
    }

    fn list_models(&self, language: Option<&str>) -> Vec<&'static ModelEntry> {
        MODELS
            .iter()
            .filter(|m| {
                language
                    .map(|l| {
                        m.language.to_lowercase().starts_with(&l.to_lowercase())
                            || m.id.to_lowercase().starts_with(&l.to_lowercase())
                    })
                    .unwrap_or(true)
            })
            .collect()
    }

    fn find_model(&self, id: &str) -> Option<&'static ModelEntry> {
        MODELS.iter().find(|m| m.id == id)
    }

    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        let entry = self
            .find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Piper model: {model_id}"))?;

        let onnx_url = piper_onnx_url(entry.id);
        let config_url = format!("{}.json", onnx_url);

        Ok(vec![
            DownloadItem {
                url: onnx_url,
                dest_relative: PathBuf::from("model.onnx"),
                size_hint_mb: Some(entry.size_mb),
            },
            DownloadItem {
                url: config_url,
                dest_relative: PathBuf::from("model.onnx.json"),
                size_hint_mb: Some(1),
            },
        ])
    }
}

fn piper_onnx_url(id: &str) -> String {
    let (lang, rest) = id.split_once('_').unwrap_or(("en", id));
    let (country_name, quality) = rest.rsplit_once('-').unwrap_or((rest, "medium"));
    let (country, name) = country_name
        .split_once('-')
        .unwrap_or((country_name, "unknown"));
    let lang_country = format!("{lang}_{country}");
    format!("{HF_BASE}/{lang}/{lang_country}/{name}/{quality}/{lang_country}-{name}-{quality}.onnx")
}

pub static MODELS: &[ModelEntry] = &[
    // English — US
    ModelEntry {
        id: "en_US-lessac-medium",
        engine: EngineKind::Piper,
        name: "Lessac",
        language: "en-US",
        quality: "medium",
        description: "High-quality US English, balanced speed/quality",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-lessac-high",
        engine: EngineKind::Piper,
        name: "Lessac HQ",
        language: "en-US",
        quality: "high",
        description: "Highest quality US English voice",
        size_mb: 100,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-amy-medium",
        engine: EngineKind::Piper,
        name: "Amy",
        language: "en-US",
        quality: "medium",
        description: "US English female voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-ryan-medium",
        engine: EngineKind::Piper,
        name: "Ryan",
        language: "en-US",
        quality: "medium",
        description: "US English male voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-arctic-medium",
        engine: EngineKind::Piper,
        name: "Arctic",
        language: "en-US",
        quality: "medium",
        description: "US English multi-speaker dataset voice",
        size_mb: 65,
        sample_rate: 22050,
    },
    // English — GB
    ModelEntry {
        id: "en_GB-alan-medium",
        engine: EngineKind::Piper,
        name: "Alan",
        language: "en-GB",
        quality: "medium",
        description: "British English male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_GB-cori-medium",
        engine: EngineKind::Piper,
        name: "Cori",
        language: "en-GB",
        quality: "medium",
        description: "British English female voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // German
    ModelEntry {
        id: "de_DE-thorsten-medium",
        engine: EngineKind::Piper,
        name: "Thorsten",
        language: "de-DE",
        quality: "medium",
        description: "German male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "de_DE-thorsten-high",
        engine: EngineKind::Piper,
        name: "Thorsten HQ",
        language: "de-DE",
        quality: "high",
        description: "High-quality German male voice",
        size_mb: 90,
        sample_rate: 22050,
    },
    // French
    ModelEntry {
        id: "fr_FR-upmc-medium",
        engine: EngineKind::Piper,
        name: "UPMC",
        language: "fr-FR",
        quality: "medium",
        description: "French voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Spanish
    ModelEntry {
        id: "es_ES-davefx-medium",
        engine: EngineKind::Piper,
        name: "DaveFX",
        language: "es-ES",
        quality: "medium",
        description: "Spanish male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Italian
    ModelEntry {
        id: "it_IT-riccardo-x_low",
        engine: EngineKind::Piper,
        name: "Riccardo",
        language: "it-IT",
        quality: "x_low",
        description: "Italian male voice (compact)",
        size_mb: 18,
        sample_rate: 16000,
    },
    // Portuguese
    ModelEntry {
        id: "pt_BR-faber-medium",
        engine: EngineKind::Piper,
        name: "Faber",
        language: "pt-BR",
        quality: "medium",
        description: "Brazilian Portuguese male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Dutch
    ModelEntry {
        id: "nl_NL-mls-medium",
        engine: EngineKind::Piper,
        name: "MLS",
        language: "nl-NL",
        quality: "medium",
        description: "Dutch voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Russian
    ModelEntry {
        id: "ru_RU-denis-medium",
        engine: EngineKind::Piper,
        name: "Denis",
        language: "ru-RU",
        quality: "medium",
        description: "Russian male voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Chinese
    ModelEntry {
        id: "zh_CN-huayan-medium",
        engine: EngineKind::Piper,
        name: "Huayan",
        language: "zh-CN",
        quality: "medium",
        description: "Mandarin Chinese female voice",
        size_mb: 55,
        sample_rate: 22050,
    },
    // Ukrainian
    ModelEntry {
        id: "uk_UA-lada-x_low",
        engine: EngineKind::Piper,
        name: "Lada",
        language: "uk-UA",
        quality: "x_low",
        description: "Ukrainian female voice (compact)",
        size_mb: 18,
        sample_rate: 16000,
    },
    // Norwegian
    ModelEntry {
        id: "no_NO-talesyntese-medium",
        engine: EngineKind::Piper,
        name: "Talesyntese",
        language: "no-NO",
        quality: "medium",
        description: "Norwegian voice",
        size_mb: 55,
        sample_rate: 22050,
    },
];
