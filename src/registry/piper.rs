use super::{DownloadItem, EngineRegistry, ModelEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

/// Pinned release tag: every model in `MODELS` was verified to exist here unless
/// it is listed in `MAIN_ONLY_MODELS`.
const HF_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0";
/// Moving `main` branch, used only for voices added after the `v1.0.0` tag.
const HF_BASE_MAIN: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main";

/// Models that 404 on the `v1.0.0` tag and are fetched from `main` instead.
///
/// `ja_JA-hi_fi_captain-medium` is deliberately absent: besides living only on
/// `main` (under the non-standard `ja_JA` path), it declares
/// `phoneme_type: "japanese"`, which needs a Japanese-specific phonemizer that
/// this espeak-ng based engine does not provide.
const MAIN_ONLY_MODELS: &[&str] = &["ko_KR-kss-medium"];

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
    let base = if MAIN_ONLY_MODELS.contains(&id) {
        HF_BASE_MAIN
    } else {
        HF_BASE
    };
    format!("{base}/{lang}/{lang_country}/{name}/{quality}/{lang_country}-{name}-{quality}.onnx")
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
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-lessac-high",
        engine: EngineKind::Piper,
        name: "Lessac HQ",
        language: "en-US",
        quality: "high",
        description: "Highest quality US English voice",
        size_mb: 114,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-amy-medium",
        engine: EngineKind::Piper,
        name: "Amy",
        language: "en-US",
        quality: "medium",
        description: "US English female voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-ryan-medium",
        engine: EngineKind::Piper,
        name: "Ryan",
        language: "en-US",
        quality: "medium",
        description: "US English male voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-arctic-medium",
        engine: EngineKind::Piper,
        name: "Arctic",
        language: "en-US",
        quality: "medium",
        description: "US English multi-speaker dataset voice",
        size_mb: 77,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-ljspeech-high",
        engine: EngineKind::Piper,
        name: "LJSpeech HQ",
        language: "en-US",
        quality: "high",
        description: "High-quality US English female voice (LJSpeech)",
        size_mb: 114,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-kristin-medium",
        engine: EngineKind::Piper,
        name: "Kristin",
        language: "en-US",
        quality: "medium",
        description: "US English female voice",
        size_mb: 64,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_US-bryce-medium",
        engine: EngineKind::Piper,
        name: "Bryce",
        language: "en-US",
        quality: "medium",
        description: "US English male voice",
        size_mb: 64,
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
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "en_GB-cori-medium",
        engine: EngineKind::Piper,
        name: "Cori",
        language: "en-GB",
        quality: "medium",
        description: "British English female voice",
        size_mb: 64,
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
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "de_DE-thorsten-high",
        engine: EngineKind::Piper,
        name: "Thorsten HQ",
        language: "de-DE",
        quality: "high",
        description: "High-quality German male voice",
        size_mb: 114,
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
        size_mb: 77,
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
        size_mb: 63,
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
        size_mb: 28,
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
        size_mb: 63,
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
        size_mb: 77,
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
        size_mb: 63,
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
        size_mb: 63,
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
        size_mb: 21,
        sample_rate: 16000,
    },
    ModelEntry {
        id: "uk_UA-ukrainian_tts-medium",
        engine: EngineKind::Piper,
        name: "Ukrainian TTS",
        language: "uk-UA",
        quality: "medium",
        description: "Ukrainian multi-speaker voice (raw-text phonemes, speaker 0 = lada)",
        size_mb: 77,
        sample_rate: 22050,
    },
    // Norwegian
    ModelEntry {
        id: "no_NO-talesyntese-medium",
        engine: EngineKind::Piper,
        name: "Talesyntese",
        language: "no-NO",
        quality: "medium",
        description: "Norwegian voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Slovenian
    ModelEntry {
        id: "sl_SI-artur-medium",
        engine: EngineKind::Piper,
        name: "Artur",
        language: "sl-SI",
        quality: "medium",
        description: "Slovenian male voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Polish
    ModelEntry {
        id: "pl_PL-gosia-medium",
        engine: EngineKind::Piper,
        name: "Gosia",
        language: "pl-PL",
        quality: "medium",
        description: "Polish female voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    ModelEntry {
        id: "pl_PL-darkman-medium",
        engine: EngineKind::Piper,
        name: "Darkman",
        language: "pl-PL",
        quality: "medium",
        description: "Polish male voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Czech
    ModelEntry {
        id: "cs_CZ-jirka-medium",
        engine: EngineKind::Piper,
        name: "Jirka",
        language: "cs-CZ",
        quality: "medium",
        description: "Czech male voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Swedish
    ModelEntry {
        id: "sv_SE-nst-medium",
        engine: EngineKind::Piper,
        name: "NST",
        language: "sv-SE",
        quality: "medium",
        description: "Swedish voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Danish
    ModelEntry {
        id: "da_DK-talesyntese-medium",
        engine: EngineKind::Piper,
        name: "Talesyntese",
        language: "da-DK",
        quality: "medium",
        description: "Danish voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Turkish
    ModelEntry {
        id: "tr_TR-dfki-medium",
        engine: EngineKind::Piper,
        name: "DFKI",
        language: "tr-TR",
        quality: "medium",
        description: "Turkish voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Arabic
    ModelEntry {
        id: "ar_JO-kareem-medium",
        engine: EngineKind::Piper,
        name: "Kareem",
        language: "ar-JO",
        quality: "medium",
        description: "Arabic (Jordan) male voice",
        size_mb: 63,
        sample_rate: 22050,
    },
    // Hindi
    ModelEntry {
        id: "hi_IN-pratham-medium",
        engine: EngineKind::Piper,
        name: "Pratham",
        language: "hi-IN",
        quality: "medium",
        description: "Hindi male voice",
        size_mb: 64,
        sample_rate: 22050,
    },
    // Korean (fetched from the `main` branch, see MAIN_ONLY_MODELS)
    ModelEntry {
        id: "ko_KR-kss-medium",
        engine: EngineKind::Piper,
        name: "KSS",
        language: "ko-KR",
        quality: "medium",
        description: "Korean female voice",
        size_mb: 63,
        sample_rate: 22050,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_builder_derives_hf_path_from_id() {
        assert_eq!(
            piper_onnx_url("sl_SI-artur-medium"),
            format!("{HF_BASE}/sl/sl_SI/artur/medium/sl_SI-artur-medium.onnx")
        );
        assert_eq!(
            piper_onnx_url("uk_UA-ukrainian_tts-medium"),
            format!("{HF_BASE}/uk/uk_UA/ukrainian_tts/medium/uk_UA-ukrainian_tts-medium.onnx")
        );
        assert_eq!(
            piper_onnx_url("it_IT-riccardo-x_low"),
            format!("{HF_BASE}/it/it_IT/riccardo/x_low/it_IT-riccardo-x_low.onnx")
        );
    }

    #[test]
    fn main_only_models_use_main_branch() {
        assert_eq!(
            piper_onnx_url("ko_KR-kss-medium"),
            format!("{HF_BASE_MAIN}/ko/ko_KR/kss/medium/ko_KR-kss-medium.onnx")
        );
        for id in MAIN_ONLY_MODELS {
            assert!(MODELS.iter().any(|m| m.id == *id), "{id} not in MODELS");
        }
    }

    #[test]
    fn model_ids_are_unique_and_match_language() {
        let mut seen = std::collections::HashSet::new();
        for m in MODELS {
            assert!(seen.insert(m.id), "duplicate id {}", m.id);
            let (lang_region, _) = m.id.split_once('-').unwrap();
            assert_eq!(
                lang_region.replace('_', "-"),
                m.language,
                "language mismatch for {}",
                m.id
            );
        }
    }
}
