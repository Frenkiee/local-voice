use super::{DownloadItem, EngineRegistry, ModelEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

const HF_BASE: &str = "https://huggingface.co/onnx-community/chatterbox-ONNX/resolve/main";

pub struct ChatterboxRegistry;

impl EngineRegistry for ChatterboxRegistry {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Chatterbox
    }

    fn list_models(&self, _language: Option<&str>) -> Vec<&'static ModelEntry> {
        MODELS.iter().collect()
    }

    fn find_model(&self, id: &str) -> Option<&'static ModelEntry> {
        MODELS.iter().find(|m| m.id == id)
    }

    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        let _entry = self
            .find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Chatterbox model: {model_id}"))?;

        let lm_file = if model_id == "chatterbox-full" {
            "language_model.onnx"
        } else {
            "language_model_q4.onnx"
        };

        let lm_size = if model_id == "chatterbox-full" {
            2048
        } else {
            350
        };

        Ok(vec![
            DownloadItem {
                url: format!("{HF_BASE}/{lm_file}"),
                dest_relative: PathBuf::from(lm_file),
                size_hint_mb: Some(lm_size),
            },
            DownloadItem {
                url: format!("{HF_BASE}/decoder.onnx"),
                dest_relative: PathBuf::from("decoder.onnx"),
                size_hint_mb: Some(50),
            },
            DownloadItem {
                url: format!("{HF_BASE}/ve.onnx"),
                dest_relative: PathBuf::from("ve.onnx"),
                size_hint_mb: Some(30),
            },
            DownloadItem {
                url: format!("{HF_BASE}/s2a.onnx"),
                dest_relative: PathBuf::from("s2a.onnx"),
                size_hint_mb: Some(100),
            },
            DownloadItem {
                url: format!("{HF_BASE}/t3_cfg_rng.onnx"),
                dest_relative: PathBuf::from("t3_cfg_rng.onnx"),
                size_hint_mb: Some(50),
            },
        ])
    }
}

pub static MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "chatterbox-full",
        engine: EngineKind::Chatterbox,
        name: "Chatterbox Full",
        language: "en",
        quality: "high",
        description: "Full precision — best quality, voice cloning, ~2.3 GB",
        size_mb: 2300,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "chatterbox-quantized",
        engine: EngineKind::Chatterbox,
        name: "Chatterbox Q4",
        language: "en",
        quality: "medium",
        description: "4-bit quantized — good quality, voice cloning, ~580 MB",
        size_mb: 580,
        sample_rate: 24000,
    },
];
