# local-voice

**Local text-to-speech for your terminal and AI agents. Zero latency, zero cloud.**

Run open-source TTS models 100% offline. Speak from the CLI, let Claude narrate its work via MCP, or browse voices in interactive mode. All processing stays on your machine.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Frenkiee/local-voice/actions/workflows/ci.yml/badge.svg)](https://github.com/Frenkiee/local-voice/actions/workflows/ci.yml)
[![Release](https://github.com/Frenkiee/local-voice/releases/latest/badge.svg)](https://github.com/Frenkiee/local-voice/releases/latest)

## Features

| Engine | Params | Quality | Speed | Languages |
|--------|--------|---------|-------|-----------|
| **Supertonic** | 66M | High | 167x realtime | en, ko, es, pt, fr |
| **Kokoro** | 82M | Near-human | Fast | Multi (26 voices) |
| **Piper** | ~15M | Good | Fastest | 20+ languages |
| **Chatterbox** | 500M | Best | Slow | en (voice cloning) |

- **MCP server** — 6 tools for Claude Desktop, Claude Code, and any MCP-compatible agent
- **Audio queue** — speak requests return instantly, audio plays sequentially in the background
- **Interactive mode** — menu-driven voice/engine/speed selection
- **Cross-platform** — macOS, Linux (apt/dnf/pacman/zypper/apk/nix), Windows (choco/scoop/winget)

---

## Installation

### From source (recommended)

```bash
git clone https://github.com/Frenkiee/local-voice.git
cd local-voice
make install
```

This single command:
1. Installs dependencies (Rust, espeak-ng) via your package manager
2. Builds the release binary → `/usr/local/bin/local-voice`
3. Configures MCP server globally for **Claude Desktop** and **Claude Code**
4. Downloads the **Supertonic** model (~263 MB)
5. Sets default voice to **F2** at **1.1x** speed

| OS | Package Managers Supported |
|----|---------------------------|
| macOS | Homebrew, MacPorts |
| Linux | apt, dnf, pacman, zypper, apk, nix |
| Windows | choco, scoop, winget |

### From GitHub releases

Download a pre-built binary from the [latest release](https://github.com/Frenkiee/local-voice/releases/latest):

```bash
# macOS (Apple Silicon)
curl -L https://github.com/Frenkiee/local-voice/releases/latest/download/local-voice-macos-arm64.tar.gz | tar xz
sudo mv local-voice /usr/local/bin/

# Linux (x86_64)
curl -L https://github.com/Frenkiee/local-voice/releases/latest/download/local-voice-linux-x86_64.tar.gz | tar xz
sudo mv local-voice /usr/local/bin/

# Windows (x86_64) — download .zip from releases page
```

> **Note:** Pre-built binaries require `espeak-ng` installed separately. On macOS: `brew install espeak-ng`, on Linux: `sudo apt install espeak-ng`.

Then install a model and configure MCP manually (see [MCP Server](#mcp-server) below).

### Manual build

```bash
# Prerequisites: Rust 1.85+, espeak-ng
cargo build --release
sudo cp target/release/local-voice /usr/local/bin/
```

### Uninstall

```bash
make uninstall    # removes binary + MCP config
```

---

## Teaching Your Agent to Speak

After installation, the MCP server is ready. To make Claude use it **proactively** (not just when asked), add a memory file:

**Claude Code** — save to `~/.claude/projects/<your-project>/memory/feedback_speak.md`:

```markdown
---
name: Speak when done
description: Use TTS to announce task starts, agent dispatches, and completions
type: feedback
---

Use `mcp__local-voice__speak` throughout your workflow:

1. **Before starting a task** — quick notice of what you're about to do
2. **When dispatching agents** — say how many agents and what they're doing
3. **When an agent finishes** — brief result summary
4. **When a task is complete** — explain what was done in 1-2 sentences
5. **When user needs to take action** — restart server, rebuild, install deps, etc.

Keep it short — 1-2 sentences max per call.
```

**Claude Desktop** — add the instruction to your system prompt or project instructions.

This turns Claude into a voice-narrated assistant. Step away from the screen and still know what's happening.

---

## Usage

### Speak

```bash
local-voice speak "Hello, how are you today?"
local-voice speak "Fast speech" -s 1.5
local-voice speak "Different voice" --voice F1
local-voice speak "Save to file" -o hello.wav
local-voice speak "Kokoro voice" --voice af_alloy -e kokoro
```

### Models

```bash
local-voice models list                    # browse all available models
local-voice models install kokoro-q8f16    # download and install
local-voice models default supertonic      # set as default (also sets engine)
local-voice models remove kokoro-fp32      # remove a model
```

### Voices

Kokoro has 26 voices, Supertonic has 10. Each is a small download on top of the base model:

```bash
local-voice voices list                    # show all voices with install status
local-voice voices list -e kokoro          # filter by engine
local-voice voices install bf_emma         # install a Kokoro voice (~1 MB)
local-voice voices install M2              # install a Supertonic voice (~420 KB)
local-voice voices default F1              # set default (also sets engine)
```

### Config

```bash
local-voice config show                    # view current settings
local-voice config set speed 1.2           # set speech speed (auto-routes to active engine)
local-voice config set steps 10            # supertonic denoising steps
local-voice config set engine kokoro       # switch engine
local-voice config set voice af_alloy      # switch voice (auto-sets engine)
local-voice config set model kokoro-q8f16  # switch model (auto-sets engine)
local-voice config paths                   # show config + model file locations
local-voice config auto-detect             # pick best engine for your hardware
```

### Interactive mode

Run with no arguments:

```
$ local-voice

  local-voice v0.1.0
  Local TTS — speak text with AI voices

  engine: supertonic  voice: F2

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

### Doctor

```bash
local-voice doctor                         # hardware profile + engine recommendations
```

---

## MCP Server

local-voice includes a built-in [MCP](https://modelcontextprotocol.io/) server with 6 tools. `make install` configures it automatically for both Claude Desktop and Claude Code.

### Manual MCP setup

If you installed from a release binary, add this config manually:

**Claude Desktop** — `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS):

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

**Claude Code** — `~/.claude/settings.json`:

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

### Tools

| Tool | Description |
|------|-------------|
| `speak` | Speak text aloud — returns immediately, audio queued in background |
| `set_config` | Change engine, model, voice, or speed |
| `get_config` | View current TTS configuration |
| `list_engines` | List available TTS engines |
| `list_models` | List available and installed models |
| `list_voices` | List available and installed voices |

---

## Engines

### Supertonic

66M parameter model by [Supertone](https://huggingface.co/Supertone/supertonic). Flow-matching architecture with a 4-model ONNX pipeline. 167x realtime on Apple Silicon. No phonemizer needed — works directly on unicode text.

```bash
local-voice models install supertonic      # 263 MB
```

10 voices: `F1`–`F5` (female), `M1`–`M5` (male). Languages: en, ko, es, pt, fr.

### Kokoro

82M parameter model from [onnx-community](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX). Near-human speech quality with 26 distinct voices. Uses espeak-ng for phonemization.

```bash
local-voice models install kokoro-q8f16    # 114 MB (recommended)
local-voice models install kokoro-fp16     # 163 MB
local-voice models install kokoro-fp32     # 326 MB (highest quality)
```

26 voices: American (`af_alloy`, `am_adam`, ...), British (`bf_alice`, `bm_daniel`, ...).

### Piper

Lightweight models (~15–100 MB) for maximum language coverage. One model = one voice.

```bash
local-voice models install en_US-lessac-medium    # 65 MB, English
local-voice models install de_DE-thorsten-medium   # 55 MB, German
```

20+ languages: en, de, fr, es, it, pt, nl, ru, zh, uk, no, and more.

### Chatterbox

500M parameter model for voice cloning. Pass a reference audio file to clone any voice.

```bash
local-voice models install chatterbox-quantized    # 580 MB
```

---

## Development

```bash
git clone https://github.com/Frenkiee/local-voice.git
cd local-voice
make deps                  # install espeak-ng + Rust
cargo build                # debug build
cargo build --release      # optimized build
cargo clippy -- -D warnings  # lint
cargo fmt --check          # format check
```

### Project structure

```
src/
  main.rs              CLI handlers, interactive mode
  cli.rs               Clap command definitions with rich help
  config.rs            TOML config management (~/.config/local-voice/config.toml)
  mcp.rs               MCP server (JSON-RPC over stdio, audio queue)
  audio.rs             Audio playback (rodio), WAV saving, AudioQueue
  download.rs          Model downloading with progress bars
  hardware.rs          Hardware detection and engine recommendations
  phonemize.rs         espeak-ng wrapper
  engine/
    mod.rs             TtsEngine trait, EngineKind enum, AudioOutput
    kokoro.rs          Kokoro ONNX inference (single model)
    supertonic.rs      Supertonic 4-model pipeline (DP → TE → VE → VOC)
    piper.rs           Piper ONNX inference
    chatterbox.rs      Chatterbox multi-session inference
  registry/
    mod.rs             EngineRegistry trait, cross-engine lookups
    kokoro.rs          Kokoro models, 26 voices, HuggingFace URLs
    supertonic.rs      Supertonic model, 10 voices, HuggingFace URLs
    piper.rs           Piper 18 models, HuggingFace URLs
    chatterbox.rs      Chatterbox 2 models, HuggingFace URLs
```

### CI/CD

- **CI** runs on every push/PR: `cargo check`, `clippy`, `fmt`, build matrix (macOS, Linux, Windows)
- **Release** triggered by `git tag v*`: builds binaries for 3 platforms, creates GitHub Release with downloads

### Contributing

1. Fork the repo
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make changes, ensure `cargo clippy -- -D warnings` and `cargo fmt --check` pass
4. Push and open a PR — CI must pass before merge

---

## License

[MIT](LICENSE)
