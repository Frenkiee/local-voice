use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use super::{AudioOutput, EngineKind, TtsEngine, VoiceInfo};
use ort::value::Value;

const S3GEN_SR: u32 = 24000;
const START_SPEECH_TOKEN: i64 = 6561;
const STOP_SPEECH_TOKEN: i64 = 6562;
const NUM_HIDDEN_LAYERS: usize = 30;
const NUM_KV_HEADS: usize = 16;
const HEAD_DIM: usize = 64;
const MAX_NEW_TOKENS: usize = 768;

pub struct ChatterboxEngine {
    speech_encoder: ort::session::Session,
    embed_tokens: ort::session::Session,
    language_model: ort::session::Session,
    cond_decoder: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    reference_audio: Vec<f32>,
    #[allow(dead_code)]
    model_id: String,
    exaggeration: f32,
}

impl ChatterboxEngine {
    pub fn load(model_dir: &Path, model_id: &str) -> Result<Self> {
        let speech_encoder = load_session(model_dir, "speech_encoder.onnx")?;
        let embed_tokens = load_session(model_dir, "embed_tokens.onnx")?;
        let language_model = load_session(model_dir, "language_model.onnx")?;
        let cond_decoder = load_session(model_dir, "conditional_decoder.onnx")?;

        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer = if tokenizer_path.exists() {
            tokenizers::Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?
        } else {
            bail!(
                "Tokenizer not found at {}. Re-install the model.",
                tokenizer_path.display()
            );
        };

        let voice_path = model_dir.join("default_voice.wav");
        let reference_audio = if voice_path.exists() {
            load_wav_f32(&voice_path)?
        } else {
            bail!(
                "Default voice not found at {}. Re-install the model.",
                voice_path.display()
            );
        };

        Ok(Self {
            speech_encoder,
            embed_tokens,
            language_model,
            cond_decoder,
            tokenizer,
            reference_audio,
            model_id: model_id.to_string(),
            exaggeration: 0.5,
        })
    }

    fn encode_reference(&mut self) -> Result<SpeechEncoderOutput> {
        let audio_len = self.reference_audio.len();
        let input =
            Value::from_array(([1usize, audio_len], self.reference_audio.clone()))
                .with_context(|| "Failed to create audio_values tensor")?;

        let outputs = self
            .speech_encoder
            .run(ort::inputs![input])
            .with_context(|| "Speech encoder inference failed")?;

        Ok(SpeechEncoderOutput {
            cond_emb: extract_f32_tensor(&outputs[0])?,
            prompt_token: extract_i64_tensor(&outputs[1])?,
            ref_x_vector: extract_f32_tensor(&outputs[2])?,
            prompt_feat: extract_f32_tensor(&outputs[3])?,
        })
    }

    fn tokenize_text(&self, text: &str) -> Result<Vec<i64>> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        Ok(encoding.get_ids().iter().map(|&id| id as i64).collect())
    }

    fn embed_and_generate(
        &mut self,
        input_ids: &[i64],
        ref_out: &SpeechEncoderOutput,
    ) -> Result<Vec<i64>> {
        let mut generate_tokens: Vec<i64> = vec![START_SPEECH_TOKEN];

        // Initialize KV cache (empty)
        let mut kv_cache: HashMap<String, Vec<f32>> = HashMap::new();
        let mut kv_cache_len: usize = 0;

        for layer in 0..NUM_HIDDEN_LAYERS {
            for kv in &["key", "value"] {
                let key = format!("past_key_values.{layer}.{kv}");
                kv_cache.insert(key, vec![]);
            }
        }

        // Determine hidden_dim from the first embed_tokens run
        let mut _hidden_dim: usize = 0;

        for i in 0..MAX_NEW_TOKENS {
            // Build embed token inputs
            let (embed_ids, pos_ids) = if i == 0 {
                (
                    input_ids.to_vec(),
                    (0..input_ids.len())
                        .map(|p| {
                            if input_ids[p] >= START_SPEECH_TOKEN {
                                0i64
                            } else {
                                (p as i64).saturating_sub(1)
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                (vec![*generate_tokens.last().unwrap()], vec![i as i64])
            };

            let embed_ids_len = embed_ids.len();
            let embed_input = Value::from_array(([1usize, embed_ids_len], embed_ids))
                .with_context(|| "Failed to create embed input_ids")?;
            let pos_input = Value::from_array(([1usize, embed_ids_len], pos_ids))
                .with_context(|| "Failed to create position_ids")?;
            let exag_input = Value::from_array(([1usize], vec![self.exaggeration]))
                .with_context(|| "Failed to create exaggeration")?;

            let embed_out = self
                .embed_tokens
                .run(ort::inputs![embed_input, pos_input, exag_input])
                .with_context(|| "Embed tokens inference failed")?;

            let (embed_shape, embed_data) = embed_out[0]
                .try_extract_tensor::<f32>()
                .with_context(|| "Failed to extract embeddings")?;

            let embed_vec = embed_data.to_vec();
            _hidden_dim = *embed_shape.last().unwrap_or(&0) as usize;
            let mut inputs_embeds = embed_vec;
            let mut seq_len = embed_ids_len;

            // Prepend conditioning embedding on first iteration
            if i == 0 {
                let cond_len = ref_out.cond_emb.len() / _hidden_dim;
                let mut combined = ref_out.cond_emb.clone();
                combined.extend_from_slice(&inputs_embeds);
                inputs_embeds = combined;
                seq_len += cond_len;
            }

            // Build attention mask
            let total_len = kv_cache_len + seq_len;
            let attention_mask = vec![1i64; total_len];

            // Build LLM inputs dynamically using ort inputs
            let embeds_value =
                Value::from_array(([1usize, seq_len, _hidden_dim], inputs_embeds))
                    .with_context(|| "Failed to create inputs_embeds")?;
            let attn_value = Value::from_array(([1usize, total_len], attention_mask))
                .with_context(|| "Failed to create attention_mask")?;

            // Build all inputs: embeds, attn, then KV cache pairs
            let mut input_values: Vec<ort::session::SessionInputValue<'_>> = vec![
                embeds_value.into(),
                attn_value.into(),
            ];

            for layer in 0..NUM_HIDDEN_LAYERS {
                for kv in &["key", "value"] {
                    let key = format!("past_key_values.{layer}.{kv}");
                    let data = kv_cache.get(&key).unwrap();
                    let cache_tokens = if data.is_empty() {
                        0
                    } else {
                        data.len() / (NUM_KV_HEADS * HEAD_DIM)
                    };
                    let val = Value::from_array((
                        [1usize, NUM_KV_HEADS, cache_tokens, HEAD_DIM],
                        data.clone(),
                    ))
                    .with_context(|| format!("Failed to create {key}"))?;
                    input_values.push(val.into());
                }
            }

            let llm_outputs = self
                .language_model
                .run(input_values.as_slice())
                .with_context(|| "Language model inference failed")?;

            // Extract logits (first output)
            let (_, logits_data) = llm_outputs[0]
                .try_extract_tensor::<f32>()
                .with_context(|| "Failed to extract logits")?;

            let logits: Vec<f32> = logits_data.to_vec();
            let vocab_size = logits.len() / seq_len;

            // Get logits for last position
            let start = (seq_len - 1) * vocab_size;
            let last_logits = &logits[start..start + vocab_size];

            // Apply repetition penalty and greedy sample
            let mut penalized = last_logits.to_vec();
            apply_repetition_penalty(&mut penalized, &generate_tokens, 1.2);

            let next_token = penalized
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(idx, _)| idx as i64)
                .unwrap_or(STOP_SPEECH_TOKEN);

            generate_tokens.push(next_token);

            if next_token == STOP_SPEECH_TOKEN {
                break;
            }

            // Update KV cache
            kv_cache_len += seq_len;
            for layer in 0..NUM_HIDDEN_LAYERS {
                for (k, kv) in ["key", "value"].iter().enumerate() {
                    let output_idx = 1 + layer * 2 + k;
                    if output_idx < llm_outputs.len() {
                        let (_, cache_data) = llm_outputs[output_idx]
                            .try_extract_tensor::<f32>()
                            .with_context(|| "Failed to extract KV cache")?;
                        let key = format!("past_key_values.{layer}.{kv}");
                        kv_cache.insert(key, cache_data.to_vec());
                    }
                }
            }
        }

        Ok(generate_tokens)
    }

    fn decode_speech(
        &mut self,
        speech_tokens: &[i64],
        ref_out: &SpeechEncoderOutput,
    ) -> Result<Vec<f32>> {
        // Strip start/stop tokens and prepend prompt tokens
        let tokens: Vec<i64> = speech_tokens
            .iter()
            .copied()
            .filter(|&t| t != START_SPEECH_TOKEN && t != STOP_SPEECH_TOKEN)
            .collect();

        let mut all_tokens = ref_out.prompt_token.clone();
        all_tokens.extend_from_slice(&tokens);

        let tokens_len = all_tokens.len();
        let tokens_value = Value::from_array(([1usize, tokens_len], all_tokens))
            .with_context(|| "Failed to create speech_tokens")?;

        let ref_x_len = ref_out.ref_x_vector.len();
        let speaker_emb =
            Value::from_array(([1usize, ref_x_len], ref_out.ref_x_vector.clone()))
                .with_context(|| "Failed to create speaker_embeddings")?;

        let feat_len = ref_out.prompt_feat.len();
        let speaker_feat =
            Value::from_array(([1usize, feat_len], ref_out.prompt_feat.clone()))
                .with_context(|| "Failed to create speaker_features")?;

        let outputs = self
            .cond_decoder
            .run(ort::inputs![tokens_value, speaker_emb, speaker_feat])
            .with_context(|| "Conditional decoder inference failed")?;

        let (_, wav_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .with_context(|| "Failed to extract audio waveform")?;

        Ok(wav_data.to_vec())
    }
}

impl TtsEngine for ChatterboxEngine {
    fn synthesize(&mut self, text: &str) -> Result<AudioOutput> {
        eprintln!("  Encoding reference voice...");
        let ref_out = self.encode_reference()?;

        eprintln!("  Tokenizing text...");
        let input_ids = self.tokenize_text(text)?;

        eprintln!("  Generating speech tokens...");
        let speech_tokens = self.embed_and_generate(&input_ids, &ref_out)?;

        eprintln!(
            "  Decoding {} speech tokens to audio...",
            speech_tokens.len()
        );
        let samples = self.decode_speech(&speech_tokens, &ref_out)?;

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
    cond_emb: Vec<f32>,
    prompt_token: Vec<i64>,
    ref_x_vector: Vec<f32>,
    prompt_feat: Vec<f32>,
}

fn load_session(model_dir: &Path, filename: &str) -> Result<ort::session::Session> {
    let path = model_dir.join(filename);
    if !path.exists() {
        bail!("Model file not found: {}", path.display());
    }
    ort::session::Session::builder()
        .with_context(|| format!("Failed to create session builder for {filename}"))?
        .commit_from_file(&path)
        .with_context(|| format!("Failed to load ONNX model: {}", path.display()))
}

fn extract_f32_tensor(value: &ort::value::DynValue) -> Result<Vec<f32>> {
    let (_, data) = value
        .try_extract_tensor::<f32>()
        .with_context(|| "Failed to extract f32 tensor")?;
    Ok(data.to_vec())
}

fn extract_i64_tensor(value: &ort::value::DynValue) -> Result<Vec<i64>> {
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
