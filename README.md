# browser39

A headless web browser for AI agents. Single binary, no external browser required.

browser39 fetches web pages and converts them to token-optimized Markdown that LLMs can actually consume. It runs JavaScript, manages cookies and sessions, queries the DOM, and fills forms. All processing happens locally, no data is sent to third-party services.

**Works with:** [Claude Desktop & Claude Code](docs/install-claude.md) | [OpenClaw](docs/install-openclaw.md) | [Any agent via CLI](docs/install-cli.md)

## Comparison

|  | browser39 | Playwright / Puppeteer | Raw HTTP (requests, ureq) |
|--|-----------|----------------------|---------------------------|
| External browser | None (single binary) | Requires Chrome/Chromium | None |
| Binary size | ~22MB | ~280MB with browser | N/A (library) |
| Platforms | macOS, Linux, Windows | macOS, Linux, Windows | Any |
| JavaScript | Yes (boa_engine) | Yes (full V8) | No |
| HTML to Markdown | Built-in, token-optimized | No (raw HTML or screenshots) | DIY |
| Token preselection | Content sections, agent picks what to read | No | No |
| Cookies & sessions | Automatic, persisted, encrypted | Manual | Manual |
| DOM queries | CSS selectors + JS expressions | Full DOM API | No |
| Forms | fill + submit | Full interaction | Manual POST |
| Auth & secrets | Profiles, redaction, opaque handles | Manual | Manual |
| Transports | MCP (stdio + HTTP), JSONL, CLI | Library API | Library API |

### Token savings in practice

Real test: extracting the "Optical communications" section from [Artemis II on Wikipedia](https://en.wikipedia.org/wiki/Artemis_II) (full page: ~14,600 tokens).

| | Raw HTTP | WebFetch (Claude Code built-in) | [Mistral Web Search](https://docs.mistral.ai/agents/tools/built-in/websearch) | browser39 |
|--|----------|--------------------------------|-------------------|-----------|
| **How it works** | Fetch full page, truncate to ~1,000 tokens | Send full page (~14,600 tokens) to intermediate model with extraction prompt | Cloud API: search + page processing by Mistral model | Fetch → content selectors list → targeted section fetch |
| **Tokens consumed** | ~1,000 (truncated) | ~14,600 (processed by intermediate model) | Cloud processed, not disclosed | **196** |
| **Found the section?** | No. Section is at token ~6,320, truncated away | Yes, but returns a lossy summary | Depends on search ranking | Yes. Exact original content |
| **Content quality** | Nav menus, infobox, article intro | Paraphrased, no links, no references | Summary with citations | Lossless markdown with links and citations |
| **Session state** | None | None | None | Cookies, history, follow-up queries free |
| **Data processing** | Local | Processed remotely | Processed remotely | Local |
| **Cost per call** | Free | Bundled | [$30 / 1,000 calls](https://mistral.ai/pricing#api) | Free |
| **Retries needed** | Pagination to find it | None, but no control over output | May not find specific section | None. Agent sees structure first |

browser39 returns the exact section in **196 tokens** at zero cost. The raw approach misses it entirely, WebFetch burns **75x more tokens** through an intermediate model, and cloud tools like Mistral's charge $0.03 per call.

## Install

```bash
cargo install browser39
```

Pre-built binaries available on the [releases page](https://github.com/alejandroqh/browser39/releases).

### Auto-install prompts

Copy and paste into your agent to install browser39 automatically:

**Claude Code**

> Install browser39 as an MCP server. Clone https://github.com/alejandroqh/browser39.git, build with cargo build --release, copy the binary to /usr/local/bin/browser39, then add it to MCP settings with command "browser39" and args ["mcp"].

**OpenClaw**

> Install the browser39 plugin: openclaw plugins install https://github.com/alejandroqh/browser39.git && openclaw gateway restart

## Quick Start

### Claude Desktop / Claude Code (MCP)

Add to your MCP settings:

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

19 tools available instantly: `browser39_fetch`, `browser39_click`, `browser39_links`, `browser39_dom_query`, `browser39_fill`, `browser39_submit`, `browser39_search`, cookies, storage, history, and more.

See [docs/install-claude.md](docs/install-claude.md) for the full guide.

### OpenClaw

```bash
openclaw plugins install https://github.com/alejandroqh/browser39.git
openclaw gateway restart
```

See [docs/install-openclaw.md](docs/install-openclaw.md) for bundle vs native plugin setup.

### CLI: one-shot fetch

```bash
browser39 fetch https://example.com
```

```
# Example Domain
This domain is for use in documentation examples without needing permission.

[Learn more](https://iana.org/domains/example)
```

### CLI: agent integration (watch mode)

Long-running subprocess that any language can talk to via JSONL files:

```bash
touch commands.jsonl
browser39 watch commands.jsonl --output results.jsonl
```

```bash
# From your agent (Python, Node, Rust, shell, anything):
echo '{"id":"1","action":"fetch","v":1,"seq":1,"url":"https://example.com"}' >> commands.jsonl
```

Drop-in `web_search` and `visit_website` tool examples: **[Python](examples/browser39_tools.py)** | **[TypeScript](examples/browser39_tools.ts)** | **[Rust](examples/browser39_tools.rs)**

See [docs/install-cli.md](docs/install-cli.md) for the full integration guide.

## Features

### Token optimization

browser39 minimizes token usage when feeding web content to LLMs:

- **Content preselection**: on first fetch, returns available content sections with token estimates instead of dumping the full page. The agent picks the relevant section and re-fetches with a targeted `selector`.
- **Heading auto-expand**: `selector: "#Astronauts"` returns the full section until the next same-level heading, not just the heading text.
- **HTML to Markdown**: strips scripts, styles, and non-content elements.
- **Compact link references** (JSON mode): `[text][N]` instead of inline URLs, with full URLs in the `links` array.
- **Same-origin URL shortening**: links on the same domain show path-only.
- **Link deduplication**: same-URL links (image + headline cards) emitted once.

### JavaScript execution

boa_engine runs JS expressions against the parsed DOM. Supports `document.querySelector`, `document.querySelectorAll`, element properties, `localStorage`, `document.cookie`, `form.submit()`, and `element.click()`.

```json
{"action": "dom_query", "script": "document.querySelectorAll('a').length"}
```

### Session persistence

Cookies, localStorage, and browsing history are persisted to disk by default (`~/.local/share/browser39/session.enc`, AES-256-GCM encrypted). An agent can log in once and stay authenticated across restarts.

Disable with `--no-persist` or config:

```toml
[session]
persistence = "memory"
```

### Forms

Fill fields by CSS selector and submit. browser39 handles `enctype`, builds the HTTP request, and returns the response page:

```json
{"action": "fill", "fields": [{"selector": "#user", "value": "agent"}, {"selector": "#pass", "value": "secret", "sensitive": true}]}
{"action": "submit", "selector": "form#login"}
```

### Security

Auth profiles keep credentials out of the LLM conversation. The agent references a profile name and never sees the token:

```toml
[auth.github]
header = "Authorization"
value_env = "GITHUB_TOKEN"
value_prefix = "Bearer "
domains = ["api.github.com"]
```

```json
{"action": "fetch", "url": "https://api.github.com/repos", "auth_profile": "github"}
```

### All transports

| Transport | Command | Use case |
|-----------|---------|----------|
| MCP (stdio) | `browser39 mcp` | Claude Desktop, Claude Code, local MCP clients |
| MCP (HTTP) | `browser39 mcp --transport sse --port 8039` | Remote agents, cloud deployments |
| JSONL watch | `browser39 watch commands.jsonl` | Any language, long-running agent IPC |
| JSONL batch | `browser39 batch commands.jsonl` | One-shot scripted operations |
| CLI fetch | `browser39 fetch <url>` | Quick page retrieval, shell scripts |

## Configuration

```bash
browser39 --config path/to/config.toml fetch https://example.com
```

Precedence: `--config` flag > `BROWSER39_CONFIG` env > `~/.config/browser39/config.toml`

See [docs/config.md](docs/config.md) for the full reference.

## Documentation

| Doc | Description |
|-----|-------------|
| [install-claude.md](docs/install-claude.md) | Claude Desktop and Claude Code setup |
| [install-openclaw.md](docs/install-openclaw.md) | OpenClaw bundle and native plugin |
| [install-cli.md](docs/install-cli.md) | CLI integration guide with Rust, Python, TypeScript examples |
| [jsonl-protocol.md](docs/jsonl-protocol.md) | Full JSONL protocol specification |
| [config.md](docs/config.md) | Configuration reference |

## Development

```bash
cargo build              # Build
cargo run                # Run
cargo test               # Run all tests
cargo clippy             # Lint
cargo fmt                # Format
```
