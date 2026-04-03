use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "local-voice",
    version,
    about = "Local TTS CLI with MCP server — run text-to-speech models locally",
    long_about = "Install and run open-source TTS models locally.\nExpose them to AI agents via MCP (Model Context Protocol)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage TTS models
    Models {
        #[command(subcommand)]
        action: ModelAction,
    },

    /// List available voices for installed models
    Voices,

    /// Speak text using a local TTS model
    Speak {
        /// Text to speak
        text: String,

        /// Voice/model to use (e.g. en_US-lessac-medium)
        #[arg(short, long)]
        voice: Option<String>,

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
}

#[derive(Subcommand)]
pub enum ModelAction {
    /// List available and installed models
    List {
        /// Filter by language (e.g. en, de, fr)
        #[arg(short, long)]
        language: Option<String>,
    },

    /// Install a model by ID
    Install {
        /// Model ID (e.g. en_US-lessac-medium)
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
        /// Config key (default_voice, output_dir)
        key: String,
        /// Config value
        value: String,
    },

    /// Show model and config file paths
    Paths,
}
