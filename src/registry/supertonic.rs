use super::{DownloadItem, EngineRegistry, ModelEntry, VoiceEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

const HF_BASE: &str = "https://huggingface.co/Supertone/supertonic/resolve/main";

pub struct SupertonicRegistry;

impl EngineRegistry for SupertonicRegistry {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Supertonic
    }

    fn list_models(&self, _language: Option<&str>) -> Vec<&'static ModelEntry> {
        MODELS.iter().collect()
    }

    fn find_model(&self, id: &str) -> Option<&'static ModelEntry> {
        MODELS.iter().find(|m| m.id == id)
    }

    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        self.find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Supertonic model: {model_id}"))?;

        let mut items = vec![
            // 4 ONNX models
            DownloadItem {
                url: format!("{HF_BASE}/onnx/duration_predictor.onnx"),
                dest_relative: PathBuf::from("duration_predictor.onnx"),
                size_hint_mb: Some(2),
            },
            DownloadItem {
                url: format!("{HF_BASE}/onnx/text_encoder.onnx"),
                dest_relative: PathBuf::from("text_encoder.onnx"),
                size_hint_mb: Some(26),
            },
            DownloadItem {
                url: format!("{HF_BASE}/onnx/vector_estimator.onnx"),
                dest_relative: PathBuf::from("vector_estimator.onnx"),
                size_hint_mb: Some(126),
            },
            DownloadItem {
                url: format!("{HF_BASE}/onnx/vocoder.onnx"),
                dest_relative: PathBuf::from("vocoder.onnx"),
                size_hint_mb: Some(97),
            },
            // Config files
            DownloadItem {
                url: format!("{HF_BASE}/onnx/tts.json"),
                dest_relative: PathBuf::from("tts.json"),
                size_hint_mb: Some(1),
            },
            DownloadItem {
                url: format!("{HF_BASE}/onnx/unicode_indexer.json"),
                dest_relative: PathBuf::from("unicode_indexer.json"),
                size_hint_mb: Some(1),
            },
        ];

        // Default voice (F1)
        items.push(DownloadItem {
            url: format!("{HF_BASE}/voice_styles/F1.json"),
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

    fn voice_download_plan(&self, voice_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        self.find_voice(voice_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Supertonic voice: {voice_id}"))?;
        Ok(vec![DownloadItem {
            url: format!("{HF_BASE}/voice_styles/{voice_id}.json"),
            dest_relative: PathBuf::from(format!("voices/{voice_id}.json")),
            size_hint_mb: Some(1),
        }])
    }
}

pub static MODELS: &[ModelEntry] = &[ModelEntry {
    id: "supertonic",
    engine: EngineKind::Supertonic,
    name: "Supertonic",
    language: "multi",
    quality: "high",
    description: "66M params, 167x realtime, multilingual — 263 MB",
    size_mb: 263,
    sample_rate: 44100,
}];

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
