mod audio;
mod cli;
mod config;
mod download;
mod ducking;
mod engine;
mod hardware;
mod mcp;
mod phonemize;
mod registry;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{Cli, Commands, ConfigAction, EngineAction, ModelAction, VoiceAction};
use config::Config;
use engine::TtsEngine;
use owo_colors::OwoColorize;

#[tokio::main]
async fn main() -> Result<()> {
    // Enable ANSI color support on Windows 10+ CMD
    #[cfg(windows)]
    let _ = enable_ansi_support::enable_ansi_support();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Engines { action }) => handle_engines(action)?,
        Some(Commands::Models { action }) => handle_models(action).await?,
        Some(Commands::Voices { action }) => handle_voices(action).await?,
        Some(Commands::Speak {
            text,
            voice,
            engine,
            speed,
            language,
            output,
            no_play,
            no_ducking,
            ducking_level,
        }) => handle_speak(
            &text,
            voice.as_deref(),
            engine.as_deref(),
            speed,
            language.as_deref(),
            output.as_deref(),
            no_play,
            SpeakDucking {
                disable: no_ducking,
                level: ducking_level,
            },
        )?,
        Some(Commands::Serve) => mcp::run_server()?,
        Some(Commands::Config { action }) => handle_config(action)?,
        Some(Commands::Doctor) => handle_doctor()?,
        None => interactive_mode().await?,
    }

    Ok(())
}

fn handle_engines(action: Option<EngineAction>) -> Result<()> {
    let hw = hardware::HardwareProfile::detect();
    let recommended = hw.recommended_engine();

    match action {
        None | Some(EngineAction::List) => {
            println!();
            println!("  {}", "TTS Engines".bold());
            println!("  {}", "─".repeat(60));

            for kind in engine::EngineKind::all() {
                let installed_count = Config::installed_models(Some(*kind)).len();
                let rec = if *kind == recommended {
                    " ★ recommended".green().to_string()
                } else {
                    String::new()
                };
                let status = if installed_count > 0 {
                    format!("{} model(s) installed", installed_count)
                } else {
                    "not installed".dimmed().to_string()
                };

                println!();
                println!("  {}{}", kind.as_str().bold(), rec);
                println!("    {}", kind.description());
                println!("    Status: {status}");
            }
            println!();
        }
        Some(EngineAction::Info { engine }) => {
            let kind: engine::EngineKind = engine.parse()?;
            let models = registry::search_all(None, Some(kind));

            println!();
            println!("  {} — {}", kind.as_str().bold(), kind.description());
            println!();
            println!("  Available models:");
            for model in models {
                let installed = if Config::is_model_installed(model.id) {
                    " ✓".green().to_string()
                } else {
                    String::new()
                };
                println!(
                    "    {:<24} {:>6}MB  {}{}",
                    model.id, model.size_mb, model.description, installed
                );
            }

            let voices = registry::voices_for_engine(kind);
            if !voices.is_empty() {
                println!();
                println!("  Available voices ({}):", voices.len());
                for voice in voices {
                    println!("    {:<16} {} ({})", voice.id, voice.name, voice.gender);
                }
            }
            println!();
        }
    }

    Ok(())
}

async fn handle_models(action: ModelAction) -> Result<()> {
    match action {
        ModelAction::List { language, engine } => {
            let engine_filter = engine
                .as_deref()
                .map(|e| e.parse::<engine::EngineKind>())
                .transpose()?;
            let models = registry::search_all(language.as_deref(), engine_filter);
            let installed = Config::installed_models(None);

            println!();
            println!(
                "  {:<26} {:<10} {:<8} {:<8} {:<6} {}",
                "MODEL".bold(),
                "ENGINE".bold(),
                "LANG".bold(),
                "QUALITY".bold(),
                "SIZE".bold(),
                "STATUS".bold()
            );
            println!("  {}", "─".repeat(78));

            for model in models {
                let status = if installed.contains(&model.id.to_string()) {
                    "✓ installed".green().to_string()
                } else {
                    String::new()
                };

                println!(
                    "  {:<26} {:<10} {:<8} {:<8} {:>4}MB {}",
                    model.id, model.engine, model.language, model.quality, model.size_mb, status
                );
            }
            println!();

            if installed.is_empty() {
                println!("  Install a model:");
                println!("    local-voice models install kokoro-q8f16     # recommended");
                println!("    local-voice models install en_US-lessac-medium  # lightweight");
                println!();
            }
        }

        ModelAction::Install { id } => {
            let (engine_kind, entry) = registry::find_model_any_engine(&id).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown model '{id}'. Run 'local-voice models list' to see available models."
                )
            })?;

            if Config::is_model_installed(&id) {
                println!("Model '{id}' is already installed.");
                return Ok(());
            }

            let model_dir = Config::model_path_for(engine_kind, &id);
            let plan = registry::download_plan(&id)?;

            println!(
                "Installing {} ({}) [{}]...",
                entry.name.bold(),
                entry.id,
                engine_kind
            );
            println!();

            for item in &plan {
                let dest = model_dir.join(&item.dest_relative);
                let size_hint = item
                    .size_hint_mb
                    .map(|s| format!(" (~{s} MB)"))
                    .unwrap_or_default();
                println!(
                    "  Downloading {}{}...",
                    item.dest_relative.display(),
                    size_hint
                );
                download::download_file(&item.url, &dest).await?;
            }

            println!();
            println!(
                "{}",
                format!("✓ Model '{id}' installed successfully.").green()
            );
            println!();

            match engine_kind {
                engine::EngineKind::Kokoro => {
                    println!("  Try it: local-voice speak 'Hello, world!'");
                }
                engine::EngineKind::Supertonic => {
                    println!("  Try it: local-voice speak 'Hello, world!' --voice F1");
                    if registry::supertonic::supported_languages(&id)
                        .is_some_and(|l| l.contains(&"sl"))
                    {
                        println!("          local-voice speak 'Dober dan!' --language sl");
                    }
                }
                _ => {
                    println!("  Try it: local-voice speak 'Hello, world!' --voice {id}");
                }
            }

            let mut config = Config::load()?;
            if config.default_voice.is_none() || config.default_engine.is_none() {
                if engine_kind == engine::EngineKind::Kokoro {
                    config.default_engine = Some(engine_kind);
                    config.default_voice = Some(id.clone());
                } else if config.default_voice.is_none() {
                    config.default_voice = Some(id.clone());
                    config.default_engine = Some(engine_kind);
                }
                config.save()?;
                println!("  Set as default.");
            }
            println!();
        }

        ModelAction::Remove { id } => {
            if !Config::is_model_installed(&id) {
                bail!("Model '{id}' is not installed.");
            }

            let engine_kind = Config::installed_engine_for(&id)
                .or_else(|| registry::find_model_any_engine(&id).map(|(e, _)| e))
                .unwrap_or(engine::EngineKind::Piper);

            let model_dir = Config::resolve_model_path(engine_kind, &id);
            std::fs::remove_dir_all(&model_dir)?;
            println!("{}", format!("✓ Model '{id}' removed.").green());

            let mut config = Config::load()?;
            if config.default_voice.as_deref() == Some(&id) {
                config.default_voice = None;
                config.default_engine = None;
                config.save()?;
            }
            if config.default_model.as_deref() == Some(&id) {
                config.default_model = None;
                config.save()?;
            }
        }

        ModelAction::Default { id } => {
            if !Config::is_model_installed(&id) {
                bail!(
                    "Model '{id}' is not installed. Run 'local-voice models install {id}' first."
                );
            }

            let mut config = Config::load()?;
            config.default_model = Some(id.clone());
            if let Some(engine) = Config::installed_engine_for(&id) {
                config.default_engine = Some(engine);
            }
            config.save()?;
            println!("{}", format!("✓ Default model set to '{id}'.").green());
        }
    }

    Ok(())
}

async fn handle_voices(action: Option<VoiceAction>) -> Result<()> {
    match action {
        None | Some(VoiceAction::List { engine: None }) => {
            show_voices_for_engines(engine::EngineKind::all())?;
        }
        Some(VoiceAction::List { engine: Some(e) }) => {
            let kind: engine::EngineKind = e.parse()?;
            show_voices_for_engines(&[kind])?;
        }
        Some(VoiceAction::Install { id }) => {
            let (engine_kind, _voice_entry) =
                registry::find_voice_any_engine(&id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Unknown voice '{id}'. Run 'local-voice voices list' to see available voices."
                    )
                })?;

            // Find the active model for this engine to know where to put the voice
            // (default model if it belongs to this engine, else first installed)
            let config = Config::load()?;
            let model_id = config.resolve_model(engine_kind).ok_or_else(|| {
                anyhow::anyhow!(
                    "No {} model installed. Install one first: local-voice models install {}",
                    engine_kind,
                    match engine_kind {
                        engine::EngineKind::Kokoro => "kokoro-q8f16",
                        engine::EngineKind::Supertonic => registry::supertonic::RECOMMENDED_MODEL,
                        _ => "<model>",
                    }
                )
            })?;

            let model_dir = Config::resolve_model_path(engine_kind, &model_id);
            // Supertonic voice styles are model-specific: fetch from the installed model's repo
            let plan = match engine_kind {
                engine::EngineKind::Supertonic => {
                    registry::supertonic::voice_download_plan_for_model(&model_id, &id)?
                }
                _ => registry::voice_download_plan(&id)?,
            };

            println!(
                "Installing voice '{}' for {} (model: {model_id})...",
                id.bold(),
                engine_kind
            );

            for item in &plan {
                let dest = model_dir.join(&item.dest_relative);
                println!("  Downloading {}...", item.dest_relative.display());
                download::download_file(&item.url, &dest).await?;
            }

            println!();
            println!("{}", format!("✓ Voice '{id}' installed.").green());
            println!("  Try it: local-voice speak 'Hello!' --voice {id}");
            println!();
        }
        Some(VoiceAction::Remove { id }) => {
            let (engine_kind, _) = registry::find_voice_any_engine(&id)
                .ok_or_else(|| anyhow::anyhow!("Unknown voice '{id}'."))?;

            let model_id = Config::load()?
                .resolve_model(engine_kind)
                .ok_or_else(|| anyhow::anyhow!("No {} model installed.", engine_kind))?;

            let model_dir = Config::resolve_model_path(engine_kind, &model_id);

            // Determine voice file extension
            let voice_file = match engine_kind {
                engine::EngineKind::Kokoro => model_dir.join("voices").join(format!("{id}.bin")),
                engine::EngineKind::Supertonic => {
                    model_dir.join("voices").join(format!("{id}.json"))
                }
                _ => bail!("Engine {engine_kind} does not support voice removal."),
            };

            if !voice_file.exists() {
                bail!("Voice '{id}' is not installed.");
            }

            std::fs::remove_file(&voice_file)?;
            println!("{}", format!("✓ Voice '{id}' removed.").green());
        }
        Some(VoiceAction::Default { id }) => {
            let mut config = Config::load()?;
            // Auto-detect engine from voice
            if let Some((engine, _)) = registry::find_voice_any_engine(&id) {
                config.default_engine = Some(engine);
            }
            config.default_voice = Some(id.clone());
            config.save()?;
            println!("{}", format!("✓ Default voice set to '{id}'.").green());
        }
    }

    Ok(())
}

fn show_voices_for_engines(engines: &[engine::EngineKind]) -> Result<()> {
    let mut any_shown = false;

    for kind in engines {
        let voices = registry::voices_for_engine(*kind);
        if voices.is_empty() {
            continue;
        }

        let models = Config::installed_models(Some(*kind));
        let installed_voices: Vec<String> = models
            .iter()
            .flat_map(|m| Config::installed_voices(*kind, m))
            .collect();

        println!();
        println!("  {} voices:", kind.as_str().bold());
        println!(
            "  {:<16} {:<16} {:<8} {:<8} {}",
            "ID".bold(),
            "NAME".bold(),
            "LANG".bold(),
            "GENDER".bold(),
            "STATUS".bold()
        );
        println!("  {}", "─".repeat(60));

        for voice in voices {
            let status = if installed_voices.contains(&voice.id.to_string()) {
                "✓ installed".green().to_string()
            } else {
                String::new()
            };
            println!(
                "  {:<16} {:<16} {:<8} {:<8} {}",
                voice.id, voice.name, voice.language, voice.gender, status
            );
        }
        any_shown = true;
    }

    if !any_shown {
        println!("No voices available for the selected engine(s).");
        println!("Install a model first: local-voice models install kokoro-q8f16");
    }
    println!();

    Ok(())
}

/// One-run ducking overrides from the `speak` command line.
#[derive(Debug, Clone, Copy, Default)]
struct SpeakDucking {
    /// `--no-ducking`
    disable: bool,
    /// `--ducking-level <0..1>`
    level: Option<f32>,
}

#[allow(clippy::too_many_arguments)]
fn handle_speak(
    text: &str,
    voice: Option<&str>,
    engine_name: Option<&str>,
    speed: Option<f32>,
    language: Option<&str>,
    output: Option<&std::path::Path>,
    no_play: bool,
    ducking_override: SpeakDucking,
) -> Result<()> {
    if let Some(s) = speed {
        validate_speed(s)?;
    }
    let config = Config::load()?;

    // Ducking: config, then per-run CLI overrides.
    let mut ducking_settings = config.ducking_settings();
    if ducking_override.disable {
        ducking_settings.enabled = false;
    }
    if let Some(level) = ducking_override.level {
        if !(0.0..=1.0).contains(&level) {
            bail!("Invalid --ducking-level {level}: must be between 0 and 1");
        }
        ducking_settings.level = level;
    }

    let engine_kind = match engine_name {
        Some(e) => e.parse::<engine::EngineKind>()?,
        None => {
            // Auto-detect engine from voice ID if provided
            if let Some(v) = voice {
                if let Some((engine, _)) = registry::find_voice_any_engine(v) {
                    engine
                } else if let Some((engine, _)) = registry::find_model_any_engine(v) {
                    // Voice arg might be a model name for Piper
                    engine
                } else {
                    config
                        .default_engine
                        .unwrap_or_else(|| hardware::HardwareProfile::detect().recommended_engine())
                }
            } else {
                config
                    .default_engine
                    .unwrap_or_else(|| hardware::HardwareProfile::detect().recommended_engine())
            }
        }
    };

    let mut tts: Box<dyn engine::TtsEngine> = match engine_kind {
        engine::EngineKind::Piper => {
            let voice_id = config
                .resolve_voice(voice)
                .ok_or_else(|| anyhow::anyhow!("No voice configured. Install a model first."))?;

            if !Config::is_model_installed(&voice_id) {
                bail!(
                    "Voice '{voice_id}' is not installed. Run: local-voice models install {voice_id}"
                );
            }

            let model_dir = Config::resolve_model_path(engine::EngineKind::Piper, &voice_id);
            eprintln!("Speaking with Piper voice '{voice_id}'...");
            Box::new(engine::piper::PiperEngine::load(&model_dir, &voice_id)?)
        }
        engine::EngineKind::Kokoro => {
            let model_id = config
                .resolve_model(engine::EngineKind::Kokoro)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No Kokoro model installed. Run: local-voice models install kokoro-q8f16"
                    )
                })?;

            let kokoro_voice = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.kokoro_voice());
            let spd = speed.unwrap_or(config.kokoro_speed());
            let model_dir = Config::resolve_model_path(engine::EngineKind::Kokoro, &model_id);

            eprintln!("Speaking with Kokoro voice '{kokoro_voice}' (model: {model_id})...");

            Box::new(engine::kokoro::KokoroEngine::load(
                &model_dir,
                &model_id,
                kokoro_voice,
                spd,
            )?)
        }
        engine::EngineKind::Chatterbox => {
            let model_id = config.resolve_model(engine::EngineKind::Chatterbox).ok_or_else(|| {
                anyhow::anyhow!(
                    "No Chatterbox model installed. Run: local-voice models install chatterbox-quantized"
                )
            })?;

            let model_dir = Config::resolve_model_path(engine::EngineKind::Chatterbox, &model_id);

            eprintln!("Speaking with Chatterbox (model: {model_id})...");

            let mut eng = engine::chatterbox::ChatterboxEngine::load(&model_dir, &model_id)?;

            // If a voice path was provided, use it for cloning
            if let Some(v) = voice
                && std::path::Path::new(v).exists()
            {
                eng.set_voice(v)?;
            }

            Box::new(eng)
        }
        engine::EngineKind::Supertonic => {
            let model_id = config
                .resolve_model(engine::EngineKind::Supertonic)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No Supertonic model installed. Run: local-voice models install supertonic-3"
                    )
                })?;

            let st_voice = voice
                .or(config.default_voice.as_deref())
                .unwrap_or(config.supertonic_voice());
            let spd = speed.unwrap_or(config.supertonic_speed());
            let steps = config.supertonic_steps();
            let lang = language.unwrap_or(config.supertonic_language());
            let model_dir = Config::resolve_model_path(engine::EngineKind::Supertonic, &model_id);

            eprintln!(
                "Speaking with Supertonic voice '{st_voice}' (model: {model_id}, language: {lang})..."
            );

            Box::new(engine::supertonic::SupertonicEngine::load(
                &model_dir, &model_id, st_voice, spd, steps, lang,
            )?)
        }
    };

    let audio_output = tts.synthesize(text)?;

    if let Some(path) = output {
        audio::save_wav(&audio_output, path)?;
        eprintln!("✓ Saved to {}", path.display());
    }

    if !no_play {
        audio::play_audio(&audio_output, &ducking_settings)?;
    }

    Ok(())
}

/// Parse a user-supplied boolean: on/off, true/false, yes/no, 1/0.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Parse a speech-speed multiplier; must be a finite number greater than zero
/// (a speed of 0 would make engines size infinite buffers).
fn parse_speed(value: &str) -> Result<f32> {
    let speed: f32 = value
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid speed: {value}"))?;
    validate_speed(speed)
}

fn validate_speed(speed: f32) -> Result<f32> {
    if !speed.is_finite() || speed <= 0.0 || speed > 10.0 {
        bail!("Invalid speed {speed}: must be between 0 (exclusive) and 10");
    }
    Ok(speed)
}

fn handle_config(action: Option<ConfigAction>) -> Result<()> {
    let mut config = Config::load()?;

    match action {
        None | Some(ConfigAction::Show) => {
            let active_engine = config
                .default_engine
                .map(|e| e.as_str().to_string())
                .unwrap_or_else(|| "(auto-detect)".dimmed().to_string());
            let speed = match config.default_engine {
                Some(engine::EngineKind::Supertonic) => config.supertonic_speed(),
                _ => config.kokoro_speed(),
            };

            println!();
            println!("  {}", "Configuration:".bold());
            println!("    engine:     {active_engine}");
            println!(
                "    model:      {}",
                config.default_model.as_deref().unwrap_or("(auto)").dimmed()
            );
            println!(
                "    voice:      {}",
                config
                    .default_voice
                    .as_deref()
                    .unwrap_or("(engine default)")
                    .dimmed()
            );
            println!("    speed:      {speed}");
            if config.default_engine == Some(engine::EngineKind::Supertonic) {
                println!("    steps:      {}", config.supertonic_steps());
                println!("    language:   {}", config.supertonic_language());
            }
            println!(
                "    output_dir: {}",
                config.output_dir.as_deref().unwrap_or("(not set)").dimmed()
            );
            let ducking = config.ducking_settings();
            if ducking.enabled {
                println!(
                    "    ducking:    on (level {}%, fade {}ms)",
                    (ducking.level * 100.0).round() as u32,
                    ducking.fade_ms
                );
            } else {
                println!("    ducking:    {}", "off".dimmed());
            }
            println!();
        }

        Some(ConfigAction::Set { key, value }) => {
            match key.as_str() {
                // Top-level shortcuts
                "speed" => {
                    let speed = parse_speed(&value)?;
                    let eng = config.default_engine.unwrap_or(engine::EngineKind::Kokoro);
                    match eng {
                        engine::EngineKind::Kokoro => {
                            config
                                .kokoro
                                .get_or_insert(config::KokoroConfig {
                                    variant: None,
                                    speed: None,
                                    default_voice: None,
                                })
                                .speed = Some(speed);
                        }
                        engine::EngineKind::Supertonic => {
                            config.supertonic.get_or_insert_default().speed = Some(speed);
                        }
                        _ => {
                            // Set both for convenience
                            config
                                .kokoro
                                .get_or_insert(config::KokoroConfig {
                                    variant: None,
                                    speed: None,
                                    default_voice: None,
                                })
                                .speed = Some(speed);
                        }
                    }
                }
                "steps" => {
                    let steps: u32 = value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid steps: {value}"))?;
                    config.supertonic.get_or_insert_default().steps = Some(steps);
                }
                "language" | "supertonic.language" => {
                    registry::supertonic::validate_language(&value)?;
                    config.supertonic.get_or_insert_default().language =
                        Some(value.trim().to_lowercase());
                }
                "engine" | "default_engine" => {
                    let eng: engine::EngineKind = value.parse()?;
                    config.default_engine = Some(eng);
                }
                "model" | "default_model" => {
                    if !Config::is_model_installed(&value) {
                        bail!(
                            "Model '{value}' is not installed. Run 'local-voice models list' to see available models."
                        );
                    }
                    config.default_model = Some(value.clone());
                    if let Some(eng) = Config::installed_engine_for(&value) {
                        config.default_engine = Some(eng);
                    }
                }
                "voice" | "default_voice" => {
                    config.default_voice = Some(value.clone());
                    if let Some((eng, _)) = registry::find_voice_any_engine(&value) {
                        config.default_engine = Some(eng);
                    }
                }
                "output_dir" => config.output_dir = Some(value.clone()),
                // Ducking keys
                "ducking" | "ducking.enabled" => {
                    let enabled = parse_bool(&value).ok_or_else(|| {
                        anyhow::anyhow!("Invalid value '{value}': use on, off, true, false, 1, 0")
                    })?;
                    config.ducking.get_or_insert_default().enabled = Some(enabled);
                }
                "ducking.level" => {
                    let level: f32 = value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid ducking level: {value}"))?;
                    if !(0.0..=1.0).contains(&level) {
                        bail!("Invalid ducking level {value}: must be between 0 and 1");
                    }
                    config.ducking.get_or_insert_default().level = Some(level);
                }
                "ducking.fade_ms" => {
                    let fade_ms: u64 = value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid ducking fade: {value}"))?;
                    if fade_ms > 5000 {
                        bail!("Invalid ducking fade {value}: must be at most 5000 ms");
                    }
                    config.ducking.get_or_insert_default().fade_ms = Some(fade_ms);
                }
                // Engine-specific keys
                "kokoro.speed" => {
                    let speed = parse_speed(&value)?;
                    config
                        .kokoro
                        .get_or_insert(config::KokoroConfig {
                            variant: None,
                            speed: None,
                            default_voice: None,
                        })
                        .speed = Some(speed);
                }
                "kokoro.default_voice" => {
                    config
                        .kokoro
                        .get_or_insert(config::KokoroConfig {
                            variant: None,
                            speed: None,
                            default_voice: None,
                        })
                        .default_voice = Some(value.clone());
                }
                "supertonic.speed" => {
                    let speed = parse_speed(&value)?;
                    config.supertonic.get_or_insert_default().speed = Some(speed);
                }
                "supertonic.steps" => {
                    let steps: u32 = value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid steps: {value}"))?;
                    config.supertonic.get_or_insert_default().steps = Some(steps);
                }
                _ => bail!(
                    "Unknown key '{key}'. Run 'local-voice config set --help' for valid keys."
                ),
            }
            config.save()?;
            println!("{}", format!("✓ Set {key} = {value}").green());
        }

        Some(ConfigAction::Paths) => {
            println!();
            println!("  {}", "Paths:".bold());
            println!("    Config:  {}", Config::path().display());
            println!("    Models:  {}", Config::models_dir().display());
            for kind in engine::EngineKind::all() {
                println!(
                    "      {:<12} {}",
                    format!("{kind}:"),
                    Config::models_dir().join(kind.as_str()).display()
                );
            }
            println!();
        }

        Some(ConfigAction::AutoDetect) => {
            let hw = hardware::HardwareProfile::detect();
            hw.display();

            let recommended = hw.recommended_engine();
            let variant = hw.recommended_variant(recommended);

            println!(
                "  Recommended: {} ({})",
                recommended.as_str().bold().green(),
                variant
            );
            println!();

            config.default_engine = Some(recommended);
            config.save()?;
            println!(
                "{}",
                format!("✓ Set default_engine = {recommended}").green()
            );
            println!();
            println!("  Install the recommended model:");
            println!("    local-voice models install {variant}");
            println!();
        }
    }

    Ok(())
}

fn handle_doctor() -> Result<()> {
    let hw = hardware::HardwareProfile::detect();
    hw.display();

    let recommended = hw.recommended_engine();
    let variant = hw.recommended_variant(recommended);

    println!(
        "  Recommended: {} (model: {})",
        recommended.as_str().bold().green(),
        variant
    );
    println!();
    println!("  {}", "Engines:".bold());

    for kind in engine::EngineKind::all() {
        let installed = Config::installed_models(Some(*kind));
        let rec = if *kind == recommended {
            "★ recommended"
        } else {
            "  available"
        };
        let status = if installed.is_empty() {
            "not installed".dimmed().to_string()
        } else {
            format!("{} model(s)", installed.len()).green().to_string()
        };

        println!(
            "    {:<12} {:<16} {:<16} {}",
            kind.as_str().bold(),
            rec,
            status,
            kind.description().dimmed()
        );
    }
    println!();

    // ── Ducking self-test ──
    println!("  {}", "Ducking:".bold());
    println!(
        "    {:<12} {} ({})",
        "backend",
        std::env::consts::OS,
        ducking::backend_name().dimmed()
    );
    let ducking = Config::load()
        .map(|c| c.ducking_settings())
        .unwrap_or_default();
    if ducking.enabled {
        println!(
            "    {:<12} on (level {}%, fade {}ms)",
            "config",
            (ducking.level * 100.0).round() as u32,
            ducking.fade_ms
        );
    } else {
        println!("    {:<12} {}", "config", "off".dimmed());
    }
    // Progress note on stderr (terminal only) so piped output stays clean;
    // erased once the probe is done.
    use std::io::IsTerminal as _;
    let show_progress = std::io::stderr().is_terminal();
    if show_progress {
        eprint!("    {:<12} testing (plays a short tone)…", "self-test");
    }
    let probe = ducking::probe();
    if show_progress {
        eprint!("\r\x1b[2K");
    }
    match &probe {
        ducking::ProbeResult::Ok => println!(
            "    {:<12} {}",
            "self-test",
            "✓ other apps were ducked and restored".green()
        ),
        ducking::ProbeResult::NothingPlaying => println!(
            "    {:<12} {}",
            "self-test",
            "⚠ backend works, but nothing else is playing so a full duck could not be verified"
                .yellow()
        ),
        ducking::ProbeResult::Unsupported(why) => {
            println!("    {:<12} {}", "self-test", "⚠ unsupported".yellow());
            println!("    {:<12} {}", "", why.dimmed());
        }
        ducking::ProbeResult::Failed(why) => {
            println!("    {:<12} {}", "self-test", "✗ failed".red());
            println!("    {:<12} {}", "", why.dimmed());
            if cfg!(target_os = "macos") {
                println!();
                println!("    {}", "To fix on macOS:".bold());
                println!(
                    "      1. Open System Settings → Privacy & Security → Screen & System Audio Recording"
                );
                println!(
                    "      2. Add the app that launches local-voice (Terminal, iTerm, Claude, VS Code…)"
                );
                println!("      3. Restart that app, then run `local-voice doctor` again");
            }
        }
    }
    println!();

    Ok(())
}

async fn interactive_mode() -> Result<()> {
    use dialoguer::{Input, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    println!();
    println!("  {} v{}", "local-voice".bold(), env!("CARGO_PKG_VERSION"));
    println!("  {}", "Local TTS — speak text with AI voices".dimmed());
    println!();

    let config = Config::load()?;
    let eng_name = config
        .default_engine
        .map(|e| e.as_str().to_string())
        .unwrap_or_else(|| "auto".into());
    let voice_name = config.default_voice.as_deref().unwrap_or("default");
    println!(
        "  engine: {}  voice: {}",
        eng_name.green(),
        voice_name.green()
    );
    println!();

    loop {
        let choices = &[
            "Speak text",
            "Change voice",
            "Change engine",
            "Change speed",
            "Install model",
            "Install voice",
            "Show config",
            "Exit",
        ];

        let selection = Select::with_theme(&theme)
            .with_prompt("What do you want to do?")
            .items(choices)
            .default(0)
            .interact()?;

        match selection {
            0 => {
                let text: String = Input::with_theme(&theme)
                    .with_prompt("Text to speak")
                    .interact_text()?;
                if !text.trim().is_empty() {
                    handle_speak(
                        &text,
                        None,
                        None,
                        None,
                        None,
                        None,
                        false,
                        SpeakDucking::default(),
                    )
                    .ok();
                }
            }
            1 => {
                let mut voice_options: Vec<String> = Vec::new();
                for kind in engine::EngineKind::all() {
                    for v in registry::voices_for_engine(*kind) {
                        voice_options.push(format!("{} — {} [{}]", v.id, v.name, kind));
                    }
                }
                for model_id in Config::installed_models(Some(engine::EngineKind::Piper)) {
                    voice_options.push(format!("{model_id} — Piper voice [piper]"));
                }
                if voice_options.is_empty() {
                    println!("  No voices available. Install a model first.");
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Select voice")
                    .items(&voice_options)
                    .default(0)
                    .interact()?;
                let voice_id = voice_options[sel].split(" — ").next().unwrap().to_string();
                let mut config = Config::load()?;
                config.default_voice = Some(voice_id.clone());
                if let Some((eng, _)) = registry::find_voice_any_engine(&voice_id) {
                    config.default_engine = Some(eng);
                }
                config.save()?;
                println!("  {}", format!("✓ Voice set to '{voice_id}'").green());
            }
            2 => {
                let engine_names: Vec<String> = engine::EngineKind::all()
                    .iter()
                    .map(|e| format!("{} — {}", e.as_str(), e.description()))
                    .collect();
                let sel = Select::with_theme(&theme)
                    .with_prompt("Select engine")
                    .items(&engine_names)
                    .default(0)
                    .interact()?;
                let eng = engine::EngineKind::all()[sel];
                let mut config = Config::load()?;
                config.default_engine = Some(eng);
                config.save()?;
                println!(
                    "  {}",
                    format!("✓ Engine set to '{}'", eng.as_str()).green()
                );
            }
            3 => {
                let config = Config::load()?;
                let current = match config.default_engine {
                    Some(engine::EngineKind::Supertonic) => config.supertonic_speed(),
                    _ => config.kokoro_speed(),
                };
                let speed_str: String = Input::with_theme(&theme)
                    .with_prompt("Speed (0.5=slow, 1.0=normal, 2.0=fast)")
                    .default(current.to_string())
                    .interact_text()?;
                match speed_str.parse::<f32>() {
                    Ok(speed) if speed > 0.0 => {
                        let mut config = Config::load()?;
                        let eng = config.default_engine.unwrap_or(engine::EngineKind::Kokoro);
                        match eng {
                            engine::EngineKind::Kokoro => {
                                config
                                    .kokoro
                                    .get_or_insert(config::KokoroConfig {
                                        variant: None,
                                        speed: None,
                                        default_voice: None,
                                    })
                                    .speed = Some(speed);
                            }
                            engine::EngineKind::Supertonic => {
                                config.supertonic.get_or_insert_default().speed = Some(speed);
                            }
                            _ => {}
                        }
                        config.save()?;
                        println!("  {}", format!("✓ Speed set to {speed}").green());
                    }
                    _ => println!("  Invalid speed value"),
                }
            }
            4 => {
                let installed = Config::installed_models(None);
                let models = registry::search_all(None, None);
                let model_options: Vec<String> = models
                    .iter()
                    .map(|m| {
                        let status = if installed.contains(&m.id.to_string()) {
                            " [installed]"
                        } else {
                            ""
                        };
                        format!("{} — {} ({}MB){status}", m.id, m.engine, m.size_mb)
                    })
                    .collect();
                let sel = Select::with_theme(&theme)
                    .with_prompt("Select model to install")
                    .items(&model_options)
                    .default(0)
                    .interact()?;
                let model_id = models[sel].id.to_string();
                if installed.contains(&model_id) {
                    println!("  Already installed.");
                } else {
                    handle_models(cli::ModelAction::Install { id: model_id }).await?;
                }
            }
            5 => {
                let mut voice_options: Vec<String> = Vec::new();
                let mut voice_ids: Vec<String> = Vec::new();
                for kind in engine::EngineKind::all() {
                    let models = Config::installed_models(Some(*kind));
                    let installed_voices: Vec<String> = models
                        .iter()
                        .flat_map(|m| Config::installed_voices(*kind, m))
                        .collect();
                    for v in registry::voices_for_engine(*kind) {
                        let status = if installed_voices.contains(&v.id.to_string()) {
                            " [installed]"
                        } else {
                            ""
                        };
                        voice_options.push(format!("{} — {} ({}){status}", v.id, v.name, kind));
                        voice_ids.push(v.id.to_string());
                    }
                }
                if voice_options.is_empty() {
                    println!("  No voices available. Install a model first.");
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Select voice to install")
                    .items(&voice_options)
                    .default(0)
                    .interact()?;
                handle_voices(Some(cli::VoiceAction::Install {
                    id: voice_ids[sel].clone(),
                }))
                .await?;
            }
            6 => handle_config(None)?,
            _ => {
                println!("  Bye!");
                break;
            }
        }
        println!();
    }

    Ok(())
}
