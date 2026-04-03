.PHONY: build install uninstall clean deps mcp-install mcp-uninstall help

# ─── OS Detection ────────────────────────────────────────────────────────────
# Detect OS: windows, macos, linux
ifeq ($(OS),Windows_NT)
    DETECTED_OS := windows
else
    UNAME_S := $(shell uname -s)
    ifeq ($(UNAME_S),Darwin)
        DETECTED_OS := macos
    else ifeq ($(UNAME_S),Linux)
        DETECTED_OS := linux
    else
        DETECTED_OS := unknown
    endif
endif

# ─── Platform-specific defaults ──────────────────────────────────────────────
# Find cargo even if not in PATH
CARGO := $(shell command -v cargo 2>/dev/null || echo "$(HOME)/.cargo/bin/cargo")

ifeq ($(DETECTED_OS),windows)
    BINARY      := local-voice.exe
    PREFIX      ?= $(USERPROFILE)\.local-voice
    INSTALL_DIR := $(PREFIX)\bin
    SEP         := \\
    MCP_CONFIG  := $(APPDATA)\Claude\claude_desktop_config.json
    CLAUDE_CODE_CONFIG_DIR := $(USERPROFILE)\.claude
else ifeq ($(DETECTED_OS),macos)
    BINARY      := local-voice
    PREFIX      ?= /usr/local
    INSTALL_DIR := $(PREFIX)/bin
    SEP         := /
    MCP_CONFIG  := $(HOME)/Library/Application Support/Claude/claude_desktop_config.json
    CLAUDE_CODE_CONFIG_DIR := $(HOME)/.claude
else
    BINARY      := local-voice
    PREFIX      ?= /usr/local
    INSTALL_DIR := $(PREFIX)/bin
    SEP         := /
    MCP_CONFIG  := $(HOME)/.config/Claude/claude_desktop_config.json
    CLAUDE_CODE_CONFIG_DIR := $(HOME)/.claude
endif

CLAUDE_CODE_CONFIG := $(CLAUDE_CODE_CONFIG_DIR)/settings.json

# ─── Help ────────────────────────────────────────────────────────────────────
help:
	@echo "local-voice — Local TTS with MCP server"
	@echo ""
	@echo "Usage:"
	@echo "  make install        Full install: deps + build + binary + MCP config"
	@echo "  make uninstall      Remove binary and MCP config"
	@echo "  make deps           Install system dependencies (espeak-ng, Rust)"
	@echo "  make build          Build release binary"
	@echo "  make mcp-install    Add MCP server config for Claude"
	@echo "  make mcp-uninstall  Remove MCP server config"
	@echo "  make clean          Clean build artifacts"
	@echo ""
	@echo "Detected OS: $(DETECTED_OS)"

# ─── Dependencies ────────────────────────────────────────────────────────────
deps:
	@echo "── Installing dependencies ($(DETECTED_OS)) ──"
	@echo ""
	@# ── Rust / Cargo ──
	@command -v cargo >/dev/null 2>&1 \
		|| test -x "$(HOME)/.cargo/bin/cargo" \
		|| { \
			echo "Installing Rust via rustup..."; \
			curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
			echo ""; \
			echo "✓ Rust installed — restart your shell or run:"; \
			echo '  source "$$HOME/.cargo/env"'; \
			echo ""; \
		}
	@command -v cargo >/dev/null 2>&1 \
		|| test -x "$(HOME)/.cargo/bin/cargo" \
		&& echo "✓ cargo found"
ifeq ($(DETECTED_OS),macos)
	@# ── espeak-ng (macOS) ──
	@command -v espeak-ng >/dev/null 2>&1 \
		|| test -x /opt/homebrew/bin/espeak-ng \
		|| test -x /usr/local/bin/espeak-ng \
		|| test -x /opt/local/bin/espeak-ng \
		&& echo "✓ espeak-ng found" || { \
		echo "Installing espeak-ng..."; \
		if command -v brew >/dev/null 2>&1 || test -x /opt/homebrew/bin/brew; then \
			BREW=$$(command -v brew 2>/dev/null || echo /opt/homebrew/bin/brew); \
			$$BREW install espeak-ng; \
		elif command -v port >/dev/null 2>&1 || test -x /opt/local/bin/port; then \
			PORT=$$(command -v port 2>/dev/null || echo /opt/local/bin/port); \
			sudo $$PORT install espeak-ng; \
		else \
			echo "⚠ Neither Homebrew nor MacPorts found."; \
			echo "  Install Homebrew first: /bin/bash -c \"\$$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""; \
			echo "  Then run: brew install espeak-ng"; \
			exit 1; \
		fi; \
	}
else ifeq ($(DETECTED_OS),linux)
	@# ── espeak-ng (Linux) ──
	@command -v espeak-ng >/dev/null 2>&1 && echo "✓ espeak-ng found" || { \
		echo "Installing espeak-ng..."; \
		if command -v apt-get >/dev/null 2>&1; then \
			sudo apt-get update -qq && sudo apt-get install -y espeak-ng; \
		elif command -v dnf >/dev/null 2>&1; then \
			sudo dnf install -y espeak-ng; \
		elif command -v pacman >/dev/null 2>&1; then \
			sudo pacman -S --noconfirm espeak-ng; \
		elif command -v zypper >/dev/null 2>&1; then \
			sudo zypper install -y espeak-ng; \
		elif command -v apk >/dev/null 2>&1; then \
			sudo apk add espeak-ng; \
		elif command -v nix-env >/dev/null 2>&1; then \
			nix-env -i espeak-ng; \
		else \
			echo "⚠ Could not detect package manager."; \
			echo "  Install espeak-ng manually for your distro."; \
			exit 1; \
		fi; \
	}
else ifeq ($(DETECTED_OS),windows)
	@# ── espeak-ng (Windows) ──
	@command -v espeak-ng >/dev/null 2>&1 && echo "✓ espeak-ng found" || { \
		echo "Installing espeak-ng..."; \
		if command -v choco >/dev/null 2>&1; then \
			choco install espeak-ng -y; \
		elif command -v scoop >/dev/null 2>&1; then \
			scoop install espeak-ng; \
		elif command -v winget >/dev/null 2>&1; then \
			winget install espeak.espeak-ng; \
		else \
			echo "⚠ No package manager found (choco/scoop/winget)."; \
			echo "  Download espeak-ng from: https://github.com/espeak-ng/espeak-ng/releases"; \
			exit 1; \
		fi; \
	}
endif
	@echo ""
	@echo "✓ All dependencies ready"

# ─── Build ───────────────────────────────────────────────────────────────────
build:
	$(CARGO) build --release

# ─── Install ─────────────────────────────────────────────────────────────────
install: deps build
ifeq ($(DETECTED_OS),windows)
	@mkdir -p "$(INSTALL_DIR)"
	@cp "target/release/$(BINARY)" "$(INSTALL_DIR)/$(BINARY)"
else
	@install -d "$(INSTALL_DIR)"
	@install -m 755 "target/release/$(BINARY)" "$(INSTALL_DIR)/$(BINARY)"
endif
	@echo ""
	@echo "✓ Installed $(BINARY) to $(INSTALL_DIR)/$(BINARY)"
ifeq ($(DETECTED_OS),windows)
	@echo ""
	@echo "Add to PATH if not already:"
	@echo '  setx PATH "%PATH%;$(INSTALL_DIR)"'
endif
	@echo ""
	@echo "Quick start:"
	@echo "  local-voice models list                         # browse voices"
	@echo "  local-voice models install en_US-lessac-medium  # install a voice"
	@echo "  local-voice speak \"Hello world\"                 # speak text"
	@echo "  local-voice serve                               # start MCP server"
	@echo ""
	@$(MAKE) --no-print-directory mcp-install

# ─── Uninstall ───────────────────────────────────────────────────────────────
uninstall:
	@rm -f "$(INSTALL_DIR)/$(BINARY)"
	@echo "✓ Removed $(BINARY)"
	@$(MAKE) --no-print-directory mcp-uninstall

# ─── Clean ───────────────────────────────────────────────────────────────────
clean:
	$(CARGO) clean

# ─── MCP Config ──────────────────────────────────────────────────────────────
# Writes to both Claude Desktop config and Claude Code CLI config

define MCP_PYTHON_UPSERT
import json, os, sys
path = sys.argv[1]
binary = sys.argv[2]
os.makedirs(os.path.dirname(path), exist_ok=True)
d = {}
if os.path.isfile(path):
    try:
        d = json.load(open(path))
    except json.JSONDecodeError:
        d = {}
d.setdefault('mcpServers', {})['local-voice'] = {
    'command': binary,
    'args': ['serve']
}
json.dump(d, open(path, 'w'), indent=2)
print('  ✓ ' + path)
endef
export MCP_PYTHON_UPSERT

define MCP_PYTHON_REMOVE
import json, os, sys
path = sys.argv[1]
if not os.path.isfile(path):
    sys.exit(0)
try:
    d = json.load(open(path))
except json.JSONDecodeError:
    sys.exit(0)
d.get('mcpServers', {}).pop('local-voice', None)
json.dump(d, open(path, 'w'), indent=2)
print('  ✓ Removed from ' + path)
endef
export MCP_PYTHON_REMOVE

mcp-install:
	@echo "Configuring MCP server..."
	@BINARY_PATH=$$(command -v $(BINARY) 2>/dev/null || echo "$(INSTALL_DIR)/$(BINARY)"); \
	if command -v python3 >/dev/null 2>&1; then \
		python3 -c "$$MCP_PYTHON_UPSERT" "$(MCP_CONFIG)" "$$BINARY_PATH"; \
		python3 -c "$$MCP_PYTHON_UPSERT" "$(CLAUDE_CODE_CONFIG)" "$$BINARY_PATH"; \
	elif command -v python >/dev/null 2>&1; then \
		python -c "$$MCP_PYTHON_UPSERT" "$(MCP_CONFIG)" "$$BINARY_PATH"; \
		python -c "$$MCP_PYTHON_UPSERT" "$(CLAUDE_CODE_CONFIG)" "$$BINARY_PATH"; \
	else \
		echo "⚠ python not found — add MCP config manually:"; \
		echo '  { "mcpServers": { "local-voice": { "command": "'$$BINARY_PATH'", "args": ["serve"] } } }'; \
		echo ""; \
		echo "  Claude Desktop: $(MCP_CONFIG)"; \
		echo "  Claude Code:    $(CLAUDE_CODE_CONFIG)"; \
	fi
	@echo ""

mcp-uninstall:
	@if command -v python3 >/dev/null 2>&1; then \
		python3 -c "$$MCP_PYTHON_REMOVE" "$(MCP_CONFIG)"; \
		python3 -c "$$MCP_PYTHON_REMOVE" "$(CLAUDE_CODE_CONFIG)"; \
	elif command -v python >/dev/null 2>&1; then \
		python -c "$$MCP_PYTHON_REMOVE" "$(MCP_CONFIG)"; \
		python -c "$$MCP_PYTHON_REMOVE" "$(CLAUDE_CODE_CONFIG)"; \
	fi
