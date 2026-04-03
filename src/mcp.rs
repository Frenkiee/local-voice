use anyhow::Result;
use serde_json::{Value, json};
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
use crate::registry;

pub fn run_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut engines: HashMap<String, Box<dyn TtsEngine>> = HashMap::new();

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
                    "description": "Convert text to speech using a local TTS model. Plays audio on the user's device. Uses configured engine, model, and voice.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The text to speak aloud"
                            }
                        },
                        "required": ["text"]
                    }
                },
                {
                    "name": "set_config",
                    "description": "Configure TTS settings: engine, model, voice, and speed",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "engine": {
                                "type": "string",
                                "description": "TTS engine: kokoro, piper, chatterbox, supertonic"
                            },
                            "model": {
                                "type": "string",
                                "description": "Model ID (e.g. kokoro-q8f16, supertonic, en_US-lessac-medium)"
                            },
                            "voice": {
                                "type": "string",
                                "description": "Voice ID (e.g. af_alloy, F1, M2)"
                            },
                            "speed": {
                                "type": "number",
                                "description": "Speech speed multiplier (default 1.0)"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "get_config",
                    "description": "View current TTS configuration (engine, model, voice, speed)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
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
                },
                {
                    "name": "list_models",
                    "description": "List available and installed TTS models",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "engine": {
                                "type": "string",
                                "description": "Filter by engine: kokoro, piper, chatterbox, supertonic"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "list_voices",
                    "description": "List available TTS voices for engines that support them (Kokoro, Supertonic)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "engine": {
                                "type": "string",
                                "description": "Filter by engine: kokoro, supertonic"
                            }
                        },
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
        "set_config" => handle_set_config(id, params, engines),
        "get_config" => handle_get_config(id),
        "list_engines" => handle_list_engines(id),
        "list_models" => handle_list_models(id, params),
        "list_voices" => handle_list_voices(id, params),
        _ => json_rpc_error(id, -32602, &format!("Unknown tool: {tool_name}")),
    }
}

// ── speak ──

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

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => return tool_error(id, &format!("Failed to load config: {e}")),
    };

    let engine_kind = config
        .default_engine
        .unwrap_or_else(|| HardwareProfile::detect().recommended_engine());

    // Build cache key from config
    let voice = config.default_voice.as_deref();
    let cache_key = build_cache_key(&config, engine_kind, voice);
    let cache_key = match cache_key {
        Ok(k) => k,
        Err(e) => return tool_error(id, &e),
    };

    // Load engine if not cached
    if !engines.contains_key(&cache_key) {
        match load_engine(&config, engine_kind, voice) {
            Ok(e) => {
                engines.insert(cache_key.clone(), e);
            }
            Err(e) => return tool_error(id, &e),
        }
    }

    let eng = engines.get_mut(&cache_key).unwrap();

    match eng.synthesize(text) {
        Ok(audio_output) => {
            if let Err(e) = audio::play_audio(&audio_output) {
                return tool_error(id, &format!("Audio playback failed: {e}"));
            }
            tool_result(id, &format!("Spoke text using {engine_kind} engine"))
        }
        Err(e) => tool_error(id, &format!("Synthesis failed: {e}")),
    }
}

// ── set_config ──

fn handle_set_config(
    id: &Option<Value>,
    params: &Value,
    engines: &mut HashMap<String, Box<dyn TtsEngine>>,
) -> Value {
    let args = &params["arguments"];
    let mut config = match Config::load() {
        Ok(c) => c,
        Err(e) => return tool_error(id, &format!("Failed to load config: {e}")),
    };

    let mut changes = Vec::new();

    if let Some(engine) = args["engine"].as_str() {
        match engine.parse::<EngineKind>() {
            Ok(e) => {
                config.default_engine = Some(e);
                changes.push(format!("engine={engine}"));
            }
            Err(e) => return tool_error(id, &format!("{e}")),
        }
    }

    if let Some(model) = args["model"].as_str() {
        if !Config::is_model_installed(model) {
            return tool_error(id, &format!("Model '{model}' is not installed"));
        }
        config.default_model = Some(model.to_string());
        if let Some(engine) = Config::installed_engine_for(model) {
            config.default_engine = Some(engine);
        }
        changes.push(format!("model={model}"));
    }

    if let Some(voice) = args["voice"].as_str() {
        config.default_voice = Some(voice.to_string());
        // Auto-detect engine from voice
        if let Some((engine, _)) = registry::find_voice_any_engine(voice) {
            config.default_engine = Some(engine);
        }
        changes.push(format!("voice={voice}"));
    }

    if let Some(speed) = args["speed"].as_f64() {
        let spd = speed as f32;
        // Set speed on the active engine's config
        let engine = config.default_engine.unwrap_or(EngineKind::Kokoro);
        match engine {
            EngineKind::Kokoro => {
                config
                    .kokoro
                    .get_or_insert(crate::config::KokoroConfig {
                        variant: None,
                        speed: None,
                        default_voice: None,
                    })
                    .speed = Some(spd);
            }
            EngineKind::Supertonic => {
                config
                    .supertonic
                    .get_or_insert(crate::config::SupertonicConfig {
                        speed: None,
                        steps: None,
                        default_voice: None,
                    })
                    .speed = Some(spd);
            }
            _ => {}
        }
        changes.push(format!("speed={spd}"));
    }

    if changes.is_empty() {
        return tool_error(
            id,
            "No config values provided. Set engine, model, voice, or speed.",
        );
    }

    // Clear engine cache so next speak uses new config
    engines.clear();

    if let Err(e) = config.save() {
        return tool_error(id, &format!("Failed to save config: {e}"));
    }

    tool_result(id, &format!("Config updated: {}", changes.join(", ")))
}

// ── get_config ──

fn handle_get_config(id: &Option<Value>) -> Value {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => return tool_error(id, &format!("Failed to load config: {e}")),
    };

    let engine = config
        .default_engine
        .map(|e| e.as_str().to_string())
        .unwrap_or_else(|| "(auto-detect)".to_string());
    let model = config.default_model.as_deref().unwrap_or("(auto)");
    let voice = config
        .default_voice
        .as_deref()
        .unwrap_or("(engine default)");

    let speed = match config.default_engine {
        Some(EngineKind::Kokoro) => config.kokoro_speed(),
        Some(EngineKind::Supertonic) => config.supertonic_speed(),
        _ => 1.0,
    };

    let mut lines = vec![
        format!("engine: {engine}"),
        format!("model: {model}"),
        format!("voice: {voice}"),
        format!("speed: {speed}"),
    ];

    if config.default_engine == Some(EngineKind::Supertonic) {
        lines.push(format!("steps: {}", config.supertonic_steps()));
    }

    tool_result(id, &lines.join("\n"))
}

// ── list_engines ──

fn handle_list_engines(id: &Option<Value>) -> Value {
    let hw = HardwareProfile::detect();
    let recommended = hw.recommended_engine();

    let mut lines = Vec::new();
    for kind in EngineKind::all() {
        let installed = Config::installed_models(Some(*kind)).len();
        let rec = if *kind == recommended {
            " (recommended)"
        } else {
            ""
        };
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

// ── list_models ──

fn handle_list_models(id: &Option<Value>, params: &Value) -> Value {
    let engine_filter = params["arguments"]["engine"]
        .as_str()
        .and_then(|e| e.parse::<EngineKind>().ok());

    let models = registry::search_all(None, engine_filter);
    let installed = Config::installed_models(None);

    let mut lines = Vec::new();
    for model in models {
        let status = if installed.contains(&model.id.to_string()) {
            " [installed]"
        } else {
            ""
        };
        lines.push(format!(
            "{}: {} ({}, {}MB){}",
            model.id, model.engine, model.quality, model.size_mb, status
        ));
    }

    tool_result(id, &lines.join("\n"))
}

// ── list_voices ──

fn handle_list_voices(id: &Option<Value>, params: &Value) -> Value {
    let engine_filter = params["arguments"]["engine"]
        .as_str()
        .and_then(|e| e.parse::<EngineKind>().ok());

    let target_engines = match engine_filter {
        Some(e) => vec![e],
        None => EngineKind::all().to_vec(),
    };

    let mut lines = Vec::new();
    for kind in target_engines {
        let voices = registry::voices_for_engine(kind);
        if voices.is_empty() {
            continue;
        }

        let models = Config::installed_models(Some(kind));
        let installed_voices: Vec<String> = models
            .iter()
            .flat_map(|m| Config::installed_voices(kind, m))
            .collect();

        lines.push(format!("{}:", kind));
        for voice in voices {
            let status = if installed_voices.contains(&voice.id.to_string()) {
                " [installed]"
            } else {
                ""
            };
            lines.push(format!(
                "  {} — {} ({}){}",
                voice.id, voice.name, voice.gender, status
            ));
        }
    }

    if lines.is_empty() {
        return tool_result(id, "No voices available. Install a model first.");
    }

    tool_result(id, &lines.join("\n"))
}

// ── Engine loading helpers ──

fn build_cache_key(
    config: &Config,
    engine_kind: EngineKind,
    voice: Option<&str>,
) -> Result<String, String> {
    match engine_kind {
        EngineKind::Piper => {
            let voice_id = config
                .resolve_voice(voice)
                .ok_or("No voice configured. Install a model first.")?;
            Ok(format!("piper:{voice_id}"))
        }
        EngineKind::Kokoro => {
            let model_id = config
                .resolve_model(EngineKind::Kokoro)
                .ok_or("No Kokoro model installed.")?;
            let v = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.kokoro_voice());
            Ok(format!("kokoro:{model_id}:{v}"))
        }
        EngineKind::Chatterbox => {
            let model_id = config
                .resolve_model(EngineKind::Chatterbox)
                .ok_or("No Chatterbox model installed.")?;
            Ok(format!("chatterbox:{model_id}"))
        }
        EngineKind::Supertonic => {
            let model_id = config
                .resolve_model(EngineKind::Supertonic)
                .ok_or("No Supertonic model installed.")?;
            let v = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.supertonic_voice());
            Ok(format!("supertonic:{model_id}:{v}"))
        }
    }
}

fn load_engine(
    config: &Config,
    engine_kind: EngineKind,
    voice: Option<&str>,
) -> Result<Box<dyn TtsEngine>, String> {
    match engine_kind {
        EngineKind::Piper => {
            let voice_id = config.resolve_voice(voice).unwrap();
            let model_dir = Config::resolve_model_path(EngineKind::Piper, &voice_id);
            PiperEngine::load(&model_dir, &voice_id)
                .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                .map_err(|e| format!("Failed to load Piper: {e}"))
        }
        EngineKind::Kokoro => {
            let model_id = config.resolve_model(EngineKind::Kokoro).unwrap();
            let v = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.kokoro_voice());
            let spd = config.kokoro_speed();
            let model_dir = Config::resolve_model_path(EngineKind::Kokoro, &model_id);
            KokoroEngine::load(&model_dir, &model_id, v, spd)
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
            let v = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.supertonic_voice());
            let spd = config.supertonic_speed();
            let steps = config.supertonic_steps();
            let model_dir = Config::resolve_model_path(EngineKind::Supertonic, &model_id);
            SupertonicEngine::load(&model_dir, &model_id, v, spd, steps)
                .map(|e| Box::new(e) as Box<dyn TtsEngine>)
                .map_err(|e| format!("Failed to load Supertonic: {e}"))
        }
    }
}

// ── JSON-RPC helpers ──

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
    json_rpc_result(id, json!({ "content": [{ "type": "text", "text": text }] }))
}

fn tool_error(id: &Option<Value>, text: &str) -> Value {
    json_rpc_result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    )
}
