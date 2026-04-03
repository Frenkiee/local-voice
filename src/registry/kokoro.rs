use super::{DownloadItem, EngineRegistry, ModelEntry, VoiceEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

const HF_BASE: &str =
    "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main";

pub struct KokoroRegistry;

impl EngineRegistry for KokoroRegistry {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Kokoro
    }

    fn list_models(&self, _language: Option<&str>) -> Vec<&'static ModelEntry> {
        // Kokoro is multilingual — all variants support the same languages
        MODELS.iter().collect()
    }

    fn find_model(&self, id: &str) -> Option<&'static ModelEntry> {
        MODELS.iter().find(|m| m.id == id)
    }

    fn download_plan(&self, model_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        let entry = self
            .find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Kokoro model: {model_id}"))?;

        let variant = VARIANTS
            .iter()
            .find(|v| v.id == model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Kokoro variant: {model_id}"))?;

        let mut items = vec![
            // ONNX model
            DownloadItem {
                url: format!("{HF_BASE}/{}", variant.file),
                dest_relative: PathBuf::from("model.onnx"),
                size_hint_mb: Some(entry.size_mb),
            },
        ];

        // Default voice (af_alloy)
        items.push(DownloadItem {
            url: format!("{HF_BASE}/voices/af_alloy.bin"),
            dest_relative: PathBuf::from("voices/af_alloy.bin"),
            size_hint_mb: Some(1),
        });

        Ok(items)
    }
}

struct KokoroVariant {
    id: &'static str,
    file: &'static str,
}

static VARIANTS: &[KokoroVariant] = &[
    KokoroVariant {
        id: "kokoro-fp32",
        file: "onnx/model.onnx",
    },
    KokoroVariant {
        id: "kokoro-fp16",
        file: "onnx/model_fp16.onnx",
    },
    KokoroVariant {
        id: "kokoro-q8f16",
        file: "onnx/model_q8f16.onnx",
    },
    KokoroVariant {
        id: "kokoro-q4f16",
        file: "onnx/model_q4f16.onnx",
    },
];

pub static MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "kokoro-fp32",
        engine: EngineKind::Kokoro,
        name: "Kokoro FP32",
        language: "multi",
        quality: "high",
        description: "Full precision — best quality, 326 MB",
        size_mb: 326,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-fp16",
        engine: EngineKind::Kokoro,
        name: "Kokoro FP16",
        language: "multi",
        quality: "high",
        description: "Half precision — great quality, 163 MB",
        size_mb: 163,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-q8f16",
        engine: EngineKind::Kokoro,
        name: "Kokoro Q8F16",
        language: "multi",
        quality: "medium",
        description: "8-bit quantized — good quality, 114 MB",
        size_mb: 114,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-q4f16",
        engine: EngineKind::Kokoro,
        name: "Kokoro Q4F16",
        language: "multi",
        quality: "medium",
        description: "4-bit quantized — compact, 154 MB",
        size_mb: 154,
        sample_rate: 24000,
    },
];

pub static VOICES: &[VoiceEntry] = &[
    // American Female
    VoiceEntry {
        id: "af_alloy",
        name: "Alloy",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_aoede",
        name: "Aoede",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_bella",
        name: "Bella",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_jessica",
        name: "Jessica",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_kore",
        name: "Kore",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_nicole",
        name: "Nicole",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_nova",
        name: "Nova",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_river",
        name: "River",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_sarah",
        name: "Sarah",
        language: "en-US",
        gender: "F",
    },
    VoiceEntry {
        id: "af_sky",
        name: "Sky",
        language: "en-US",
        gender: "F",
    },
    // American Male
    VoiceEntry {
        id: "am_adam",
        name: "Adam",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_echo",
        name: "Echo",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_eric",
        name: "Eric",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_fenrir",
        name: "Fenrir",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_liam",
        name: "Liam",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_michael",
        name: "Michael",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_onyx",
        name: "Onyx",
        language: "en-US",
        gender: "M",
    },
    VoiceEntry {
        id: "am_puck",
        name: "Puck",
        language: "en-US",
        gender: "M",
    },
    // British Female
    VoiceEntry {
        id: "bf_alice",
        name: "Alice",
        language: "en-GB",
        gender: "F",
    },
    VoiceEntry {
        id: "bf_emma",
        name: "Emma",
        language: "en-GB",
        gender: "F",
    },
    VoiceEntry {
        id: "bf_isabella",
        name: "Isabella",
        language: "en-GB",
        gender: "F",
    },
    VoiceEntry {
        id: "bf_lily",
        name: "Lily",
        language: "en-GB",
        gender: "F",
    },
    // British Male
    VoiceEntry {
        id: "bm_daniel",
        name: "Daniel",
        language: "en-GB",
        gender: "M",
    },
    VoiceEntry {
        id: "bm_fable",
        name: "Fable",
        language: "en-GB",
        gender: "M",
    },
    VoiceEntry {
        id: "bm_george",
        name: "George",
        language: "en-GB",
        gender: "M",
    },
    VoiceEntry {
        id: "bm_lewis",
        name: "Lewis",
        language: "en-GB",
        gender: "M",
    },
];

/// Download URL for a specific Kokoro voice
pub fn voice_download_url(voice_id: &str) -> String {
    format!("{HF_BASE}/voices/{voice_id}.bin")
}
