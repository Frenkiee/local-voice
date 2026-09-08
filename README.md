# local-voice

**Local text-to-speech for your terminal and AI agents. Zero latency. Zero cloud. Zero compromise.**

Run open-source TTS models 100% offline. Speak from the CLI, let Claude narrate its work via MCP, or browse voices in interactive mode. All processing stays on your machine.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Frenkiee/local-voice/actions/workflows/ci.yml/badge.svg)](https://github.com/Frenkiee/local-voice/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Frenkiee/local-voice)](https://github.com/Frenkiee/local-voice/releases/latest)

## Features

| Engine | Params | Quality | Speed | Languages |
|--------|--------|---------|-------|-----------|
| **Supertonic** | 66M / 99M (v3) | High | 167x realtime | 31 languages incl. sl, de, ja, ko (Supertonic 3) |
| **Kokoro** | 82M | Near-human | Fast | 9 languages (54 voices) |
| **Piper** | ~15M | Good | Fastest | 20 languages (32 voices) |
| **Chatterbox** | 350M (Turbo) / 500M | Best | Slow (Turbo ~6x faster) | en (voice cloning, `[laugh]` tags) |

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
4. Downloads the **Supertonic 3** model (~399 MB, 31 languages)
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

On Windows:
```cmd
cargo build --release
.\target\release\local-voice.exe speak "Hello world"
```

> **Important:** Always use `--release` builds. Debug builds are significantly slower for TTS inference.

> **Windows PATH:** If `make install` was used, add the install directory to PATH:
> ```cmd
> setx PATH "%PATH%;%USERPROFILE%\.local-voice\bin"
> ```
> Then restart your terminal.

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

Kokoro has 54 voices, Supertonic has 10. Each is a small download on top of the base model:

```bash
local-voice voices list                    # show all voices with install status
local-voice voices list -e kokoro          # filter by engine
local-voice voices install bf_emma         # install a Kokoro voice (~0.5 MB)
local-voice voices install ff_siwis        # non-English Kokoro voice (French)
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
local-voice config set ducking off         # stop lowering other apps' audio while speaking
local-voice config paths                   # show config + model file locations
local-voice config auto-detect             # pick best engine for your hardware
```

### Ducking

While local-voice speaks, every *other* app's audio (music, podcasts, browser tabs, …) is
faded down to 20 % and faded back up once speech finishes, so notifications stay
intelligible over whatever you're listening to. The system/master volume is never touched —
only other apps' per-app output level. Ducking is **on by default**.

```bash
local-voice config set ducking off         # disable globally (on|off|true|false|1|0)
local-voice config set ducking.level 0.4   # duck other apps to 40 % instead of 20 % (0..1)
local-voice config set ducking.fade_ms 500 # fade in/out duration in ms (max 5000, default 300)
local-voice speak "Hi" --no-ducking        # skip ducking for a single run
local-voice speak "Hi" --ducking-level 0.5 # override the level for a single run
```

Back-to-back `speak_async` calls from the MCP server are played under a single duck, so the
volume doesn't pump up and down between consecutive notifications.

Platform notes:

- **macOS 14.2+** — uses Core Audio process taps. The first time it runs, macOS shows a
  one-time **"System Audio Recording"** permission prompt for the app that launched
  `local-voice` (e.g. Terminal, iTerm, or Claude). If it's denied, ducking silently does
  nothing and speech plays at normal volume; grant it later in
  *System Settings → Privacy & Security → Screen & System Audio Recording*. Older macOS
  versions play without ducking.
- **Windows** — per-app volume via WASAPI audio sessions; no permissions needed.
- **Linux** — uses `pactl` to adjust per-stream volume (PulseAudio or PipeWire with
  `pipewire-pulse`). Without `pactl` on `PATH`, speech plays without ducking.

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
| `speak` | Speak text aloud — returns immediately, audio queued in background. Other apps' audio is ducked while speaking |
| `set_config` | Change engine, model, voice, or speed; `ducking` (boolean) and `ducking_level` (0..1) control audio ducking |
| `get_config` | View current TTS configuration |
| `list_engines` | List available TTS engines |
| `list_models` | List available and installed models |
| `list_voices` | List available and installed voices |

---

## Engines

### Supertonic

Flow-matching TTS by [Supertone](https://huggingface.co/Supertone) with a 4-model ONNX pipeline (duration predictor → text encoder → vector estimator → vocoder). 167x realtime on Apple Silicon, 44.1 kHz output. No phonemizer needed — works directly on unicode text.

| Model | Params | Size | Languages |
|-------|--------|------|-----------|
| `supertonic-3` (recommended) | 99M | 399 MB | 31: en, ko, ja, ar, bg, cs, da, de, el, es, et, fi, fr, hi, hr, hu, id, it, lt, lv, nl, pl, pt, ro, ru, sk, **sl**, sv, tr, uk, vi |
| `supertonic-2` | 66M | 264 MB | en, ko, es, pt, fr |
| `supertonic` (v1) | 66M | 263 MB | en only |

```bash
local-voice models install supertonic-3    # 399 MB, then: local-voice models default supertonic-3
local-voice speak "Dober dan, kako si?" --language sl
local-voice config set language sl         # make Slovenian the default (also: supertonic.language)
```

10 voices per model: `F1`–`F5` (female), `M1`–`M5` (male). Voice styles are model-specific, so `voices install M2` fetches the file for the model that is currently the default. Supertonic 3 also understands expression tags like `<laugh>`, `<breath>`, `<sigh>` inside the text. Language is passed as a tag around the text (`<sl>…</sl>`), so switching language needs no extra download.

### Kokoro

82M parameter model from [onnx-community](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX). Near-human speech quality with all 54 upstream voices across 9 languages. Uses espeak-ng for phonemization; the espeak voice is picked from the first letter of the voice ID.

```bash
local-voice models install kokoro-q8f16    # 86 MB (recommended)
local-voice models install kokoro-quantized # 92 MB (int8)
local-voice models install kokoro-uint8f16 # 114 MB
local-voice models install kokoro-q4f16    # 155 MB
local-voice models install kokoro-fp16     # 163 MB
local-voice models install kokoro-fp32     # 326 MB (highest quality)
```

54 voices (each a ~0.5 MB download):

| Prefix | Language | Voices |
|--------|----------|--------|
| `af_` / `am_` | American English | `af_heart`, `af_bella`, `am_adam`, `am_santa`, ... (20) |
| `bf_` / `bm_` | British English | `bf_alice`, `bf_emma`, `bm_daniel`, `bm_george`, ... (8) |
| `ef_` / `em_` | Spanish | `ef_dora`, `em_alex`, `em_santa` |
| `ff_` | French | `ff_siwis` |
| `hf_` / `hm_` | Hindi | `hf_alpha`, `hf_beta`, `hm_omega`, `hm_psi` |
| `if_` / `im_` | Italian | `if_sara`, `im_nicola` |
| `pf_` / `pm_` | Brazilian Portuguese | `pf_dora`, `pm_alex`, `pm_santa` |
| `jf_` / `jm_` | Japanese (experimental) | `jf_alpha`, `jf_gongitsune`, `jf_nezumi`, `jf_tebukuro`, `jm_kumo` |
| `zf_` / `zm_` | Mandarin Chinese (experimental) | `zf_xiaobei`, `zf_xiaoni`, `zf_xiaoxiao`, `zf_xiaoyi`, `zm_yunjian`, `zm_yunxi`, `zm_yunxia`, `zm_yunyang` |

Japanese and Chinese voices are marked *experimental*: upstream Kokoro phonemizes those languages with misaki, while local-voice uses espeak-ng (`ja` / `cmn`), so pronunciation is noticeably rougher than for the other languages.

### Piper

Lightweight models (~15–100 MB) for maximum language coverage. One model = one voice.

```bash
local-voice models install en_US-lessac-medium    # 63 MB, English
local-voice models install de_DE-thorsten-medium   # 63 MB, German
local-voice models install sl_SI-artur-medium      # 63 MB, Slovenian
local-voice speak "Dober dan, kako si?" --voice sl_SI-artur-medium
```

32 voices in 20 languages (all pinned to the `v1.0.0` tag of [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices)):

| Language | Models |
|----------|--------|
| English (US) | `en_US-lessac-medium`, `en_US-lessac-high`, `en_US-ljspeech-high`, `en_US-amy-medium`, `en_US-kristin-medium`, `en_US-ryan-medium`, `en_US-bryce-medium`, `en_US-arctic-medium` |
| English (GB) | `en_GB-alan-medium`, `en_GB-cori-medium` |
| German | `de_DE-thorsten-medium`, `de_DE-thorsten-high` |
| French | `fr_FR-upmc-medium` |
| Spanish | `es_ES-davefx-medium` |
| Italian | `it_IT-riccardo-x_low` |
| Portuguese (BR) | `pt_BR-faber-medium` |
| Dutch | `nl_NL-mls-medium` |
| Russian | `ru_RU-denis-medium` |
| Ukrainian | `uk_UA-lada-x_low`, `uk_UA-ukrainian_tts-medium` |
| Chinese | `zh_CN-huayan-medium` |
| Norwegian | `no_NO-talesyntese-medium` |
| Slovenian | `sl_SI-artur-medium` |
| Polish | `pl_PL-gosia-medium`, `pl_PL-darkman-medium` |
| Czech | `cs_CZ-jirka-medium` |
| Swedish | `sv_SE-nst-medium` |
| Danish | `da_DK-talesyntese-medium` |
| Turkish | `tr_TR-dfki-medium` |
| Arabic | `ar_JO-kareem-medium` |
| Hindi | `hi_IN-pratham-medium` |
| Korean | `ko_KR-kss-medium` (fetched from the `main` branch) |

The espeak-ng voice for each model is read from its `.onnx.json` (`espeak.voice`), so no per-language mapping is needed. `uk_UA-ukrainian_tts-medium` is a raw-text, multi-speaker model (no espeak-ng involved; speaker 0 is used). Japanese Piper voices are not listed because they need a Japanese-specific phonemizer.

### Chatterbox

Zero-shot voice cloning from Resemble AI, English only, 24 kHz. Pass a reference WAV with `--voice path/to/voice.wav` to clone any voice; without one the bundled reference voice is used. Both models run the same four-session ONNX pipeline (speech encoder → token embedder → autoregressive LM → conditional decoder) on CPU, so expect seconds rather than milliseconds.

```bash
local-voice models install chatterbox-turbo        # 697 MB (recommended)
local-voice models install chatterbox-quantized    # 1.5 GB
local-voice models install chatterbox-full         # 3.1 GB
```

| Model | Source | Params | Notes |
|-------|--------|--------|-------|
| `chatterbox-turbo` | [ResembleAI/chatterbox-turbo-ONNX](https://huggingface.co/ResembleAI/chatterbox-turbo-ONNX) (Q4 tier) | 350M | 1-step mel decoder, ~6x faster than the original; supports paralinguistic tags such as `[laugh]`, `[chuckle]`, `[cough]`, `[sigh]` inline in the text |
| `chatterbox-quantized` | [onnx-community/chatterbox-ONNX](https://huggingface.co/onnx-community/chatterbox-ONNX) (Q4 language model, fp32 rest) | 500M | Original Chatterbox with emotion exaggeration |
| `chatterbox-full` | onnx-community/chatterbox-ONNX (fp32) | 500M | Same as above at full precision; needs ~16 GB RAM |

```bash
local-voice speak "Oh, that's hilarious! [chuckle] Anyway, how are you?" --engine chatterbox
```

The `fp16` / `q4f16` tiers of both repos are not offered: they export float16 KV caches, which the engine does not handle yet. Sizes above are the real download totals (MiB, checked against Hugging Face); the language model is ~195 MB of Turbo's total, the rest is the speech encoder and decoder.

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
  ducking/
    mod.rs             DuckingSettings, duck() + RAII DuckGuard, Ducker trait
    macos.rs           Core Audio process-tap backend (macOS 14.2+)
    windows.rs         WASAPI per-session volume backend
    linux.rs           pactl (PulseAudio / PipeWire) per-stream volume backend
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
    kokoro.rs          Kokoro 6 model variants, 54 voices, HuggingFace URLs
    supertonic.rs      Supertonic v1/2/3 models, 10 voices, HuggingFace URLs
    piper.rs           Piper 32 models / 20 languages, HuggingFace URLs
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
