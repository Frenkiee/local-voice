use super::{DownloadItem, EngineRegistry, ModelEntry, VoiceEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

const HF_BASE: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main";

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

    fn list_voices(&self) -> Vec<&'static VoiceEntry> {
        VOICES.iter().collect()
    }

    fn find_voice(&self, voice_id: &str) -> Option<&'static VoiceEntry> {
        VOICES.iter().find(|v| v.id == voice_id)
    }

    fn voice_download_plan(&self, voice_id: &str) -> anyhow::Result<Vec<DownloadItem>> {
        self.find_voice(voice_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Kokoro voice: {voice_id}"))?;
        Ok(vec![DownloadItem {
            url: voice_download_url(voice_id),
            dest_relative: PathBuf::from(format!("voices/{voice_id}.bin")),
            size_hint_mb: Some(1),
        }])
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
    KokoroVariant {
        id: "kokoro-uint8f16",
        file: "onnx/model_uint8f16.onnx",
    },
    KokoroVariant {
        id: "kokoro-quantized",
        file: "onnx/model_quantized.onnx",
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
        description: "8-bit weights / fp16 activations — good quality, 86 MB",
        size_mb: 86,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-q4f16",
        engine: EngineKind::Kokoro,
        name: "Kokoro Q4F16",
        language: "multi",
        quality: "medium",
        description: "4-bit quantized — 155 MB",
        size_mb: 155,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-uint8f16",
        engine: EngineKind::Kokoro,
        name: "Kokoro UINT8F16",
        language: "multi",
        quality: "medium",
        description: "uint8 weights / fp16 activations — 114 MB",
        size_mb: 114,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "kokoro-quantized",
        engine: EngineKind::Kokoro,
        name: "Kokoro INT8",
        language: "multi",
        quality: "medium",
        description: "Fully int8 quantized — 92 MB",
        size_mb: 92,
        sample_rate: 24000,
    },
];

/// All 54 voices shipped in onnx-community/Kokoro-82M-v1.0-ONNX (each `.bin` is 522,240 bytes).
/// The first letter of the ID selects the language (see `engine::kokoro::espeak_voice_for`).
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
        id: "af_heart",
        name: "Heart",
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
    VoiceEntry {
        id: "am_santa",
        name: "Santa",
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
    // Spanish
    VoiceEntry {
        id: "ef_dora",
        name: "Dora",
        language: "es",
        gender: "F",
    },
    VoiceEntry {
        id: "em_alex",
        name: "Alex",
        language: "es",
        gender: "M",
    },
    VoiceEntry {
        id: "em_santa",
        name: "Santa",
        language: "es",
        gender: "M",
    },
    // French
    VoiceEntry {
        id: "ff_siwis",
        name: "Siwis",
        language: "fr-FR",
        gender: "F",
    },
    // Hindi
    VoiceEntry {
        id: "hf_alpha",
        name: "Alpha",
        language: "hi",
        gender: "F",
    },
    VoiceEntry {
        id: "hf_beta",
        name: "Beta",
        language: "hi",
        gender: "F",
    },
    VoiceEntry {
        id: "hm_omega",
        name: "Omega",
        language: "hi",
        gender: "M",
    },
    VoiceEntry {
        id: "hm_psi",
        name: "Psi",
        language: "hi",
        gender: "M",
    },
    // Italian
    VoiceEntry {
        id: "if_sara",
        name: "Sara",
        language: "it",
        gender: "F",
    },
    VoiceEntry {
        id: "im_nicola",
        name: "Nicola",
        language: "it",
        gender: "M",
    },
    // Brazilian Portuguese
    VoiceEntry {
        id: "pf_dora",
        name: "Dora",
        language: "pt-BR",
        gender: "F",
    },
    VoiceEntry {
        id: "pm_alex",
        name: "Alex",
        language: "pt-BR",
        gender: "M",
    },
    VoiceEntry {
        id: "pm_santa",
        name: "Santa",
        language: "pt-BR",
        gender: "M",
    },
    // Japanese — experimental: upstream Kokoro phonemizes ja/zh with misaki, we
    // use espeak-ng (`ja` / `cmn`), so pronunciation quality is lower.
    VoiceEntry {
        id: "jf_alpha",
        name: "Alpha",
        language: "ja",
        gender: "F",
    },
    VoiceEntry {
        id: "jf_gongitsune",
        name: "Gongitsune",
        language: "ja",
        gender: "F",
    },
    VoiceEntry {
        id: "jf_nezumi",
        name: "Nezumi",
        language: "ja",
        gender: "F",
    },
    VoiceEntry {
        id: "jf_tebukuro",
        name: "Tebukuro",
        language: "ja",
        gender: "F",
    },
    VoiceEntry {
        id: "jm_kumo",
        name: "Kumo",
        language: "ja",
        gender: "M",
    },
    // Mandarin Chinese — experimental (see Japanese note above)
    VoiceEntry {
        id: "zf_xiaobei",
        name: "Xiaobei",
        language: "zh",
        gender: "F",
    },
    VoiceEntry {
        id: "zf_xiaoni",
        name: "Xiaoni",
        language: "zh",
        gender: "F",
    },
    VoiceEntry {
        id: "zf_xiaoxiao",
        name: "Xiaoxiao",
        language: "zh",
        gender: "F",
    },
    VoiceEntry {
        id: "zf_xiaoyi",
        name: "Xiaoyi",
        language: "zh",
        gender: "F",
    },
    VoiceEntry {
        id: "zm_yunjian",
        name: "Yunjian",
        language: "zh",
        gender: "M",
    },
    VoiceEntry {
        id: "zm_yunxi",
        name: "Yunxi",
        language: "zh",
        gender: "M",
    },
    VoiceEntry {
        id: "zm_yunxia",
        name: "Yunxia",
        language: "zh",
        gender: "M",
    },
    VoiceEntry {
        id: "zm_yunyang",
        name: "Yunyang",
        language: "zh",
        gender: "M",
    },
];

/// Download URL for a specific Kokoro voice
pub fn voice_download_url(voice_id: &str) -> String {
    format!("{HF_BASE}/voices/{voice_id}.bin")
}
