mod audio;
mod cli;
mod config;
mod download;
mod engine;
mod hardware;
mod mcp;
mod phonemize;
mod registry;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{Cli, Commands, ConfigAction, EngineAction, ModelAction};
use config::Config;
use engine::TtsEngine;
use owo_colors::OwoColorize;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Engines { action } => handle_engines(action)?,
        Commands::Models { action } => handle_models(action).await?,
        Commands::Voices => handle_voices()?,
        Commands::Speak {
            text,
            voice,
            engine,
            speed,
            output,
            no_play,
        } => handle_speak(
            &text,
            voice.as_deref(),
            engine.as_deref(),
            speed,
            output.as_deref(),
            no_play,
        )?,
        Commands::Serve => mcp::run_server()?,
        Commands::Config { action } => handle_config(action)?,
        Commands::Doctor => handle_doctor()?,
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

            if kind == engine::EngineKind::Kokoro {
                println!();
                println!("  Available voices (26):");
                for voice in registry::voices_for_engine(kind) {
                    println!(
                        "    {:<16} {} ({})",
                        voice.id, voice.name, voice.gender
                    );
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
            println!("{}", format!("✓ Model '{id}' installed successfully.").green());
            println!();

            if engine_kind == engine::EngineKind::Kokoro {
                println!("  Try it: local-voice speak 'Hello, world!'");
            } else {
                println!("  Try it: local-voice speak 'Hello, world!' --voice {id}");
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
                .or_else(|| {
                    registry::find_model_any_engine(&id).map(|(e, _)| e)
                })
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
        }
    }

    Ok(())
}

fn handle_voices() -> Result<()> {
    let installed = Config::installed_models(None);

    if installed.is_empty() {
        println!("No voices installed.");
        println!();
        println!("Install one:");
        println!("  local-voice models install kokoro-q8f16");
        return Ok(());
    }

    let config = Config::load()?;
    let default = config.default_voice.as_deref();

    println!();
    println!("  {}", "Installed voices:".bold());
    println!();
    for voice in &installed {
        let marker = if Some(voice.as_str()) == default {
            " (default)".green().to_string()
        } else {
            String::new()
        };
        let info = registry::find_model_any_engine(voice)
            .map(|(engine_kind, m)| format!(" [{}] — {}", engine_kind, m.description))
            .unwrap_or_default();
        println!("    {voice}{marker}{info}");
    }

    // Show Kokoro voices if a Kokoro model is installed
    let kokoro_installed = !Config::installed_models(Some(engine::EngineKind::Kokoro)).is_empty();
    if kokoro_installed {
        println!();
        println!("  {} (use with --voice):", "Kokoro voices".bold());
        let voices = registry::voices_for_engine(engine::EngineKind::Kokoro);
        for voice in voices {
            println!("    {:<16} {} ({})", voice.id, voice.name, voice.gender);
        }
    }
    println!();

    Ok(())
}

fn handle_speak(
    text: &str,
    voice: Option<&str>,
    engine_name: Option<&str>,
    speed: Option<f32>,
    output: Option<&std::path::Path>,
    no_play: bool,
) -> Result<()> {
    let config = Config::load()?;

    let engine_kind = match engine_name {
        Some(e) => e.parse::<engine::EngineKind>()?,
        None => config
            .default_engine
            .unwrap_or_else(|| hardware::HardwareProfile::detect().recommended_engine()),
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
            let kokoro_models = Config::installed_models(Some(engine::EngineKind::Kokoro));
            let model_id = kokoro_models.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "No Kokoro model installed. Run: local-voice models install kokoro-q8f16"
                )
            })?;

            let kokoro_voice = voice.unwrap_or(config.kokoro_voice());
            let spd = speed.unwrap_or(config.kokoro_speed());
            let model_dir =
                Config::resolve_model_path(engine::EngineKind::Kokoro, model_id);

            eprintln!(
                "Speaking with Kokoro voice '{kokoro_voice}' (model: {model_id})..."
            );

            Box::new(engine::kokoro::KokoroEngine::load(
                &model_dir,
                model_id,
                kokoro_voice,
                spd,
            )?)
        }
        engine::EngineKind::Chatterbox => {
            bail!("Chatterbox engine not yet implemented. Use --engine piper for now.")
        }
    };

    let audio_output = tts.synthesize(text)?;

    if let Some(path) = output {
        audio::save_wav(&audio_output, path)?;
        eprintln!("✓ Saved to {}", path.display());
    }

    if !no_play {
        audio::play_audio(&audio_output)?;
    }

    Ok(())
}

fn handle_config(action: Option<ConfigAction>) -> Result<()> {
    let mut config = Config::load()?;

    match action {
        None | Some(ConfigAction::Show) => {
            println!();
            println!("  {}", "Configuration:".bold());
            println!(
                "    default_engine: {}",
                config
                    .default_engine
                    .map(|e| e.as_str().to_string())
                    .unwrap_or_else(|| "(auto-detect)".dimmed().to_string())
            );
            println!(
                "    default_voice:  {}",
                config.default_voice.as_deref().unwrap_or("(not set)")
            );
            println!(
                "    output_dir:     {}",
                config.output_dir.as_deref().unwrap_or("(not set)")
            );
            if let Some(ref kokoro) = config.kokoro {
                println!();
                println!("  {}", "Kokoro:".bold());
                println!(
                    "    speed:         {}",
                    kokoro.speed.map(|s| s.to_string()).unwrap_or("1.0".into())
                );
                println!(
                    "    default_voice: {}",
                    kokoro.default_voice.as_deref().unwrap_or("af_alloy")
                );
            }
            println!();
        }

        Some(ConfigAction::Set { key, value }) => {
            match key.as_str() {
                "default_voice" => config.default_voice = Some(value.clone()),
                "default_engine" => {
                    let engine: engine::EngineKind = value.parse()?;
                    config.default_engine = Some(engine);
                }
                "output_dir" => config.output_dir = Some(value.clone()),
                "kokoro.speed" => {
                    let speed: f32 = value.parse().map_err(|_| anyhow::anyhow!("Invalid speed value"))?;
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
                _ => bail!(
                    "Unknown config key '{key}'. Valid keys: default_voice, default_engine, output_dir, kokoro.speed, kokoro.default_voice"
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
            println!(
                "  Install the recommended model:"
            );
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

    Ok(())
}
