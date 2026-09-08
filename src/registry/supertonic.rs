use super::{DownloadItem, EngineRegistry, ModelEntry, VoiceEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

/// Recommended Supertonic model (newest, most languages).
pub const RECOMMENDED_MODEL: &str = "supertonic-3";

/// Languages supported by the original English-only release.
pub const LANGS_V1: &[&str] = &["en"];

/// Languages supported by Supertonic 2.
pub const LANGS_V2: &[&str] = &["en", "ko", "es", "pt", "fr"];

/// Languages supported by Supertonic 3 (31 languages).
pub const LANGS_V3: &[&str] = &[
    "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu",
    "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi",
];

/// Per-model HuggingFace repo and file size hints (MB).
struct RepoInfo {
    base_url: &'static str,
    dp_mb: u32,
    text_enc_mb: u32,
    vector_est_mb: u32,
    vocoder_mb: u32,
}

fn repo_info(model_id: &str) -> Option<RepoInfo> {
    match model_id {
        "supertonic" => Some(RepoInfo {
            base_url: "https://huggingface.co/Supertone/supertonic/resolve/main",
            dp_mb: 2,
            text_enc_mb: 27,
            vector_est_mb: 132,
            vocoder_mb: 101,
        }),
        "supertonic-2" => Some(RepoInfo {
            base_url: "https://huggingface.co/Supertone/supertonic-2/resolve/main",
            dp_mb: 2,
            text_enc_mb: 27,
            vector_est_mb: 132,
            vocoder_mb: 101,
        }),
        "supertonic-3" => Some(RepoInfo {
            base_url: "https://huggingface.co/Supertone/supertonic-3/resolve/main",
            dp_mb: 4,
            text_enc_mb: 36,
            vector_est_mb: 257,
            vocoder_mb: 101,
        }),
        _ => None,
    }
}

/// HuggingFace base URL for a Supertonic model ID.
pub fn hf_base(model_id: &str) -> anyhow::Result<&'static str> {
    repo_info(model_id)
        .map(|r| r.base_url)
        .ok_or_else(|| anyhow::anyhow!("Unknown Supertonic model: {model_id}"))
}

/// Languages a Supertonic model can speak (ISO 639-1 codes), if known.
pub fn supported_languages(model_id: &str) -> Option<&'static [&'static str]> {
    match model_id {
        "supertonic-3" => Some(LANGS_V3),
        "supertonic-2" => Some(LANGS_V2),
        "supertonic" => Some(LANGS_V1),
        _ => None,
    }
}

/// Check that `lang` is a language code some Supertonic model can speak.
/// (Whether the *installed* model supports it is checked at engine load.)
pub fn validate_language(lang: &str) -> anyhow::Result<()> {
    let l = lang.trim().to_lowercase();
    if l == "na" || l == "auto" || LANGS_V3.contains(&l.as_str()) {
        Ok(())
    } else {
        anyhow::bail!(
            "Unknown language '{lang}'. Use 'auto' or one of: {}",
            LANGS_V3.join(", ")
        )
    }
}

pub struct SupertonicRegistry;

impl EngineRegistry for SupertonicRegistry {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Supertonic
    }

    fn list_models(&self, language: Option<&str>) -> Vec<&'static ModelEntry> {
        MODELS
            .iter()
            .filter(|m| {
                language
                    .map(|l| {
                        let l = l.to_lowercase();
                        supported_languages(m.id)
                            .unwrap_or(LANGS_V1)
                            .iter()
                            .any(|code| l.starts_with(code) || code.starts_with(&l))
                    })
                    .unwrap_or(true)
            })
            .collect()
    }

    fn find_model(&self, id: &str) -> Option<&'static ModelEntry> {
        MODELS.iter().find(|m| m.id == id)
    }

    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        self.find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Supertonic model: {model_id}"))?;
        let repo = repo_info(model_id).ok_or_else(|| {
            anyhow::anyhow!("No download source for Supertonic model: {model_id}")
        })?;
        let base = repo.base_url;

        let mut items = vec![
            // 4 ONNX models
            DownloadItem {
                url: format!("{base}/onnx/duration_predictor.onnx"),
                dest_relative: PathBuf::from("duration_predictor.onnx"),
                size_hint_mb: Some(repo.dp_mb),
            },
            DownloadItem {
                url: format!("{base}/onnx/text_encoder.onnx"),
                dest_relative: PathBuf::from("text_encoder.onnx"),
                size_hint_mb: Some(repo.text_enc_mb),
            },
            DownloadItem {
                url: format!("{base}/onnx/vector_estimator.onnx"),
                dest_relative: PathBuf::from("vector_estimator.onnx"),
                size_hint_mb: Some(repo.vector_est_mb),
            },
            DownloadItem {
                url: format!("{base}/onnx/vocoder.onnx"),
                dest_relative: PathBuf::from("vocoder.onnx"),
                size_hint_mb: Some(repo.vocoder_mb),
            },
            // Config files
            DownloadItem {
                url: format!("{base}/onnx/tts.json"),
                dest_relative: PathBuf::from("tts.json"),
                size_hint_mb: Some(1),
            },
            DownloadItem {
                url: format!("{base}/onnx/unicode_indexer.json"),
                dest_relative: PathBuf::from("unicode_indexer.json"),
                size_hint_mb: Some(1),
            },
        ];

        // Default voice (F1)
        items.push(DownloadItem {
            url: format!("{base}/voice_styles/F1.json"),
            dest_relative: PathBuf::from("voices/F1.json"),
            size_hint_mb: Some(1),
        });

        Ok(items)
    }

    fn list_voices(&self) -> Vec<&'static VoiceEntry> {
        VOICES.iter().collect()
    }

    fn find_voice(&self, voice_id: &str) -> Option<&'static VoiceEntry> {
        VOICES.iter().find(|v| v.id == voice_id)
    }

    /// Voice download plan for the recommended model. Voice style files differ
    /// per model, so callers that know the installed model should use
    /// [`voice_download_plan_for_model`].
    fn voice_download_plan(&self, voice_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        voice_download_plan_for_model(RECOMMENDED_MODEL, voice_id)
    }
}

/// Download plan for a voice style file from the HF repo of a specific model.
/// Style embeddings are model-specific: a v1 style will not load into v3.
pub fn voice_download_plan_for_model(
    model_id: &str,
    voice_id: &str,
) -> anyhow::Result<Vec<DownloadItem>> {
    VOICES
        .iter()
        .find(|v| v.id == voice_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown Supertonic voice: {voice_id}"))?;
    let base = hf_base(model_id)?;
    Ok(vec![DownloadItem {
        url: format!("{base}/voice_styles/{voice_id}.json"),
        dest_relative: PathBuf::from(format!("voices/{voice_id}.json")),
        size_hint_mb: Some(1),
    }])
}

pub static MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "supertonic-3",
        engine: EngineKind::Supertonic,
        name: "Supertonic 3",
        language: "multi",
        quality: "high",
        description: "99M params, 31 languages (en, de, sl, ja, ko, ...), expression tags — 399 MB (recommended)",
        size_mb: 399,
        sample_rate: 44100,
    },
    ModelEntry {
        id: "supertonic-2",
        engine: EngineKind::Supertonic,
        name: "Supertonic 2",
        language: "multi",
        quality: "high",
        description: "66M params, 5 languages (en, ko, es, pt, fr) — 264 MB",
        size_mb: 264,
        sample_rate: 44100,
    },
    ModelEntry {
        id: "supertonic",
        engine: EngineKind::Supertonic,
        name: "Supertonic",
        language: "en",
        quality: "high",
        description: "66M params, 167x realtime, English only — 263 MB",
        size_mb: 263,
        sample_rate: 44100,
    },
];

pub static VOICES: &[VoiceEntry] = &[
    VoiceEntry {
        id: "F1",
        name: "Female 1",
        language: "multi",
        gender: "F",
    },
    VoiceEntry {
        id: "F2",
        name: "Female 2",
        language: "multi",
        gender: "F",
    },
    VoiceEntry {
        id: "F3",
        name: "Female 3",
        language: "multi",
        gender: "F",
    },
    VoiceEntry {
        id: "F4",
        name: "Female 4",
        language: "multi",
        gender: "F",
    },
    VoiceEntry {
        id: "F5",
        name: "Female 5",
        language: "multi",
        gender: "F",
    },
    VoiceEntry {
        id: "M1",
        name: "Male 1",
        language: "multi",
        gender: "M",
    },
    VoiceEntry {
        id: "M2",
        name: "Male 2",
        language: "multi",
        gender: "M",
    },
    VoiceEntry {
        id: "M3",
        name: "Male 3",
        language: "multi",
        gender: "M",
    },
    VoiceEntry {
        id: "M4",
        name: "Male 4",
        language: "multi",
        gender: "M",
    },
    VoiceEntry {
        id: "M5",
        name: "Male 5",
        language: "multi",
        gender: "M",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_plan_uses_per_model_repo() {
        let reg = SupertonicRegistry;
        for (id, repo) in [
            ("supertonic", "Supertone/supertonic/"),
            ("supertonic-2", "Supertone/supertonic-2/"),
            ("supertonic-3", "Supertone/supertonic-3/"),
        ] {
            let plan = reg.download_plan(id).unwrap();
            assert_eq!(plan.len(), 7, "{id}");
            assert!(plan.iter().all(|i| i.url.contains(repo)), "{id}");
        }
        assert!(reg.download_plan("supertonic-9").is_err());
    }

    #[test]
    fn voice_plan_follows_model() {
        let plan = voice_download_plan_for_model("supertonic-3", "M2").unwrap();
        assert_eq!(
            plan[0].url,
            "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M2.json"
        );
        assert_eq!(plan[0].dest_relative, PathBuf::from("voices/M2.json"));
        let plan = voice_download_plan_for_model("supertonic", "F1").unwrap();
        assert!(plan[0].url.contains("Supertone/supertonic/"));
        assert!(voice_download_plan_for_model("supertonic-3", "X9").is_err());
    }

    #[test]
    fn language_filter_and_support() {
        let reg = SupertonicRegistry;
        let sl: Vec<_> = reg.list_models(Some("sl")).iter().map(|m| m.id).collect();
        assert_eq!(sl, vec!["supertonic-3"]);
        let en: Vec<_> = reg.list_models(Some("en")).iter().map(|m| m.id).collect();
        assert_eq!(en, vec!["supertonic-3", "supertonic-2", "supertonic"]);
        assert!(supported_languages("supertonic-3").unwrap().contains(&"sl"));
        assert!(!supported_languages("supertonic").unwrap().contains(&"sl"));
        assert_eq!(supported_languages("supertonic-3").unwrap().len(), 31);
        assert!(supported_languages("custom").is_none());
        assert!(validate_language("sl").is_ok());
        assert!(validate_language("SL").is_ok());
        assert!(validate_language("na").is_ok());
        assert!(validate_language("xx").is_err());
    }
}
