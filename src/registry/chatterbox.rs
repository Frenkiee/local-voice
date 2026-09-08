use super::{DownloadItem, EngineRegistry, ModelEntry};
use crate::engine::EngineKind;
use std::path::PathBuf;

/// Original Chatterbox (500M, Llama-style LM, 10-step mel decoder).
const HF_BASE: &str = "https://huggingface.co/onnx-community/chatterbox-ONNX/resolve/main";

/// Chatterbox Turbo (350M, GPT-2-style LM, distilled 1-step mel decoder,
/// paralinguistic tags such as `[laugh]`). Same four-session layout as the
/// original export; the engine detects the variant from the ONNX graph inputs.
const HF_TURBO_BASE: &str = "https://huggingface.co/ResembleAI/chatterbox-turbo-ONNX/resolve/main";

pub struct ChatterboxRegistry;

/// A model component is a tiny `.onnx` graph stub plus a large `.onnx_data`
/// weight file. The stub records the *upstream* data filename internally, so
/// the data file must keep its upstream name on disk, while the stub itself may
/// be saved under any name (`dest_stub`). Sizes are MiB, HEAD-checked against
/// Hugging Face `x-linked-size` (Sept 2026).
fn component(
    base: &str,
    upstream: &str,
    dest_stub: &str,
    stub_mb: u32,
    data_mb: u32,
) -> [DownloadItem; 2] {
    [
        DownloadItem {
            url: format!("{base}/onnx/{upstream}.onnx"),
            dest_relative: PathBuf::from(format!("{dest_stub}.onnx")),
            size_hint_mb: Some(stub_mb),
        },
        DownloadItem {
            url: format!("{base}/onnx/{upstream}.onnx_data"),
            dest_relative: PathBuf::from(format!("{upstream}.onnx_data")),
            size_hint_mb: Some(data_mb),
        },
    ]
}

fn small(url: String, dest: &str, size_mb: u32) -> DownloadItem {
    DownloadItem {
        url,
        dest_relative: PathBuf::from(dest),
        size_hint_mb: Some(size_mb),
    }
}

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

        let mut plan: Vec<DownloadItem> = Vec::with_capacity(10);

        match model_id {
            "chatterbox-turbo" => {
                // Q4 tier of the Turbo export. Stubs are saved under the
                // canonical component names so the installed-model detection
                // and the engine's file probing work unchanged.
                plan.extend(component(
                    HF_TURBO_BASE,
                    "language_model_q4",
                    "language_model_q4",
                    1,
                    195,
                ));
                plan.extend(component(
                    HF_TURBO_BASE,
                    "speech_encoder_q4",
                    "speech_encoder",
                    2,
                    219,
                ));
                plan.extend(component(
                    HF_TURBO_BASE,
                    "embed_tokens_q4",
                    "embed_tokens",
                    1,
                    36,
                ));
                plan.extend(component(
                    HF_TURBO_BASE,
                    "conditional_decoder_q4",
                    "conditional_decoder",
                    3,
                    235,
                ));
                plan.push(small(
                    format!("{HF_TURBO_BASE}/tokenizer.json"),
                    "tokenizer.json",
                    4,
                ));
                // The Turbo repo ships no reference voice; the original
                // Chatterbox one is a plain 24 kHz WAV and works for both.
                plan.push(small(
                    format!("{HF_BASE}/default_voice.wav"),
                    "default_voice.wav",
                    1,
                ));
            }
            _ => {
                let (lm_file, lm_data_mb) = if model_id == "chatterbox-full" {
                    ("language_model", 1985)
                } else {
                    ("language_model_q4", 338)
                };
                plan.extend(component(HF_BASE, lm_file, lm_file, 1, lm_data_mb));
                plan.extend(component(
                    HF_BASE,
                    "speech_encoder",
                    "speech_encoder",
                    2,
                    564,
                ));
                plan.extend(component(HF_BASE, "embed_tokens", "embed_tokens", 1, 59));
                plan.extend(component(
                    HF_BASE,
                    "conditional_decoder",
                    "conditional_decoder",
                    7,
                    510,
                ));
                plan.push(small(
                    format!("{HF_BASE}/tokenizer.json"),
                    "tokenizer.json",
                    1,
                ));
                plan.push(small(
                    format!("{HF_BASE}/default_voice.wav"),
                    "default_voice.wav",
                    1,
                ));
            }
        }

        Ok(plan)
    }
}

// `size_mb` is the sum of the per-file hints above (MiB).
//
// Not offered: the `*_fp16` / `*_q4f16` tiers of either repo. Their KV caches
// are float16 tensors and the engine only handles float32 caches; adding them
// needs f16 support in the engine, not just a registry entry.
pub static MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "chatterbox-turbo",
        engine: EngineKind::Chatterbox,
        name: "Chatterbox Turbo Q4",
        language: "en",
        quality: "high",
        description: "Turbo 350M, 1-step decoder, [laugh]/[chuckle] tags, voice cloning, ~700 MB",
        size_mb: 697,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "chatterbox-full",
        engine: EngineKind::Chatterbox,
        name: "Chatterbox Full",
        language: "en",
        quality: "high",
        description: "Full precision — best quality, voice cloning, ~3.1 GB",
        size_mb: 3131,
        sample_rate: 24000,
    },
    ModelEntry {
        id: "chatterbox-quantized",
        engine: EngineKind::Chatterbox,
        name: "Chatterbox Q4",
        language: "en",
        quality: "medium",
        description: "4-bit language model — good quality, voice cloning, ~1.5 GB",
        size_mb: 1484,
        sample_rate: 24000,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_total(id: &str) -> u32 {
        ChatterboxRegistry
            .download_plan(id)
            .unwrap()
            .iter()
            .map(|i| i.size_hint_mb.unwrap_or(0))
            .sum()
    }

    #[test]
    fn size_totals_match_download_plans() {
        for m in MODELS {
            assert_eq!(m.size_mb, plan_total(m.id), "size_mb drift for {}", m.id);
        }
    }

    #[test]
    fn data_files_keep_upstream_names() {
        for m in MODELS {
            for item in ChatterboxRegistry.download_plan(m.id).unwrap() {
                let dest = item.dest_relative.to_string_lossy().into_owned();
                if dest.ends_with(".onnx_data") {
                    assert!(item.url.ends_with(&format!("/{dest}")), "{}: {dest}", m.id);
                }
            }
        }
    }

    #[test]
    fn plans_contain_files_the_installed_check_expects() {
        for m in MODELS {
            let dests: Vec<String> = ChatterboxRegistry
                .download_plan(m.id)
                .unwrap()
                .into_iter()
                .map(|i| i.dest_relative.to_string_lossy().into_owned())
                .collect();
            for required in [
                "speech_encoder.onnx",
                "embed_tokens.onnx",
                "conditional_decoder.onnx",
                "tokenizer.json",
                "default_voice.wav",
            ] {
                assert!(dests.iter().any(|d| d == required), "{}: {required}", m.id);
            }
            assert!(
                dests
                    .iter()
                    .any(|d| d == "language_model.onnx" || d == "language_model_q4.onnx"),
                "{}: language model stub",
                m.id
            );
        }
    }
}
