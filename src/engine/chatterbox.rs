//! Chatterbox inference over four ONNX sessions (speech encoder, token
//! embedder, autoregressive language model, conditional mel/vocoder decoder).
//!
//! Two upstream exports are supported and detected from the ONNX graph inputs
//! rather than from the model id:
//!
//! * **Original** (`onnx-community/chatterbox-ONNX`): Llama-style LM with 30
//!   layers and no `position_ids` input; `embed_tokens` takes `input_ids`,
//!   `position_ids` and an `exaggeration` scalar; the tokenizer's
//!   post-processor wraps the text as `[EXAGGERATION] [START] … [STOP]
//!   [START_SPEECH]`.
//! * **Turbo** (`ResembleAI/chatterbox-turbo-ONNX`): GPT-2-style LM with 24
//!   layers and an explicit `position_ids` input; `embed_tokens` takes only
//!   `input_ids`; the GPT-2 tokenizer appends two `<|endoftext|>` tokens; the
//!   decoder wants three trailing `SILENCE_TOKEN`s. Paralinguistic tags such
//!   as `[laugh]` are ordinary added tokens in its vocabulary.
//!
//! Both share the speech-encoder and conditional-decoder input/output names,
//! the `past_key_values.{layer}.{key,value}` / `present.{layer}.{key,value}`
//! cache naming, the speech token ids, and 24 kHz output.

use anyhow::{Context, Result, bail};
use std::path::Path;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use ort::memory::Allocator;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor, Value};

const S3GEN_SR: u32 = 24000;
const START_SPEECH_TOKEN: i64 = 6561;
const STOP_SPEECH_TOKEN: i64 = 6562;
/// Turbo only: appended (x3) after the generated tokens before decoding.
const SILENCE_TOKEN: i64 = 4299;
const TURBO_TRAILING_SILENCE: usize = 3;
/// Both exports use 16 KV heads of 64 dims (hidden 1024).
const NUM_KV_HEADS: usize = 16;
const HEAD_DIM: usize = 64;
/// Upstream reference uses 1024 for Turbo; ~25 speech tokens per second.
const MAX_NEW_TOKENS: usize = 1024;
const REPETITION_PENALTY: f32 = 1.2;

/// Which export family is loaded. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Original,
    Turbo,
}

pub struct ChatterboxEngine {
    speech_encoder: Session,
    embed_tokens: Session,
    language_model: Session,
    cond_decoder: Session,
    tokenizer: tokenizers::Tokenizer,
    reference_audio: Vec<f32>,
    model_id: String,
    exaggeration: f32,
    variant: Variant,
    num_layers: usize,
}

impl ChatterboxEngine {
    pub fn load(model_dir: &Path, model_id: &str) -> Result<Self> {
        let speech_encoder = load_component(model_dir, "speech_encoder")?;
        let embed_tokens = load_component(model_dir, "embed_tokens")?;
        let language_model = load_component(model_dir, "language_model")?;
        let cond_decoder = load_component(model_dir, "conditional_decoder")?;

        let lm_inputs = input_names(&language_model);
        let embed_inputs = input_names(&embed_tokens);

        let variant = if lm_inputs.iter().any(|n| n == "position_ids") {
            Variant::Turbo
        } else {
            Variant::Original
        };
        if variant == Variant::Original && !embed_inputs.iter().any(|n| n == "exaggeration") {
            bail!(
                "Unrecognised Chatterbox export in {}: language model has no position_ids and \
                 embed_tokens has no exaggeration input",
                model_dir.display()
            );
        }

        let num_layers = lm_inputs
            .iter()
            .filter(|n| n.starts_with("past_key_values.") && n.ends_with(".key"))
            .count();
        if num_layers == 0 {
            bail!(
                "Language model in {} exposes no KV cache inputs",
                model_dir.display()
            );
        }
        ensure_f32_kv_cache(&language_model)?;

        let tokenizer_path = model_dir.join("tokenizer.json");
        if !tokenizer_path.exists() {
            bail!(
                "Tokenizer not found at {}. Re-install the model.",
                tokenizer_path.display()
            );
        }
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        let voice_path = model_dir.join("default_voice.wav");
        if !voice_path.exists() {
            bail!(
                "Default voice not found at {}. Re-install the model.",
                voice_path.display()
            );
        }
        let reference_audio = load_wav_f32(&voice_path)?;

        Ok(Self {
            speech_encoder,
            embed_tokens,
            language_model,
            cond_decoder,
            tokenizer,
            reference_audio,
            model_id: model_id.to_string(),
            exaggeration: 0.5,
            variant,
            num_layers,
        })
    }

    fn encode_reference(&mut self) -> Result<SpeechEncoderOutput> {
        let audio_len = self.reference_audio.len();
        let input = Value::from_array(([1usize, audio_len], self.reference_audio.clone()))
            .with_context(|| "Failed to create audio_values tensor")?;

        let outputs = self
            .speech_encoder
            .run(ort::inputs![input])
            .with_context(|| "Speech encoder inference failed")?;

        let (cond_shape, cond_emb) = extract_f32(&outputs[0])?;
        let hidden_dim = *cond_shape.last().unwrap_or(&0);
        if hidden_dim == 0 {
            bail!("Speech encoder returned an empty conditioning embedding");
        }
        let (_, ref_x_vector) = extract_f32(&outputs[2])?;
        let (prompt_feat_shape, prompt_feat) = extract_f32(&outputs[3])?;

        Ok(SpeechEncoderOutput {
            cond_emb,
            hidden_dim,
            prompt_token: extract_i64(&outputs[1])?,
            ref_x_vector,
            prompt_feat,
            prompt_feat_shape,
        })
    }

    /// Tokenize with the tokenizer's post-processor enabled: both exports rely
    /// on it (the original appends the `[START_SPEECH]` token the LM must see;
    /// Turbo appends its end-of-text markers), matching upstream `tokenizer(text)`.
    fn tokenize_text(&self, text: &str) -> Result<Vec<i64>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        Ok(encoding.get_ids().iter().map(|&id| id as i64).collect())
    }

    /// Embed one chunk of ids (the whole prompt on step 0, one token after).
    /// Returns the flat `[1, n, hidden]` embedding.
    fn embed(&mut self, ids: Vec<i64>, positions: Vec<i64>) -> Result<Vec<f32>> {
        let n = ids.len();
        let ids_value = Value::from_array(([1usize, n], ids))
            .with_context(|| "Failed to create embed input_ids")?;

        let mut inputs: Vec<(&str, SessionInputValue<'_>)> = vec![("input_ids", ids_value.into())];
        if self.variant == Variant::Original {
            let pos_value = Value::from_array(([1usize, n], positions))
                .with_context(|| "Failed to create position_ids")?;
            let exag_value = Value::from_array(([1usize], vec![self.exaggeration]))
                .with_context(|| "Failed to create exaggeration")?;
            inputs.push(("position_ids", pos_value.into()));
            inputs.push(("exaggeration", exag_value.into()));
        }

        let out = self
            .embed_tokens
            .run(inputs)
            .with_context(|| "Embed tokens inference failed")?;
        Ok(extract_f32(&out[0])?.1)
    }

    fn embed_and_generate(
        &mut self,
        input_ids: &[i64],
        ref_out: &SpeechEncoderOutput,
    ) -> Result<Vec<i64>> {
        let hidden = ref_out.hidden_dim;
        let mut generated: Vec<i64> = vec![START_SPEECH_TOKEN];

        // KV cache: `present.*` outputs of the previous step, fed back as
        // `past_key_values.*`. Values are ref-counted ORT tensors, so moving
        // them across steps costs no copies. Order: layer-major, key then value.
        let mut kv_cache: Vec<DynValue> = Vec::with_capacity(self.num_layers * 2);
        // `Value::from_array` rejects zero-length dims, so allocate the empty
        // caches through the allocator instead.
        let allocator = Allocator::default();
        for _ in 0..self.num_layers * 2 {
            let empty = Tensor::<f32>::new(&allocator, [1usize, NUM_KV_HEADS, 0usize, HEAD_DIM])
                .with_context(|| "Failed to create empty KV cache tensor")?;
            kv_cache.push(empty.into_dyn());
        }
        let mut past_len: usize = 0;

        for step in 0..MAX_NEW_TOKENS {
            // 1. Embed: whole text prompt on the first step, then the token
            //    generated last step. Position ids only matter for the
            //    original export's embedder (Turbo's takes ids alone).
            let (ids, positions): (Vec<i64>, Vec<i64>) = if step == 0 {
                let pos = input_ids
                    .iter()
                    .enumerate()
                    .map(|(p, &id)| {
                        if id >= START_SPEECH_TOKEN {
                            0
                        } else {
                            p as i64 - 1
                        }
                    })
                    .collect();
                (input_ids.to_vec(), pos)
            } else {
                (vec![*generated.last().unwrap()], vec![step as i64])
            };
            let mut inputs_embeds = self.embed(ids, positions)?;
            let mut seq_len = inputs_embeds.len() / hidden;

            // 2. Prepend the reference-voice conditioning on the first step.
            if step == 0 {
                let mut combined = ref_out.cond_emb.clone();
                combined.extend_from_slice(&inputs_embeds);
                inputs_embeds = combined;
                seq_len += ref_out.cond_emb.len() / hidden;
            }

            // 3. Language model inputs, by name.
            let total_len = past_len + seq_len;
            let embeds_value = Value::from_array(([1usize, seq_len, hidden], inputs_embeds))
                .with_context(|| "Failed to create inputs_embeds")?;
            let attn_value = Value::from_array(([1usize, total_len], vec![1i64; total_len]))
                .with_context(|| "Failed to create attention_mask")?;

            let mut inputs: Vec<(String, SessionInputValue<'_>)> =
                Vec::with_capacity(3 + kv_cache.len());
            inputs.push(("inputs_embeds".into(), embeds_value.into()));
            inputs.push(("attention_mask".into(), attn_value.into()));
            if self.variant == Variant::Turbo {
                // Absolute positions over the [cond ‖ text ‖ generated] stream.
                let pos: Vec<i64> = (past_len..total_len).map(|p| p as i64).collect();
                let pos_value = Value::from_array(([1usize, seq_len], pos))
                    .with_context(|| "Failed to create LM position_ids")?;
                inputs.push(("position_ids".into(), pos_value.into()));
            }
            for (idx, value) in kv_cache.drain(..).enumerate() {
                let layer = idx / 2;
                let kind = if idx % 2 == 0 { "key" } else { "value" };
                inputs.push((format!("past_key_values.{layer}.{kind}"), value.into()));
            }

            let mut outputs = self
                .language_model
                .run(inputs)
                .with_context(|| "Language model inference failed")?;

            // 4. Greedy sample the last position with repetition penalty.
            let (logits_shape, logits) = outputs[0]
                .try_extract_tensor::<f32>()
                .with_context(|| "Failed to extract logits")?;
            let vocab = *logits_shape.last().unwrap_or(&0) as usize;
            if vocab == 0 || logits.len() < vocab {
                bail!("Language model returned malformed logits");
            }
            let mut last = logits[logits.len() - vocab..].to_vec();
            apply_repetition_penalty(&mut last, &generated, REPETITION_PENALTY);
            let next_token = last
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(idx, _)| idx as i64)
                .unwrap_or(STOP_SPEECH_TOKEN);

            generated.push(next_token);
            if next_token == STOP_SPEECH_TOKEN {
                break;
            }

            // 5. Carry the cache forward.
            for layer in 0..self.num_layers {
                for kind in ["key", "value"] {
                    let name = format!("present.{layer}.{kind}");
                    let value = outputs
                        .remove(&name)
                        .ok_or_else(|| anyhow::anyhow!("Language model output {name} missing"))?;
                    kv_cache.push(value);
                }
            }
            past_len = total_len;
        }

        Ok(generated)
    }

    fn decode_speech(
        &mut self,
        speech_tokens: &[i64],
        ref_out: &SpeechEncoderOutput,
    ) -> Result<Vec<f32>> {
        // prompt tokens ‖ generated (minus start/stop) [‖ silence for Turbo]
        let mut all_tokens = ref_out.prompt_token.clone();
        all_tokens.extend(
            speech_tokens
                .iter()
                .copied()
                .filter(|&t| t != START_SPEECH_TOKEN && t != STOP_SPEECH_TOKEN),
        );
        if self.variant == Variant::Turbo {
            all_tokens.extend(std::iter::repeat_n(SILENCE_TOKEN, TURBO_TRAILING_SILENCE));
        }

        let tokens_len = all_tokens.len();
        let tokens_value = Value::from_array(([1usize, tokens_len], all_tokens))
            .with_context(|| "Failed to create speech_tokens")?;

        let ref_x_len = ref_out.ref_x_vector.len();
        let speaker_emb = Value::from_array(([1usize, ref_x_len], ref_out.ref_x_vector.clone()))
            .with_context(|| "Failed to create speaker_embeddings")?;

        // Keep the encoder's [1, T, 80] layout; the decoder rejects a flat vector.
        let speaker_feat = Value::from_array((
            ref_out.prompt_feat_shape.clone(),
            ref_out.prompt_feat.clone(),
        ))
        .with_context(|| "Failed to create speaker_features")?;

        let inputs: Vec<(&str, SessionInputValue<'_>)> = vec![
            ("speech_tokens", tokens_value.into()),
            ("speaker_embeddings", speaker_emb.into()),
            ("speaker_features", speaker_feat.into()),
        ];
        let outputs = self
            .cond_decoder
            .run(inputs)
            .with_context(|| "Conditional decoder inference failed")?;

        Ok(extract_f32(&outputs[0])?.1)
    }
}

impl TtsEngine for ChatterboxEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        let started = std::time::Instant::now();

        eprintln!("  Encoding reference voice...");
        let ref_out = self.encode_reference()?;
        let t_encode = started.elapsed();

        eprintln!("  Tokenizing text...");
        let input_ids = self.tokenize_text(text)?;

        eprintln!("  Generating speech tokens...");
        let t0 = std::time::Instant::now();
        let speech_tokens = self.embed_and_generate(&input_ids, &ref_out)?;
        let t_generate = t0.elapsed();

        eprintln!(
            "  Decoding {} speech tokens to audio...",
            speech_tokens.len()
        );
        let t0 = std::time::Instant::now();
        let samples = self.decode_speech(&speech_tokens, &ref_out)?;
        let t_decode = t0.elapsed();

        eprintln!(
            "  Chatterbox ({:?}): {:.1}s audio in {:.1}s (encode {:.1}s, generate {:.1}s, decode {:.1}s)",
            self.variant,
            samples.len() as f32 / S3GEN_SR as f32,
            started.elapsed().as_secs_f32(),
            t_encode.as_secs_f32(),
            t_generate.as_secs_f32(),
            t_decode.as_secs_f32()
        );

        Ok(AudioOutput {
            samples,
            sample_rate: S3GEN_SR,
            channels: 1,
        })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn engine_kind(&self) -> EngineKind {
        EngineKind::Chatterbox
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        vec![VoiceInfo {
            id: "default".to_string(),
            name: "Default Voice".to_string(),
            language: "en".to_string(),
            description: "Built-in reference voice".to_string(),
        }]
    }

    fn set_voice(&mut self, voice_path: &str) -> Result<()> {
        let path = Path::new(voice_path);
        if !path.exists() {
            bail!("Voice file not found: {voice_path}");
        }
        self.reference_audio = load_wav_f32(path)?;
        Ok(())
    }
}

struct SpeechEncoderOutput {
    /// Flat `[1, cond_len, hidden]` conditioning embedding.
    cond_emb: Vec<f32>,
    hidden_dim: usize,
    prompt_token: Vec<i64>,
    ref_x_vector: Vec<f32>,
    prompt_feat: Vec<f32>,
    /// `[1, T, 80]`, forwarded verbatim to the decoder.
    prompt_feat_shape: Vec<usize>,
}

/// Load `<name>.onnx`, falling back to the quantized `<name>_q4.onnx` stub
/// name used by the `chatterbox-quantized` install.
fn load_component(model_dir: &Path, name: &str) -> Result<Session> {
    let candidates = [format!("{name}.onnx"), format!("{name}_q4.onnx")];
    let Some(path) = candidates
        .iter()
        .map(|f| model_dir.join(f))
        .find(|p| p.exists())
    else {
        bail!(
            "Model file not found: {} (tried {})",
            model_dir.join(&candidates[0]).display(),
            candidates.join(", ")
        );
    };
    Session::builder()
        .with_context(|| format!("Failed to create session builder for {name}"))?
        .commit_from_file(&path)
        .with_context(|| format!("Failed to load ONNX model: {}", path.display()))
}

fn input_names(session: &Session) -> Vec<String> {
    session
        .inputs()
        .iter()
        .map(|i| i.name().to_string())
        .collect()
}

/// The fp16 / q4f16 tiers export float16 KV caches, which this engine does not
/// handle; fail at load time with a clear message instead of mid-generation.
fn ensure_f32_kv_cache(lm: &Session) -> Result<()> {
    use ort::value::TensorElementType;
    use ort::value::ValueType;

    for outlet in lm.inputs() {
        if !outlet.name().starts_with("past_key_values.") {
            continue;
        }
        if let ValueType::Tensor { ty, .. } = outlet.dtype()
            && *ty != TensorElementType::Float32
        {
            bail!(
                "Language model KV cache is {ty:?}, only float32 exports are supported \
                 (use the fp32 or q4 tier, not fp16/q4f16)"
            );
        }
    }
    Ok(())
}

fn extract_f32(value: &DynValue) -> Result<(Vec<usize>, Vec<f32>)> {
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .with_context(|| "Failed to extract f32 tensor")?;
    Ok((shape.iter().map(|&d| d as usize).collect(), data.to_vec()))
}

fn extract_i64(value: &DynValue) -> Result<Vec<i64>> {
    let (_, data) = value
        .try_extract_tensor::<i64>()
        .with_context(|| "Failed to extract i64 tensor")?;
    Ok(data.to_vec())
}

fn load_wav_f32(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| "Failed to read WAV samples")?,
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| "Failed to read WAV samples")?,
    };

    // Downmix to mono if needed
    let channels = spec.channels as usize;
    let samples: Vec<f32> = if channels > 1 {
        samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples
    };

    if samples.is_empty() {
        bail!("Reference voice {} contains no samples", path.display());
    }

    // Resample to 24kHz if needed (simple nearest-neighbor)
    if spec.sample_rate != S3GEN_SR {
        let ratio = S3GEN_SR as f64 / spec.sample_rate as f64;
        let new_len = (samples.len() as f64 * ratio) as usize;
        let resampled: Vec<f32> = (0..new_len)
            .map(|i| {
                let src_idx = (i as f64 / ratio) as usize;
                samples[src_idx.min(samples.len() - 1)]
            })
            .collect();
        Ok(resampled)
    } else {
        Ok(samples)
    }
}

fn apply_repetition_penalty(logits: &mut [f32], input_ids: &[i64], penalty: f32) {
    for &id in input_ids {
        let idx = id as usize;
        if idx < logits.len() {
            if logits[idx] < 0.0 {
                logits[idx] *= penalty;
            } else {
                logits[idx] /= penalty;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repetition_penalty_scales_seen_tokens_only() {
        let mut logits = vec![2.0, -2.0, 1.0];
        apply_repetition_penalty(&mut logits, &[0, 1, 99], 2.0);
        assert_eq!(logits, vec![1.0, -4.0, 1.0]);
    }
}
