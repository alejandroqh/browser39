# Install browser39 as a CLI tool

browser39 integrates with any agent application via **watch mode** — a long-running process that communicates through JSONL files. No MCP required. This guide shows how to replace basic `web_search` and `visit_website` tools with a full browser.

## Why watch mode over raw HTTP

| | Raw HTTP (ureq, reqwest) | browser39 watch |
|---|---|---|
| JavaScript | No | Yes (boa_engine) |
| Cookies & sessions | Manual | Automatic, persisted |
| HTML to markdown | DIY | Built-in, token-optimized |
| Navigation history | No | Yes, with back/forward |
| Forms | Manual POST | fill + submit |
| Content sections | No | Auto-detected, agent picks which to read |
| Search | Scrape DDG yourself | Fetch DDG URL, get parsed results |

## Build

```bash
git clone https://github.com/alejandroqh/browser39.git
cd browser39
cargo build --release
cp target/release/browser39 /path/to/your-agent/tools/browser39
```

## Architecture

```
your-agent                          browser39 watch
    |                                     |
    |-- append command to commands.jsonl ->|
    |                                     |-- fetch page, convert to md
    |<- read result from results.jsonl ---|
    |                                     |
    |-- append next command ------------->|
    |<- read result ----------------------|
    |                                     |
    |-- append {"action":"quit"} -------->|
    |                                 [exits]
```

**Key design choices:**
- browser39 runs as a **singleton subprocess** — started on first tool call, kept alive for the session
- Commands and results use **append-only JSONL files** (one JSON object per line)
- When reading results, only inspect the **last 2 lines** to keep reads lightweight
- Each command has a monotonically increasing `seq` number; the agent polls for its matching result

## Rust Example — Complete Integration

This is the actual implementation used in [diana86](https://github.com/alejandroqh/diana86), a Rust agent orchestrator.

### BrowserClient singleton

The client manages the browser39 subprocess, sends commands, and reads results.

```rust
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use serde_json::Value;

static CLIENT: LazyLock<Mutex<BrowserClient>> = LazyLock::new(|| {
    Mutex::new(BrowserClient::new())
});

struct BrowserClient {
    process: Option<Child>,
    seq: u64,
    dir: PathBuf,
    bin: PathBuf,
}

impl BrowserClient {
    fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let dir = cwd.join(".cache/browser39");
        let bin = cwd.join("tools/browser39");
        Self { process: None, seq: 0, dir, bin }
    }

    fn commands_path(&self) -> PathBuf { self.dir.join("commands.jsonl") }
    fn results_path(&self) -> PathBuf { self.dir.join("results.jsonl") }

    /// Start browser39 watch if not already running.
    fn ensure_running(&mut self) -> Result<(), String> {
        if let Some(ref mut child) = self.process {
            match child.try_wait() {
                Ok(Some(_)) => self.process = None, // exited, restart
                Ok(None) => return Ok(()),           // still alive
                Err(_) => self.process = None,
            }
        }

        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("browser39: mkdir failed: {e}"))?;

        // Truncate files for a fresh session
        File::create(self.commands_path())
            .map_err(|e| format!("browser39: create commands: {e}"))?;
        File::create(self.results_path())
            .map_err(|e| format!("browser39: create results: {e}"))?;
        self.seq = 0;

        if !self.bin.exists() {
            return Err(format!("browser39 not found at {}", self.bin.display()));
        }

        let child = Command::new(&self.bin)
            .arg("watch")
            .arg(self.commands_path())
            .arg("--output")
            .arg(self.results_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("browser39: spawn failed: {e}"))?;

        self.process = Some(child);
        std::thread::sleep(Duration::from_millis(250));
        Ok(())
    }

    /// Send a command and block until the result appears.
    fn send(&mut self, action: &str, extra: Value) -> Result<Value, String> {
        self.ensure_running()?;

        self.seq += 1;
        let seq = self.seq;

        let mut cmd = serde_json::json!({
            "id": format!("cmd-{seq}"),
            "action": action,
            "v": 1,
            "seq": seq,
        });

        // Merge action-specific fields
        if let (Some(obj), Some(extra_obj)) = (cmd.as_object_mut(), extra.as_object()) {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }

        // Append to commands file
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.commands_path())
            .map_err(|e| format!("browser39: write: {e}"))?;
        writeln!(file, "{}", serde_json::to_string(&cmd).unwrap())
            .map_err(|e| format!("browser39: write: {e}"))?;
        file.flush().map_err(|e| format!("browser39: flush: {e}"))?;

        // Poll for result (last 2 lines only)
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(35) {
                return Err("browser39: timeout".into());
            }
            if let Ok(f) = File::open(self.results_path()) {
                let lines: Vec<String> = BufReader::new(f)
                    .lines()
                    .filter_map(|l| l.ok())
                    .filter(|l| !l.trim().is_empty())
                    .collect();
                for line in lines.iter().rev().take(2) {
                    if let Ok(val) = serde_json::from_str::<Value>(line) {
                        if val.get("seq").and_then(|s| s.as_u64()) == Some(seq) {
                            return Ok(val);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Public API — send a command to the shared browser39 process.
pub fn send_command(action: &str, extra: Value) -> Result<Value, String> {
    CLIENT.lock().map_err(|e| format!("lock: {e}"))?.send(action, extra)
}
```

### web_search tool

Fetches the DuckDuckGo HTML search page through browser39, extracts result links from the `uddg` redirect parameters.

```rust
fn web_search(query: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query),
    );

    let result = send_command("fetch", serde_json::json!({
        "url": url,
        "options": {
            "max_tokens": 4000,
            "strip_nav": true,
            "show_selectors_first": false
        }
    }))?;

    if result["ok"] != true {
        return Err(result["error"].as_str().unwrap_or("search failed").into());
    }

    // DDG result links are redirects: /l/?uddg=<encoded_url>&rut=...
    let links = result["links"].as_array().cloned().unwrap_or_default();
    let results: Vec<(String, String)> = links.iter()
        .filter_map(|link| {
            let href = link["href"].as_str()?;
            let text = link["text"].as_str().filter(|t| !t.is_empty())?;
            if !href.contains("uddg=") { return None; }
            // Skip ads
            if href.contains("ad_domain") || href.contains("ad_provider") {
                return None;
            }
            // Extract real URL from uddg parameter
            let real_url = href.split('?').chain(href.split('&'))
                .find_map(|part| part.strip_prefix("uddg="))
                .map(|v| urlencoding::decode(v).unwrap_or_default().into_owned())?;
            Some((text.to_string(), real_url))
        })
        .take(5)
        .collect();

    Ok(results)
}
```

### visit_website tool

Uses browser39's content sections feature: first call returns the page structure, the agent picks a section, second call returns focused content.

```rust
fn visit_website(url: &str, selector: Option<&str>) -> Result<String, String> {
    let mut options = serde_json::json!({
        "max_tokens": 4000,
        "strip_nav": true,
        "include_links": true,
    });

    if let Some(sel) = selector {
        // Agent chose a section — fetch it directly
        options["selector"] = serde_json::Value::String(sel.to_string());
        options["show_selectors_first"] = serde_json::Value::Bool(false);
    }
    // Without a selector, show_selectors_first defaults to true —
    // browser39 returns the page's content sections so the agent can
    // choose which to read in the next round.

    let result = send_command("fetch", serde_json::json!({
        "url": url,
        "options": options,
    }))?;

    if result["ok"] != true {
        return Err(result["error"].as_str().unwrap_or("fetch failed").into());
    }

    Ok(result["markdown"].as_str().unwrap_or("").to_string())
}
```

**Typical agent flow:**

```
Round 1: visit_website("https://example.com")
  → "Page has content sections. Re-fetch with a selector..."
  → Agent sees the hint and decides which section to read

Round 2: visit_website("https://example.com", selector="article")
  → "# Example Article\n\nThe actual content..."
```

## CLI Hook Pattern

For agents that use shell-based tool dispatch (hooks, plugins), you can replace `web_search` and `visit_website` with shell scripts that talk to browser39 watch.

### Start browser39 in the background

```bash
mkdir -p .cache/browser39
touch .cache/browser39/commands.jsonl
browser39 watch .cache/browser39/commands.jsonl \
    --output .cache/browser39/results.jsonl &
BROWSER_PID=$!
```

### web_search hook

```bash
#!/bin/bash
# hooks/web_search.sh — replace web_search tool
QUERY="$1"
SEQ="$2"
ENCODED=$(python3 -c "import urllib.parse; print(urllib.parse.quote_plus('$QUERY'))")

echo "{\"id\":\"s-$SEQ\",\"action\":\"fetch\",\"v\":1,\"seq\":$SEQ,\"url\":\"https://html.duckduckgo.com/html/?q=$ENCODED\",\"options\":{\"max_tokens\":4000,\"strip_nav\":true,\"show_selectors_first\":false}}" >> .cache/browser39/commands.jsonl

# Wait for result (poll last 2 lines)
for i in $(seq 1 50); do
    RESULT=$(tail -2 .cache/browser39/results.jsonl 2>/dev/null | grep "\"seq\":$SEQ")
    if [ -n "$RESULT" ]; then
        echo "$RESULT"
        exit 0
    fi
    sleep 0.1
done
echo '{"ok":false,"error":"timeout"}'
```

### visit_website hook

```bash
#!/bin/bash
# hooks/visit_website.sh — replace visit_website tool
URL="$1"
SEQ="$2"
SELECTOR="${3:-}"  # optional

if [ -n "$SELECTOR" ]; then
    OPTIONS="{\"max_tokens\":4000,\"strip_nav\":true,\"include_links\":true,\"selector\":\"$SELECTOR\",\"show_selectors_first\":false}"
else
    OPTIONS="{\"max_tokens\":4000,\"strip_nav\":true,\"include_links\":true}"
fi

echo "{\"id\":\"v-$SEQ\",\"action\":\"fetch\",\"v\":1,\"seq\":$SEQ,\"url\":\"$URL\",\"options\":$OPTIONS}" >> .cache/browser39/commands.jsonl

for i in $(seq 1 50); do
    RESULT=$(tail -2 .cache/browser39/results.jsonl 2>/dev/null | grep "\"seq\":$SEQ")
    if [ -n "$RESULT" ]; then
        echo "$RESULT"
        exit 0
    fi
    sleep 0.1
done
echo '{"ok":false,"error":"timeout"}'
```

### Cleanup

```bash
# Send quit and wait for exit
echo "{\"id\":\"q\",\"action\":\"quit\",\"v\":1,\"seq\":999}" >> .cache/browser39/commands.jsonl
wait $BROWSER_PID
```

## JSONL Protocol Reference

Every command is a single JSON line with these required fields:

```json
{"id": "unique", "action": "fetch", "v": 1, "seq": 1, "url": "https://..."}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique ID, echoed in result |
| `action` | string | `fetch`, `links`, `back`, `forward`, `info`, `quit`, etc. |
| `v` | integer | Protocol version (always `1`) |
| `seq` | integer | Monotonically increasing sequence number |

Results mirror the same structure:

```json
{"id": "unique", "ok": true, "seq": 1, "url": "...", "title": "...", "markdown": "...", "links": [...]}
```

See [jsonl-protocol.md](jsonl-protocol.md) for the full specification.

## Session Persistence

By default, browser39 watch persists cookies, localStorage, and history to disk (`~/.local/share/browser39/session.enc`). This means:

- The agent can log in once and stay authenticated across restarts
- Browsing history is preserved for the `back`/`forward`/`history` actions
- Use `--no-persist` flag if you want a clean session each time

## Complete Examples

Ready-to-use single-file integrations with `web_search`, `visit_website`, tool dispatch, and LLM tool definitions:

- **[Python](../examples/browser39_tools.py)** — zero dependencies, `python3 examples/browser39_tools.py`
- **[TypeScript](../examples/browser39_tools.ts)** — zero dependencies, `npx tsx examples/browser39_tools.ts`
- **[Rust](../examples/browser39_tools.rs)** — only `serde_json`, copy into your project

Each file includes `TOOL_DEFINITIONS` / `tool_definitions()` with the JSON schemas for LLM tool-calling APIs (Anthropic, OpenAI, etc.).
