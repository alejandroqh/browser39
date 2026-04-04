# Install browser39 in Claude

browser39 works with **Claude Desktop** and **Claude Code** as an MCP server. This guide covers both manual setup and a one-click install prompt.

## Quick Install Prompt

Copy and paste this prompt into Claude to have it install browser39 automatically:

```
Install browser39 as an MCP server. Download the binary for this system from https://github.com/alejandroqh/browser39/releases/latest/download/ — assets are named browser39-{os}-{arch} (macos-arm64, macos-x64, linux-arm64, linux-x64, windows-x64.exe). Save to ~/.local/bin/browser39, make it executable, and add it to MCP settings with command "browser39" and args ["mcp"].
```

## Claude Desktop

Add to your `claude_desktop_config.json`:

**macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
**Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
**Linux:** `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "browser39": {
      "command": "browser39",
      "args": ["mcp"]
    }
  }
}
```

If the binary is not in PATH, use the full path:

```json
{
  "mcpServers": {
    "browser39": {
      "command": "/usr/local/bin/browser39",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop after saving.

## Claude Code

Add to your project `.mcp.json`:

```json
{
  "mcpServers": {
    "browser39": {
      "command": "browser39",
      "args": ["mcp"]
    }
  }
}
```

Or add globally via Claude Code settings:

```bash
claude mcp add browser39 -- browser39 mcp
```

## Build from Source

```bash
git clone https://github.com/alejandroqh/browser39.git
cd browser39
cargo build --release
cp target/release/browser39 /usr/local/bin/
```

## Verify

After installation, you should see 29 tools available:

- `browser39_fetch` — fetch pages as markdown
- `browser39_click` — follow links
- `browser39_links` — list page links
- `browser39_dom_query` — CSS/JS DOM queries
- `browser39_fill` / `browser39_submit` — form interaction
- `browser39_cookies` / `browser39_set_cookie` / `browser39_delete_cookie`
- `browser39_storage_get` / `browser39_storage_set` / `browser39_storage_delete` / `browser39_storage_list` / `browser39_storage_clear`
- `browser39_search` — web search
- `browser39_back` / `browser39_forward` / `browser39_history`
- `browser39_info` — session info

And 4 resources:

- `browser39://page` — current page markdown
- `browser39://page/links` — links JSON
- `browser39://page/meta` — metadata JSON
- `browser39://cookies` — cookies JSON

## Remote Agents (HTTP Transport)

For remote agents, start browser39 with the HTTP transport:

```bash
browser39 mcp --transport sse --port 8039
```

Then configure your MCP client to connect to `http://localhost:8039`.
