# local-voice

**Local text-to-speech for your terminal and AI agents.**

Run open-source TTS models 100% offline. Speak text from the command line, expose voices to Claude and other AI agents via MCP, or use the interactive mode to browse and configure engines.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.85+-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/Platforms-macOS%20%7C%20Linux%20%7C%20Windows-green.svg)](#installation)

## Features

- **4 TTS engines** with different strengths:

  | Engine | Params | Quality | Speed | Languages |
  |--------|--------|---------|-------|-----------|
  | **Kokoro** | 82M | Near-human | Fast | Multi (26 voices) |
  | **Supertonic** | 66M | High | 167x realtime | en, ko, es, pt, fr |
  | **Piper** | ~15M | Good | Fastest | 20+ languages |
  | **Chatterbox** | 500M | Best | Slow | en (voice cloning) |

- **MCP server** for Claude Desktop, Claude Code, and any MCP-compatible agent
- **Interactive mode** with menu-driven voice/engine selection
- **Cross-platform** — macOS, Linux (apt/dnf/pacman/zypper/apk/nix), Windows (choco/scoop/winget)
- **Fully offline** — no cloud, no API keys, no data leaves your machine

## Quick Start

```bash
git clone https://github.com/Frenkiee/local-voice.git
cd local-voice
make install
```

This installs dependencies (Rust, espeak-ng), builds the binary, and configures the MCP server for Claude.

Then:

```bash
# Install a model
local-voice models install kokoro-q8f16      # 114 MB, recommended
local-voice models install supertonic        # 263 MB, fastest

# Speak
local-voice speak "Hello world"

# Interactive mode
local-voice
```

## Installation

### One-line install (macOS / Linux / Windows)

```bash
make install
```

The Makefile auto-detects your OS and package manager:

| OS | Package Manager | What gets installed |
|----|-----------------|---------------------|
| macOS | Homebrew / MacPorts | `espeak-ng`, Rust via rustup |
| Linux | apt / dnf / pacman / zypper / apk / nix | `espeak-ng`, Rust via rustup |
| Windows | choco / scoop / winget | `espeak-ng`, Rust via rustup |

The binary is installed to `/usr/local/bin/local-voice` (or `%USERPROFILE%\.local-voice\bin` on Windows).

### Manual install

```bash
# Prerequisites: Rust (1.85+), espeak-ng
cargo build --release
cp target/release/local-voice /usr/local/bin/
```

### Uninstall

```bash
make uninstall
```

## Usage

### Speaking text

```bash
local-voice speak "Hello, how are you today?"
local-voice speak "Fast speech" --speed 1.5
local-voice speak "Different voice" --voice F1
local-voice speak "Save to file" --output hello.wav
local-voice speak "Kokoro voice" --voice af_alloy -e kokoro
```

### Managing models

```bash
local-voice models list                    # show all available models
local-voice models install kokoro-q8f16    # download and install
local-voice models install supertonic      # install Supertonic engine
local-voice models default supertonic      # set as default
local-voice models remove kokoro-fp32      # remove a model
```

### Managing voices

Engines like Kokoro (26 voices) and Supertonic (10 voices) support multiple voices per model:

```bash
local-voice voices list                    # show all voices
local-voice voices list -e kokoro          # filter by engine
local-voice voices install bf_emma         # install a Kokoro voice
local-voice voices install M2              # install a Supertonic voice
local-voice voices default F1              # set default voice
```

### Configuration

```bash
local-voice config show                    # view current settings
local-voice config set speed 1.2           # set speech speed
local-voice config set engine supertonic   # set default engine
local-voice config set voice af_alloy      # set default voice
local-voice config set model kokoro-q8f16  # set default model
local-voice config set steps 10            # supertonic denoising steps
local-voice config paths                   # show file locations
local-voice config auto-detect             # auto-pick best engine for your hardware
```

### Interactive mode

Run `local-voice` with no arguments for a menu-driven interface:

```
$ local-voice

  local-voice v0.1.0
  Local TTS — speak text with AI voices

  engine: supertonic  voice: F1

? What do you want to do?
> Speak text
  Change voice
  Change engine
  Change speed
  Install model
  Install voice
  Show config
  Exit
```

### Hardware diagnostics

```bash
local-voice doctor                         # show hardware + recommendations
```

## MCP Server

local-voice includes a built-in [MCP](https://modelcontextprotocol.io/) server that lets AI agents speak text aloud.

### Setup

`make install` automatically configures MCP for both Claude Desktop and Claude Code. To configure manually:

**Claude Desktop** (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "local-voice": {
      "command": "local-voice",
      "args": ["serve"]
    }
  }
}
```

**Claude Code** (`~/.claude/settings.json`):

```json
{
  "mcpServers": {
    "local-voice": {
      "command": "local-voice",
      "args": ["serve"]
    }
  }
}
```

### MCP Tools

| Tool | Description |
|------|-------------|
| `speak` | Speak text aloud (uses configured engine/voice/speed) |
| `set_config` | Change engine, model, voice, or speed |
| `get_config` | View current TTS configuration |
| `list_engines` | List available TTS engines |
| `list_models` | List available and installed models |
| `list_voices` | List available and installed voices |

### Teaching Your Agent to Speak

To make Claude (or any MCP-compatible agent) use local-voice proactively, add a memory/instruction like this:

```markdown
Use `mcp__local-voice__speak` throughout your workflow:

1. **Before starting a task** — quick notice of what you're about to do
2. **When dispatching agents** — say how many agents and what they're doing
3. **When an agent finishes** — brief result summary
4. **When a task is complete** — explain what was done in 1-2 sentences
5. **When user needs to take action** — restart server, rebuild, install deps, etc.

Keep it short and informative — 1-2 sentences max per call.
```

For **Claude Code**, save this as a memory file at:
```
~/.claude/projects/<your-project>/memory/feedback_speak.md
```

With frontmatter:
```yaml
---
name: Speak when done
description: Use TTS to announce task starts, agent dispatches, and completions
type: feedback
---
```

For **Claude Desktop**, add the instruction to your system prompt or project instructions.

This turns your agent into a voice-narrated assistant — you can step away from the screen and still know what's happening.

## Engines

### Kokoro

82M parameter model with 26 high-quality voices. Near-human speech quality, CPU-friendly. Uses espeak-ng for phonemization.

```bash
local-voice models install kokoro-q8f16    # 114 MB (recommended)
local-voice models install kokoro-fp16     # 163 MB
local-voice models install kokoro-fp32     # 326 MB (highest quality)
```

Voices: `af_alloy`, `af_nova`, `am_adam`, `am_echo`, `bf_alice`, `bm_daniel`, and 20 more.

### Supertonic

66M parameter model by Supertone. Generates speech at 167x realtime on Apple Silicon. Flow-matching architecture with 4-model ONNX pipeline. No phonemizer needed.

```bash
local-voice models install supertonic      # 263 MB
```

Voices: `F1`-`F5` (female), `M1`-`M5` (male). Languages: English, Korean, Spanish, Portuguese, French.

### Piper

Lightweight models (~15-100MB) that run anywhere. One model per voice/language.

```bash
local-voice models install en_US-lessac-medium   # 65 MB
local-voice models install de_DE-thorsten-medium  # 55 MB
```

20+ languages including English, German, French, Spanish, Italian, Portuguese, Russian, Chinese, and more.

### Chatterbox

500M parameter model with voice cloning capability. Best quality but slower, prefers GPU.

```bash
local-voice models install chatterbox-quantized   # 580 MB
```

## Development

```bash
git clone https://github.com/Frenkiee/local-voice.git
cd local-voice
make deps          # install espeak-ng + Rust
cargo build        # debug build
cargo build --release  # release build
```

### Project structure

```
src/
  main.rs           CLI handlers and interactive mode
  cli.rs            Clap command definitions
  config.rs         TOML configuration management
  mcp.rs            MCP server (JSON-RPC over stdio)
  audio.rs          Audio playback (rodio) and WAV saving
  download.rs       Model downloading with progress bars
  hardware.rs       Hardware detection and engine recommendations
  phonemize.rs      espeak-ng wrapper for phonemization
  engine/
    mod.rs          TtsEngine trait and EngineKind enum
    kokoro.rs       Kokoro ONNX inference
    supertonic.rs   Supertonic 4-model pipeline
    piper.rs        Piper ONNX inference
    chatterbox.rs   Chatterbox multi-session inference
  registry/
    mod.rs          EngineRegistry trait, cross-engine lookups
    kokoro.rs       Kokoro models/voices/download URLs
    supertonic.rs   Supertonic models/voices/download URLs
    piper.rs        Piper models/download URLs
    chatterbox.rs   Chatterbox models/download URLs
```

## License

[MIT](LICENSE)
