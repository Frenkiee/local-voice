use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "local-voice",
    version,
    about = "Local TTS CLI with MCP server — run text-to-speech models locally",
    long_about = "Install and run open-source TTS models locally.\nMultiple engines: Kokoro (recommended), Piper (lightweight), Chatterbox (voice cloning).\nExpose them to AI agents via MCP (Model Context Protocol)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// List available TTS engines and recommendations
    Engines {
        #[command(subcommand)]
        action: Option<EngineAction>,
    },

    /// Manage TTS models
    Models {
        #[command(subcommand)]
        action: ModelAction,
    },

    /// List installed voices
    Voices,

    /// Speak text using a local TTS model
    Speak {
        /// Text to speak
        text: String,

        /// Voice to use (e.g. af_alloy for Kokoro, en_US-lessac-medium for Piper)
        #[arg(short, long)]
        voice: Option<String>,

        /// TTS engine override (kokoro, piper, chatterbox)
        #[arg(short, long)]
        engine: Option<String>,

        /// Speech speed multiplier (Kokoro only, default 1.0)
        #[arg(long)]
        speed: Option<f32>,

        /// Save audio to file instead of playing
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Don't play audio, only save to file
        #[arg(long, default_value_t = false)]
        no_play: bool,
    },

    /// Start MCP server (JSON-RPC over stdio)
    Serve,

    /// Show or modify configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Show hardware info and recommended engine
    Doctor,
}

#[derive(Subcommand)]
pub enum EngineAction {
    /// List all engines with status
    List,
    /// Show detailed info about an engine
    Info {
        /// Engine name (kokoro, piper, chatterbox)
        engine: String,
    },
}

#[derive(Subcommand)]
pub enum ModelAction {
    /// List available and installed models
    List {
        /// Filter by language (e.g. en, de, fr)
        #[arg(short, long)]
        language: Option<String>,
        /// Filter by engine (kokoro, piper, chatterbox)
        #[arg(short, long)]
        engine: Option<String>,
    },

    /// Install a model by ID
    Install {
        /// Model ID (e.g. kokoro-q8f16, en_US-lessac-medium)
        id: String,
    },

    /// Remove an installed model
    Remove {
        /// Model ID
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current configuration
    Show,

    /// Set a configuration value
    Set {
        /// Config key (default_voice, default_engine, output_dir, kokoro.speed, kokoro.default_voice)
        key: String,
        /// Config value
        value: String,
    },

    /// Show model and config file paths
    Paths,

    /// Auto-detect hardware and set recommended engine
    AutoDetect,
}
