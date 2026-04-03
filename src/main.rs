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
use cli::{Cli, Commands, ConfigAction, ModelAction};
use config::Config;
use engine::TtsEngine;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Models { action } => handle_models(action).await?,
        Commands::Voices => handle_voices()?,
        Commands::Speak {
            text,
            voice,
            output,
            no_play,
        } => handle_speak(&text, voice.as_deref(), output.as_deref(), no_play)?,
        Commands::Serve => mcp::run_server()?,
        Commands::Config { action } => handle_config(action)?,
    }

    Ok(())
}

async fn handle_models(action: ModelAction) -> Result<()> {
    match action {
        ModelAction::List { language } => {
            let models = registry::search_models(language.as_deref());
            let installed = Config::installed_models();

            println!();
            println!(
                "  {:<26} {:<8} {:<8} {:<6} {}",
                "MODEL", "LANG", "QUALITY", "SIZE", "STATUS"
            );
            println!("  {}", "─".repeat(70));

            for model in models {
                let status = if installed.contains(&model.id.to_string()) {
                    "✓ installed"
                } else {
                    ""
                };

                println!(
                    "  {:<26} {:<8} {:<8} {:>4}MB {}",
                    model.id, model.language, model.quality, model.size_mb, status
                );
            }
            println!();

            if installed.is_empty() {
                println!("  Install a model:");
                println!("    local-voice models install en_US-lessac-medium");
                println!();
            }
        }

        ModelAction::Install { id } => {
            let entry = registry::find_model(&id);
            let entry = match entry {
                Some(e) => e,
                None => bail!(
                    "Unknown model '{id}'. Run 'local-voice models list' to see available models."
                ),
            };

            if Config::is_model_installed(&id) {
                println!("Model '{id}' is already installed.");
                return Ok(());
            }

            let model_dir = Config::model_path(&id);
            let onnx_url = entry.onnx_url();
            let config_url = entry.config_url();

            println!("Installing {} ({})...", entry.name, entry.id);
            println!();

            println!("  Downloading model...");
            download::download_file(&onnx_url, &model_dir.join("model.onnx")).await?;

            println!("  Downloading config...");
            download::download_file(&config_url, &model_dir.join("model.onnx.json")).await?;

            println!();
            println!("✓ Model '{id}' installed successfully.");
            println!();
            println!("  Try it: local-voice speak 'Hello, world!' --voice {id}");

            // Set as default if it's the first model
            let mut config = Config::load()?;
            if config.default_voice.is_none() {
                config.default_voice = Some(id.clone());
                config.save()?;
                println!("  Set as default voice.");
            }
            println!();
        }

        ModelAction::Remove { id } => {
            if !Config::is_model_installed(&id) {
                bail!("Model '{id}' is not installed.");
            }

            let model_dir = Config::model_path(&id);
            std::fs::remove_dir_all(&model_dir)?;
            println!("✓ Model '{id}' removed.");

            // Clear default if it was this model
            let mut config = Config::load()?;
            if config.default_voice.as_deref() == Some(&id) {
                config.default_voice = None;
                config.save()?;
            }
        }
    }

    Ok(())
}

fn handle_voices() -> Result<()> {
    let installed = Config::installed_models();

    if installed.is_empty() {
        println!("No voices installed.");
        println!();
        println!("Install one:");
        println!("  local-voice models install en_US-lessac-medium");
        return Ok(());
    }

    let config = Config::load()?;
    let default = config.default_voice.as_deref();

    println!();
    println!("  Installed voices:");
    println!();
    for voice in &installed {
        let marker = if Some(voice.as_str()) == default {
            " (default)"
        } else {
            ""
        };
        let info = registry::find_model(voice)
            .map(|m| format!(" — {}", m.description))
            .unwrap_or_default();
        println!("    {voice}{marker}{info}");
    }
    println!();

    Ok(())
}

fn handle_speak(
    text: &str,
    voice: Option<&str>,
    output: Option<&std::path::Path>,
    no_play: bool,
) -> Result<()> {
    let config = Config::load()?;
    let voice_id = config
        .resolve_voice(voice)
        .ok_or_else(|| anyhow::anyhow!("No voice configured. Install a model first."))?;

    if !Config::is_model_installed(&voice_id) {
        bail!("Voice '{voice_id}' is not installed. Run: local-voice models install {voice_id}");
    }

    let model_dir = Config::model_path(&voice_id);
    let mut engine = engine::piper::PiperEngine::load(&model_dir, &voice_id)?;

    eprintln!("Speaking with voice '{voice_id}'...");
    let audio_output = engine.synthesize(text)?;

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
            println!("  Configuration:");
            println!(
                "    default_voice: {}",
                config
                    .default_voice
                    .as_deref()
                    .unwrap_or("(not set)")
            );
            println!(
                "    output_dir:    {}",
                config.output_dir.as_deref().unwrap_or("(not set)")
            );
            println!();
        }

        Some(ConfigAction::Set { key, value }) => {
            match key.as_str() {
                "default_voice" => config.default_voice = Some(value.clone()),
                "output_dir" => config.output_dir = Some(value.clone()),
                _ => bail!("Unknown config key '{key}'. Valid keys: default_voice, output_dir"),
            }
            config.save()?;
            println!("✓ Set {key} = {value}");
        }

        Some(ConfigAction::Paths) => {
            println!();
            println!("  Paths:");
            println!("    Config:  {}", Config::path().display());
            println!("    Models:  {}", Config::models_dir().display());
            println!();
        }
    }

    Ok(())
}
