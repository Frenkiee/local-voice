use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use crate::audio;
use crate::config::Config;
use crate::engine::chatterbox::ChatterboxEngine;
use crate::engine::kokoro::KokoroEngine;
use crate::engine::piper::PiperEngine;
use crate::engine::supertonic::SupertonicEngine;
use crate::engine::{EngineKind, TtsEngine};
use crate::hardware::HardwareProfile;

pub fn run_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut engines: HashMap<String, Box<dyn TtsEngine>> = HashMap::new();

    eprintln!("[local-voice] MCP server starting (multi-engine)");

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[local-voice] Invalid JSON: {e}");
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request["method"].as_str().unwrap_or("");

        let response = match method {
            "initialize" => handle_initialize(&id),
            "notifications/initialized" | "initialized" => continue,
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tools_call(&id, &request, &mut engines),
            "ping" => json_rpc_result(&id, json!({})),
            _ => {
                if id.is_some() {
                    json_rpc_error(&id, -32601, &format!("Method not found: {method}"))
                } else {
                    continue;
                }
            }
        };

        let out = serde_json::to_string(&response)?;
        writeln!(stdout, "{out}")?;
        stdout.flush()?;
    }

    Ok(())
}

fn handle_initialize(id: &Option<Value>) -> Value {
    json_rpc_result(
        id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "local-voice",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

fn handle_tools_list(id: &Option<Value>) -> Value {
    json_rpc_result(
        id,
        json!({
            "tools": [
                {
                    "name": "speak",
                    "description": "Convert text to speech using a local TTS model. Plays audio on the user's device. Supports multiple engines: Kokoro (recommended), Piper (lightweight), Chatterbox (voice cloning).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The text to speak aloud"
                            },
                            "voice": {
                                "type": "string",
                                "description": "Voice ID. For Kokoro: af_alloy, am_adam, bf_alice, etc. For Piper: en_US-lessac-medium, etc."
                            },
                            "engine": {
                                "type": "string",
                                "description": "TTS engine: kokoro (best quality), piper (fastest), chatterbox (voice cloning). Auto-detected if omitted."
                            },
                            "speed": {
                                "type": "number",
                                "description": "Speech speed multiplier (Kokoro only, default 1.0)"
                            },
                            "save_path": {
                                "type": "string",
                                "description": "Optional file path to save the audio as WAV"
                            }
                        },
                        "required": ["text"]
                    }
                },
                {
                    "name": "list_voices",
                    "description": "List all installed TTS voices and available engines",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "engine": {
                                "type": "string",
                                "description": "Filter by engine: kokoro, piper, chatterbox"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "list_engines",
                    "description": "List available TTS engines and the recommended one for this hardware",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            ]
        }),
    )
}

fn handle_tools_call(
    id: &Option<Value>,
    request: &Value,
    engines: &mut HashMap<String, Box<dyn TtsEngine>>,
) -> Value {
    let params = &request["params"];
    let tool_name = params["name"].as_str().unwrap_or("");

    match tool_name {
        "speak" => handle_speak(id, params, engines),
        "list_voices" => handle_list_voices(id, params),
        "list_engines" => handle_list_engines(id),
        _ => json_rpc_error(id, -32602, &format!("Unknown tool: {tool_name}")),
    }
}

fn handle_speak(
    id: &Option<Value>,
    params: &Value,
    engines: &mut HashMap<String, Box<dyn TtsEngine>>,
) -> Value {
    let args = &params["arguments"];
    let text = match args["text"].as_str() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return tool_error(id, "Missing or empty 'text' argument"),
    };

    let voice = args["voice"].as_str().map(String::from);
    let engine_arg = args["engine"].as_str();
    let speed = args["speed"].as_f64().map(|s| s as f32);
    let save_path = args["save_path"].as_str().map(std::path::PathBuf::from);

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => return tool_error(id, &format!("Failed to load config: {e}")),
    };

    // Determine engine
    let engine_kind = match engine_arg {
        Some(e) => match e.parse::<EngineKind>() {
            Ok(k) => k,
            Err(e) => return tool_error(id, &format!("{e}")),
        },
        None => config
            .default_engine
            .unwrap_or_else(|| HardwareProfile::detect().recommended_engine()),
    };

    // Build cache key
    let cache_key = match engine_kind {
        EngineKind::Piper => {
            let voice_id = match config.resolve_voice(voice.as_deref()) {
                Some(v) => v,
                None => {
                    return tool_error(
                        id,
                        "No voice available. Install one with: local-voice models install en_US-lessac-medium",
                    )
                }
            };
            format!("piper:{voice_id}")
        }
        EngineKind::Kokoro => {
            let kokoro_voice = voice.as_deref().or(config.default_voice.as_deref()).unwrap_or(config.kokoro_voice());
            let model_id = match config.resolve_model(EngineKind::Kokoro) {
                Some(m) => m,
                None => {
                    return tool_error(
                        id,
                        "No Kokoro model installed. Run: local-voice models install kokoro-q8f16",
                    )
                }
            };
            format!("kokoro:{model_id}:{kokoro_voice}")
        }
        EngineKind::Chatterbox => {
            let model_id = match config.resolve_model(EngineKind::Chatterbox) {
                Some(m) => m,
                None => {
                    return tool_error(
                        id,
                        "No Chatterbox model installed. Run: local-voice models install chatterbox-quantized",
                    )
                }
            };
            format!("chatterbox:{model_id}")
        }
        EngineKind::Supertonic => {
            let st_voice = voice.as_deref().or(config.default_voice.as_deref()).unwrap_or(config.supertonic_voice());
            let model_id = match config.resolve_model(EngineKind::Supertonic) {
                Some(m) => m,
                None => {
                    return tool_error(
                        id,
                        "No Supertonic model installed. Run: local-voice models install supertonic",
                    )
                }
            };
            format!("supertonic:{model_id}:{st_voice}")
        }
    };

    // Load engine if not cached
    if !engines.contains_key(&cache_key) {
        let engine: Result<Box<dyn TtsEngine>, String> = match engine_kind {
            EngineKind::Piper => {
                let voice_id = config.resolve_voice(voice.as_deref()).unwrap();
                let model_dir = Config::resolve_model_path(EngineKind::Piper, &voice_id);
                PiperEngine::load(&model_dir, &voice_id)
                    .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                    .map_err(|e| format!("Failed to load Piper: {e}"))
            }
            EngineKind::Kokoro => {
                let model_id = config.resolve_model(EngineKind::Kokoro).unwrap();
                let kokoro_voice = voice.as_deref().or(config.default_voice.as_deref()).unwrap_or(config.kokoro_voice());
                let spd = speed.unwrap_or(config.kokoro_speed());
                let model_dir = Config::resolve_model_path(EngineKind::Kokoro, &model_id);
                KokoroEngine::load(&model_dir, &model_id, kokoro_voice, spd)
                    .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                    .map_err(|e| format!("Failed to load Kokoro: {e}"))
            }
            EngineKind::Chatterbox => {
                let model_id = config.resolve_model(EngineKind::Chatterbox).unwrap();
                let model_dir = Config::resolve_model_path(EngineKind::Chatterbox, &model_id);
                ChatterboxEngine::load(&model_dir, &model_id)
                    .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                    .map_err(|e| format!("Failed to load Chatterbox: {e}"))
            }
            EngineKind::Supertonic => {
                let model_id = config.resolve_model(EngineKind::Supertonic).unwrap();
                let st_voice = voice.as_deref().or(config.default_voice.as_deref()).unwrap_or(config.supertonic_voice());
                let spd = speed.unwrap_or(config.supertonic_speed());
                let steps = config.supertonic_steps();
                let model_dir = Config::resolve_model_path(EngineKind::Supertonic, &model_id);
                SupertonicEngine::load(&model_dir, &model_id, st_voice, spd, steps)
                    .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                    .map_err(|e| format!("Failed to load Supertonic: {e}"))
            }
        };

        match engine {
            Ok(e) => {
                engines.insert(cache_key.clone(), e);
            }
            Err(e) => return tool_error(id, &e),
        }
    }

    let eng = engines.get_mut(&cache_key).unwrap();

    match eng.synthesize(text) {
        Ok(audio_output) => {
            if let Some(ref path) = save_path {
                if let Err(e) = audio::save_wav(&audio_output, path) {
                    return tool_error(id, &format!("Failed to save audio: {e}"));
                }
            }

            if let Err(e) = audio::play_audio(&audio_output) {
                eprintln!("[local-voice] Audio playback failed: {e}");
                if save_path.is_some() {
                    return tool_result(
                        id,
                        &format!(
                            "Audio saved to {} (playback failed: {e})",
                            save_path.unwrap().display()
                        ),
                    );
                }
                return tool_error(id, &format!("Audio playback failed: {e}"));
            }

            let msg = if let Some(ref path) = save_path {
                format!(
                    "Spoke text using {} engine and saved to {}",
                    engine_kind,
                    path.display()
                )
            } else {
                format!("Spoke text using {engine_kind} engine")
            };

            tool_result(id, &msg)
        }
        Err(e) => tool_error(id, &format!("Synthesis failed: {e}")),
    }
}

fn handle_list_voices(id: &Option<Value>, params: &Value) -> Value {
    let engine_filter = params["arguments"]["engine"]
        .as_str()
        .and_then(|e| e.parse::<EngineKind>().ok());

    let installed = Config::installed_models(engine_filter);
    if installed.is_empty() {
        return tool_result(
            id,
            "No voices installed. Install with: local-voice models install kokoro-q8f16",
        );
    }

    let mut lines = vec![format!("Installed models: {}", installed.join(", "))];

    // Show voices for engines that have them
    let engines = match engine_filter {
        Some(e) => vec![e],
        None => EngineKind::all().to_vec(),
    };

    for kind in engines {
        let voices = crate::registry::voices_for_engine(kind);
        if !voices.is_empty() && !Config::installed_models(Some(kind)).is_empty() {
            let voice_ids: Vec<&str> = voices.iter().map(|v| v.id).collect();
            lines.push(format!("{} voices: {}", kind, voice_ids.join(", ")));
        }
    }

    tool_result(id, &lines.join("\n"))
}

fn handle_list_engines(id: &Option<Value>) -> Value {
    let hw = HardwareProfile::detect();
    let recommended = hw.recommended_engine();

    let mut lines = Vec::new();
    for kind in EngineKind::all() {
        let installed = Config::installed_models(Some(*kind)).len();
        let rec = if *kind == recommended { " (recommended)" } else { "" };
        let status = if installed > 0 {
            format!("{installed} model(s) installed")
        } else {
            "not installed".to_string()
        };
        lines.push(format!(
            "{}{rec}: {status} — {}",
            kind.as_str(),
            kind.description()
        ));
    }

    tool_result(id, &lines.join("\n"))
}

// JSON-RPC helpers

fn json_rpc_result(id: &Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.clone().unwrap_or(Value::Null),
        "result": result
    })
}

fn json_rpc_error(id: &Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.clone().unwrap_or(Value::Null),
        "error": { "code": code, "message": message }
    })
}

fn tool_result(id: &Option<Value>, text: &str) -> Value {
    json_rpc_result(
        id,
        json!({ "content": [{ "type": "text", "text": text }] }),
    )
}

fn tool_error(id: &Option<Value>, text: &str) -> Value {
    json_rpc_result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    )
}
