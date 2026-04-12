# browser39 — Claude Bundle Plugin

This directory makes browser39 installable as an OpenClaw **Claude bundle**.

## Install

```bash
openclaw plugins install git@github.com:alejandroqh/browser39.git
```

## What it provides

OpenClaw detects this as a Claude bundle and maps the MCP server config from `.mcp.json`.
The browser39 binary runs as a stdio MCP subprocess, exposing all tools:

- `browser39_fetch` — fetch URL as markdown
- `browser39_click` — follow links
- `browser39_links` — list page links
- `browser39_dom_query` — CSS/JS DOM queries
- `browser39_fill` / `browser39_submit` — form interaction
- `browser39_cookies` / `browser39_set_cookie` / `browser39_delete_cookie`
- `browser39_storage_get` / `browser39_storage_set` / `browser39_storage_delete` / `browser39_storage_list` / `browser39_storage_clear`
- `browser39_search` — web search
- `browser39_back` / `browser39_forward` / `browser39_history`
- `browser39_info` — session info

## Requirements

The `browser39` binary must be in PATH, or update `.mcp.json` to point to the binary location.
