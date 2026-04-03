use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

use crate::audio;
use crate::config::Config;
use crate::engine::piper::PiperEngine;
use crate::engine::TtsEngine;

pub fn run_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut engine: Option<Box<dyn TtsEngine>> = None;

    eprintln!("[local-voice] MCP server starting");

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
            "notifications/initialized" | "initialized" => {
                // Notification — no response needed
                continue;
            }
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tools_call(&id, &request, &mut engine),
            "ping" => json_rpc_result(&id, json!({})),
            _ => {
                if id.is_some() {
                    json_rpc_error(&id, -32601, &format!("Method not found: {method}"))
                } else {
                    // Unknown notification, ignore
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
            "capabilities": {
                "tools": {}
            },
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
                    "description": "Convert text to speech using a local TTS model. Plays audio on the user's device.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The text to speak aloud"
                            },
                            "voice": {
                                "type": "string",
                                "description": "Voice/model ID to use (e.g. en_US-lessac-medium). Uses default if not specified."
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
                    "description": "List all installed TTS voices available for the speak tool",
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
    engine: &mut Option<Box<dyn TtsEngine>>,
) -> Value {
    let params = &request["params"];
    let tool_name = params["name"].as_str().unwrap_or("");

    match tool_name {
        "speak" => handle_speak(id, params, engine),
        "list_voices" => handle_list_voices(id),
        _ => json_rpc_error(id, -32602, &format!("Unknown tool: {tool_name}")),
    }
}

fn handle_speak(
    id: &Option<Value>,
    params: &Value,
    engine: &mut Option<Box<dyn TtsEngine>>,
) -> Value {
    let args = &params["arguments"];
    let text = match args["text"].as_str() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return tool_error(id, "Missing or empty 'text' argument"),
    };

    let voice = args["voice"].as_str().map(String::from);
    let save_path = args["save_path"].as_str().map(std::path::PathBuf::from);

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => return tool_error(id, &format!("Failed to load config: {e}")),
    };

    let voice_id = match config.resolve_voice(voice.as_deref()) {
        Some(v) => v,
        None => {
            return tool_error(
                id,
                "No voice available. Install one with: local-voice models install en_US-lessac-medium",
            )
        }
    };

    // Load engine if needed (lazy init or voice change)
    let needs_reload = engine
        .as_ref()
        .map(|e| e.model_id() != voice_id)
        .unwrap_or(true);

    if needs_reload {
        let model_dir = Config::resolve_model_path(crate::engine::EngineKind::Piper, &voice_id);
        match PiperEngine::load(&model_dir, &voice_id) {
            Ok(e) => *engine = Some(Box::new(e)),
            Err(e) => return tool_error(id, &format!("Failed to load model '{voice_id}': {e}")),
        }
    }

    let eng = engine.as_mut().unwrap();

    match eng.synthesize(text) {
        Ok(audio_output) => {
            // Save if requested
            if let Some(ref path) = save_path {
                if let Err(e) = audio::save_wav(&audio_output, path) {
                    return tool_error(id, &format!("Failed to save audio: {e}"));
                }
            }

            // Play audio
            if let Err(e) = audio::play_audio(&audio_output) {
                eprintln!("[local-voice] Audio playback failed: {e}");
                // Don't fail the tool call if playback fails but save succeeded
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
                    "Spoke text using voice '{}' and saved to {}",
                    voice_id,
                    path.display()
                )
            } else {
                format!("Spoke text using voice '{voice_id}'")
            };

            tool_result(id, &msg)
        }
        Err(e) => tool_error(id, &format!("Synthesis failed: {e}")),
    }
}

fn handle_list_voices(id: &Option<Value>) -> Value {
    let installed = Config::installed_models(None);
    if installed.is_empty() {
        return tool_result(
            id,
            "No voices installed. Install with: local-voice models install en_US-lessac-medium",
        );
    }

    let list = installed.join(", ");
    tool_result(id, &format!("Installed voices: {list}"))
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
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tool_result(id: &Option<Value>, text: &str) -> Value {
    json_rpc_result(
        id,
        json!({
            "content": [
                {
                    "type": "text",
                    "text": text
                }
            ]
        }),
    )
}

fn tool_error(id: &Option<Value>, text: &str) -> Value {
    json_rpc_result(
        id,
        json!({
            "content": [
                {
                    "type": "text",
                    "text": text
                }
            ],
            "isError": true
        }),
    )
}
