use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "local-voice",
    version,
    about = "Local TTS CLI with MCP server — run text-to-speech models locally",
    long_about = "\
\x1b[1mlocal-voice\x1b[0m — Run text-to-speech models locally

  Engines:  Kokoro · Piper · Chatterbox · Supertonic
  Protocol: MCP (Model Context Protocol) for AI agents
  Privacy:  100% offline, no cloud, no API keys",
    after_help = "\x1b[1mQuick start:\x1b[0m
  local-voice speak \"Hello world\"                  Speak with default voice
  local-voice speak \"Hi\" --voice F1 --speed 1.2    Override voice and speed
  local-voice models install supertonic-3           Install a model
  local-voice voices list                           Browse available voices
  local-voice voices default af_alloy               Set default voice
  local-voice config show                           View current settings
  local-voice serve                                 Start MCP server
  local-voice                                       Interactive mode"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// List available TTS engines and recommendations
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice engines                    List all engines with status
  local-voice engines info kokoro        Detailed info about an engine")]
    Engines {
        #[command(subcommand)]
        action: Option<EngineAction>,
    },

    /// Manage TTS models — list, install, remove, set default
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice models list                List all available models
  local-voice models list -e kokoro      Filter by engine
  local-voice models install supertonic-3  Install a model
  local-voice models default kokoro-q8f16  Set default model
  local-voice models remove kokoro-fp32  Remove a model")]
    Models {
        #[command(subcommand)]
        action: ModelAction,
    },

    /// Manage voices — list, install, remove, set default
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice voices                     List all voices
  local-voice voices list -e kokoro      Filter by engine
  local-voice voices install bf_emma     Install a Kokoro voice
  local-voice voices install M2          Install a Supertonic voice
  local-voice voices default F1          Set default voice")]
    Voices {
        #[command(subcommand)]
        action: Option<VoiceAction>,
    },

    /// Speak text aloud using the configured TTS engine
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice speak \"Hello world\"                  Use defaults
  local-voice speak \"Hi\" --voice af_alloy          Override voice
  local-voice speak \"Hi\" -e supertonic --speed 1.2 Override engine and speed
  local-voice speak \"Dober dan\" -l sl              Slovenian (Supertonic 3)
  local-voice speak \"Hi\" -o output.wav             Save to file
  local-voice speak \"Hi\" -o out.wav --no-play      Save only, don't play
  local-voice speak \"Hi\" --no-ducking              Don't lower other apps' volume")]
    Speak {
        /// Text to speak
        text: String,

        /// Voice ID (e.g. af_alloy, F1, M2, en_US-lessac-medium)
        #[arg(short, long)]
        voice: Option<String>,

        /// TTS engine (kokoro, piper, chatterbox, supertonic)
        #[arg(short, long)]
        engine: Option<String>,

        /// Speech speed multiplier (default: 1.0 kokoro, 1.05 supertonic)
        #[arg(short, long)]
        speed: Option<f32>,

        /// Language code for multilingual engines (Supertonic 2/3: en, sl, de, ja, ...)
        #[arg(short, long)]
        language: Option<String>,

        /// Save audio to WAV file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Don't play audio (only useful with --output)
        #[arg(long, default_value_t = false)]
        no_play: bool,

        /// Don't duck other apps' audio while speaking (overrides config for this run)
        #[arg(long, default_value_t = false)]
        no_ducking: bool,

        /// Volume other apps are ducked to while speaking, 0..1 (default: 0.1 = 10%)
        #[arg(long, value_name = "0..1", conflicts_with = "no_ducking")]
        ducking_level: Option<f32>,
    },

    /// Start MCP server (JSON-RPC over stdio)
    #[command(after_help = "\x1b[1mUsage:\x1b[0m
  Add to Claude Desktop config or .claude/settings.json:
  { \"command\": \"local-voice\", \"args\": [\"serve\"] }")]
    Serve,

    /// Show or modify configuration
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice config                     Show current config
  local-voice config set speed 1.2       Set speech speed
  local-voice config set engine kokoro   Set default engine
  local-voice config set voice af_alloy  Set default voice
  local-voice config set model supertonic-3  Set default model
  local-voice config set ducking off     Stop lowering other apps' audio
  local-voice config paths               Show file paths
  local-voice config auto-detect         Auto-detect best engine")]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Diagnose hardware and recommend the best engine
    #[command(after_help = "\x1b[1mExamples:\x1b[0m
  local-voice doctor                     Show hardware + recommendations")]
    Doctor,
}

#[derive(Subcommand)]
pub enum EngineAction {
    /// List all engines with install status
    List,
    /// Show detailed info about an engine (models, voices)
    Info {
        /// Engine name (kokoro, piper, chatterbox, supertonic)
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
        /// Filter by engine (kokoro, piper, chatterbox, supertonic)
        #[arg(short, long)]
        engine: Option<String>,
    },

    /// Download and install a model
    Install {
        /// Model ID (e.g. kokoro-q8f16, supertonic-3, en_US-lessac-medium)
        id: String,
    },

    /// Remove an installed model
    Remove {
        /// Model ID
        id: String,
    },

    /// Set the default model (also sets engine)
    Default {
        /// Model ID (e.g. kokoro-q8f16, supertonic-3)
        id: String,
    },
}

#[derive(Subcommand)]
pub enum VoiceAction {
    /// List available and installed voices
    List {
        /// Filter by engine (kokoro, supertonic)
        #[arg(short, long)]
        engine: Option<String>,
    },

    /// Download and install a voice
    Install {
        /// Voice ID (e.g. af_alloy, bf_emma, M1, F2)
        id: String,
    },

    /// Remove an installed voice
    Remove {
        /// Voice ID
        id: String,
    },

    /// Set the default voice (also sets engine)
    Default {
        /// Voice ID (e.g. af_alloy, F1, en_US-lessac-medium)
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current configuration
    Show,

    /// Set a configuration value
    #[command(after_help = "\x1b[1mKeys:\x1b[0m
  speed       Speech speed (routes to active engine)
  steps       Denoising steps (supertonic only)
  language    Language code (supertonic-2/3 only, e.g. en, sl, de)
  engine      Default engine (kokoro, piper, chatterbox, supertonic)
  model       Default model (e.g. kokoro-q8f16)
  voice       Default voice (e.g. af_alloy, F1)
  output_dir  Default output directory for WAV files
  ducking     Lower other apps' audio while speaking (on, off)

\x1b[1mDucking keys:\x1b[0m
  ducking.level           Volume other apps are ducked to, 0..1 (default 0.1)
  ducking.fade_ms         Fade in/out duration in ms, max 5000 (default 300)

\x1b[1mEngine-specific keys:\x1b[0m
  kokoro.speed            Kokoro speech speed
  kokoro.default_voice    Kokoro default voice
  supertonic.speed        Supertonic speech speed
  supertonic.steps        Supertonic denoising steps
  supertonic.language     Supertonic language code (default en)")]
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },

    /// Show config and model file paths
    Paths,

    /// Auto-detect hardware and set recommended engine
    AutoDetect,
}
