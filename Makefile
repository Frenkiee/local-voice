.PHONY: build install uninstall clean check-deps mcp-install mcp-uninstall help

PREFIX ?= /usr/local
BINARY = local-voice
CARGO = cargo

help: ## Show this help
	@echo "local-voice — Local TTS with MCP server"
	@echo ""
	@echo "Usage:"
	@echo "  make build          Build release binary"
	@echo "  make install        Install binary and MCP config"
	@echo "  make uninstall      Remove binary and MCP config"
	@echo "  make mcp-install    Add MCP server config for Claude"
	@echo "  make mcp-uninstall  Remove MCP server config"
	@echo "  make check-deps     Check system dependencies"
	@echo "  make clean          Clean build artifacts"

build: ## Build release binary
	$(CARGO) build --release

install: check-deps build ## Install binary to PREFIX
	install -d $(PREFIX)/bin
	install -m 755 target/release/$(BINARY) $(PREFIX)/bin/$(BINARY)
	@echo ""
	@echo "✓ Installed $(BINARY) to $(PREFIX)/bin/$(BINARY)"
	@echo ""
	@echo "Quick start:"
	@echo "  $(BINARY) models list              # see available voices"
	@echo "  $(BINARY) models install en_US-lessac-medium  # install a voice"
	@echo "  $(BINARY) speak 'Hello world'      # speak text"
	@echo "  $(BINARY) serve                    # start MCP server"
	@echo ""
	@$(MAKE) --no-print-directory mcp-install

uninstall: ## Remove binary
	rm -f $(PREFIX)/bin/$(BINARY)
	@echo "✓ Removed $(BINARY)"
	@$(MAKE) --no-print-directory mcp-uninstall

clean: ## Clean build artifacts
	$(CARGO) clean

check-deps: ## Check system dependencies
	@echo "Checking dependencies..."
	@command -v espeak-ng >/dev/null 2>&1 || { \
		echo ""; \
		echo "⚠ espeak-ng not found (required for Piper TTS phonemization)"; \
		echo ""; \
		echo "Install it:"; \
		echo "  macOS:  brew install espeak-ng"; \
		echo "  Ubuntu: sudo apt install espeak-ng"; \
		echo "  Fedora: sudo dnf install espeak-ng"; \
		echo "  Arch:   sudo pacman -S espeak-ng"; \
		echo ""; \
		exit 1; \
	}
	@echo "✓ espeak-ng found"

MCP_CONFIG_DIR = $(HOME)/.claude
MCP_CONFIG = $(MCP_CONFIG_DIR)/settings.json

mcp-install: ## Add MCP server config for Claude Code
	@mkdir -p $(MCP_CONFIG_DIR)
	@BINARY_PATH=$$(command -v $(BINARY) 2>/dev/null || echo "$(PREFIX)/bin/$(BINARY)"); \
	if [ -f "$(MCP_CONFIG)" ]; then \
		if command -v python3 >/dev/null 2>&1; then \
			python3 -c " \
import json, sys; \
p = '$(MCP_CONFIG)'; \
d = json.load(open(p)); \
d.setdefault('mcpServers', {})['local-voice'] = {'command': '$$BINARY_PATH', 'args': ['serve']}; \
json.dump(d, open(p, 'w'), indent=2); \
print('✓ Added local-voice MCP server to $(MCP_CONFIG)')"; \
		else \
			echo "⚠ python3 not found, add manually to $(MCP_CONFIG):"; \
			echo '  "mcpServers": { "local-voice": { "command": "'$$BINARY_PATH'", "args": ["serve"] } }'; \
		fi; \
	else \
		echo '{ "mcpServers": { "local-voice": { "command": "'$$BINARY_PATH'", "args": ["serve"] } } }' > "$(MCP_CONFIG)"; \
		echo "✓ Created $(MCP_CONFIG) with local-voice MCP server"; \
	fi

mcp-uninstall: ## Remove MCP server config
	@if [ -f "$(MCP_CONFIG)" ] && command -v python3 >/dev/null 2>&1; then \
		python3 -c " \
import json; \
p = '$(MCP_CONFIG)'; \
d = json.load(open(p)); \
d.get('mcpServers', {}).pop('local-voice', None); \
json.dump(d, open(p, 'w'), indent=2); \
print('✓ Removed local-voice from $(MCP_CONFIG)')"; \
	fi
