# Install browser39 in OpenClaw

browser39 integrates with OpenClaw as either a **Claude bundle** (MCP subprocess) or a **native plugin** (in-process tool registration). Both require the browser39 binary to be installed.

## Quick Install Prompt

Copy and paste this prompt into OpenClaw to have it install browser39 automatically:

```
Install the browser39 plugin for web browsing. It's a headless browser that converts pages to token-optimized markdown.

Steps:
1. Check if browser39 binary exists: which browser39
2. If not found, clone and build:
   git clone https://github.com/alejandroqh/browser39.git ~/.local/share/browser39-repo
   cd ~/.local/share/browser39-repo && cargo build --release
   cp target/release/browser39 /usr/local/bin/browser39
3. Install the plugin as a Claude bundle:
   openclaw plugins install https://github.com/alejandroqh/browser39.git
4. Restart the gateway: openclaw gateway restart
5. Verify: openclaw plugins list

If cargo is not installed, install Rust first: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Option A: Claude Bundle (recommended)

The simplest path. OpenClaw detects the `.claude-plugin/` layout and launches browser39 as an MCP stdio subprocess.

### Install

```bash
# From the GitHub repo
openclaw plugins install https://github.com/alejandroqh/browser39.git

# Or from a local clone
git clone https://github.com/alejandroqh/browser39.git
openclaw plugins install ./browser39
```

### Verify

```bash
openclaw plugins list
openclaw plugins inspect browser39
```

It should show `Format: bundle` with subtype `claude`.

### Restart

```bash
openclaw gateway restart
```

The 19 browser39 MCP tools are now available as `browser39__browser39_fetch`, `browser39__browser39_click`, etc.

## Option B: Native Plugin

Deeper integration with config validation, UI hints, and in-process tool registration. The native plugin spawns `browser39 mcp` and proxies tool calls via JSON-RPC.

### Install

```bash
# From the GitHub repo (native plugin subdirectory)
git clone https://github.com/alejandroqh/browser39.git
cd browser39/openclaw-plugin
npm install
openclaw plugins install .

# Or link for development
openclaw plugins install -l .
```

### Configure

```json
{
  "plugins": {
    "entries": {
      "browser39": {
        "enabled": true,
        "config": {
          "binaryPath": "/usr/local/bin/browser39",
          "configPath": "~/.config/browser39/config.toml"
        }
      }
    }
  }
}
```

Both `binaryPath` and `configPath` are optional — defaults to `browser39` in PATH and no config file.

### Restart

```bash
openclaw gateway restart
```

Tools register directly as `browser39_fetch`, `browser39_click`, etc. (no server prefix).

## Build the Binary

Both approaches require the `browser39` binary:

```bash
git clone https://github.com/alejandroqh/browser39.git
cd browser39
cargo build --release
cp target/release/browser39 /usr/local/bin/
```

### Pre-built binaries

Check the [releases page](https://github.com/alejandroqh/browser39/releases) for pre-built binaries:

- `browser39-v1.0.0-macos-arm64` — macOS Apple Silicon
- `browser39-v1.0.0-linux-arm64` — Linux ARM64

## Available Tools

Once installed, these 19 tools are available:

| Tool | Description |
|------|-------------|
| `browser39_fetch` | Fetch a URL as token-optimized markdown |
| `browser39_click` | Follow a link by index or text |
| `browser39_links` | List all links on the current page |
| `browser39_dom_query` | Query DOM with CSS selector or JavaScript |
| `browser39_fill` | Fill form fields by CSS selector |
| `browser39_submit` | Submit a form |
| `browser39_cookies` | List cookies |
| `browser39_set_cookie` | Set a cookie |
| `browser39_delete_cookie` | Delete a cookie |
| `browser39_storage_get` | Get a localStorage value |
| `browser39_storage_set` | Set a localStorage value |
| `browser39_storage_delete` | Delete a localStorage key |
| `browser39_storage_list` | List localStorage entries |
| `browser39_storage_clear` | Clear localStorage |
| `browser39_search` | Web search (DuckDuckGo) |
| `browser39_back` | Navigate back |
| `browser39_forward` | Navigate forward |
| `browser39_history` | Search/list browsing history |
| `browser39_info` | Session info and liveness |

## Coexistence with OpenClaw's Built-in Browser

browser39 and OpenClaw's built-in `browser` plugin serve different purposes:

- **OpenClaw browser** — full Chrome/Brave/Edge automation (CDP + Playwright), screenshots, DOM actions by ref
- **browser39** — headless HTTP client, HTML-to-markdown conversion, token-optimized for AI consumption

They can run side by side. If you want browser39 to be the only browser, disable the built-in:

```json
{
  "plugins": {
    "entries": {
      "browser": { "enabled": false }
    }
  }
}
```

## Troubleshooting

**Plugin detected but tools are missing**
Make sure the browser39 binary is in PATH or `binaryPath` is set correctly. Run `which browser39` to verify.

**Gateway restart required**
Config and plugin changes require `openclaw gateway restart`.

**Bundle vs native tool naming**
Bundle installs prefix tools with the server name: `browser39__browser39_fetch`. Native installs register them directly: `browser39_fetch`.
