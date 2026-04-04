//! browser39 tools — drop-in web_search and visit_website for any LLM agent.
//!
//! Single-file example. Requires `browser39` in PATH, `serde_json` as dependency.
//! Manages a long-running browser39 watch subprocess via JSONL files.
//!
//! ```toml
//! [dependencies]
//! serde_json = "1"
//! ```
//!
//! Usage:
//!     let results = web_search("rust programming");
//!     let page = visit_website("https://example.com", None);
//!     let page = visit_website("https://example.com", Some("article"));
//!     let schemas = tool_definitions();

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

// ---------------------------------------------------------------------------
// LLM tool definitions (Anthropic/OpenAI format)
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> Value {
    serde_json::json!([
        {
            "name": "web_search",
            "description": "Search the web. Returns up to 5 results with title and URL.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "visit_website",
            "description": "Fetch a URL and return page content as markdown. Without a selector, returns the page's content sections so you can choose which to read. With a selector (e.g. \"article\", \"main\"), returns that section's content directly.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch (http or https)" },
                    "selector": { "type": "string", "description": "CSS selector for a content section (e.g. \"article\", \"main\")" }
                },
                "required": ["url"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// BrowserClient — singleton managing browser39 watch subprocess
// ---------------------------------------------------------------------------

static CLIENT: LazyLock<Mutex<BrowserClient>> = LazyLock::new(|| Mutex::new(BrowserClient::new()));

struct BrowserClient {
    process: Option<Child>,
    seq: u64,
    dir: PathBuf,
}

impl BrowserClient {
    fn new() -> Self {
        let dir = std::env::temp_dir().join("browser39_agent");
        Self {
            process: None,
            seq: 0,
            dir,
        }
    }

    fn commands_path(&self) -> PathBuf {
        self.dir.join("commands.jsonl")
    }
    fn results_path(&self) -> PathBuf {
        self.dir.join("results.jsonl")
    }

    fn ensure_running(&mut self) -> Result<(), String> {
        if let Some(ref mut child) = self.process {
            match child.try_wait() {
                Ok(Some(_)) => self.process = None,
                Ok(None) => return Ok(()),
                Err(_) => self.process = None,
            }
        }

        fs::create_dir_all(&self.dir).map_err(|e| format!("browser39: mkdir: {e}"))?;
        File::create(self.commands_path()).map_err(|e| format!("browser39: create: {e}"))?;
        File::create(self.results_path()).map_err(|e| format!("browser39: create: {e}"))?;
        self.seq = 0;

        let child = Command::new("browser39")
            .arg("watch")
            .arg(self.commands_path())
            .arg("--output")
            .arg(self.results_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("browser39: spawn: {e}"))?;

        self.process = Some(child);
        std::thread::sleep(Duration::from_millis(300));
        Ok(())
    }

    fn send(&mut self, action: &str, extra: Value) -> Result<Value, String> {
        self.ensure_running()?;
        self.seq += 1;
        let seq = self.seq;

        let mut cmd = serde_json::json!({
            "id": format!("cmd-{seq}"), "action": action, "v": 1, "seq": seq,
        });
        if let (Some(obj), Some(extra_obj)) = (cmd.as_object_mut(), extra.as_object()) {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }

        let mut f = OpenOptions::new()
            .append(true)
            .open(self.commands_path())
            .map_err(|e| format!("browser39: write: {e}"))?;
        writeln!(f, "{}", serde_json::to_string(&cmd).unwrap())
            .map_err(|e| format!("browser39: write: {e}"))?;
        f.flush().map_err(|e| format!("browser39: flush: {e}"))?;

        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(35) {
                return Err("browser39: timeout".into());
            }
            if let Ok(file) = File::open(self.results_path()) {
                let lines: Vec<String> = BufReader::new(file)
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

    fn quit(&mut self) {
        if let Some(ref mut child) = self.process {
            if child.try_wait().ok().flatten().is_none() {
                self.seq += 1;
                let cmd = serde_json::json!({"id":"quit","action":"quit","v":1,"seq":self.seq});
                if let Ok(mut f) = OpenOptions::new().append(true).open(self.commands_path()) {
                    let _ = writeln!(f, "{}", serde_json::to_string(&cmd).unwrap());
                }
                let _ = child.wait();
            }
            self.process = None;
        }
    }
}

fn send_command(action: &str, extra: Value) -> Result<Value, String> {
    CLIENT
        .lock()
        .map_err(|e| format!("lock: {e}"))?
        .send(action, extra)
}

// ---------------------------------------------------------------------------
// URL encoding (minimal, no extra deps)
// ---------------------------------------------------------------------------

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h << 4 | l) as char);
                i += 3;
            } else {
                out.push('%');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

pub struct SearchResult {
    pub title: String,
    pub url: String,
}

/// Search the web via DuckDuckGo. Returns up to 5 results.
pub fn web_search(query: &str) -> Result<Vec<SearchResult>, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", url_encode(query));
    let result = send_command(
        "fetch",
        serde_json::json!({
            "url": url,
            "options": {"max_tokens": 4000, "strip_nav": true, "show_selectors_first": false}
        }),
    )?;

    if result.get("ok") != Some(&Value::Bool(true)) {
        return Ok(vec![]);
    }

    let links = result["links"].as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();

    for link in &links {
        let href = link["href"].as_str().unwrap_or("");
        let text = link["text"].as_str().unwrap_or("");
        if text.is_empty() || !href.contains("uddg=") {
            continue;
        }
        if href.contains("ad_domain") || href.contains("ad_provider") {
            continue;
        }

        let real_url = href
            .replace('?', "&")
            .split('&')
            .find(|p| p.starts_with("uddg="))
            .map(|p| url_decode(&p[5..]));

        if let Some(real_url) = real_url {
            results.push(SearchResult {
                title: text.to_string(),
                url: real_url,
            });
        }
        if results.len() >= 5 {
            break;
        }
    }
    Ok(results)
}

/// Fetch a URL and return markdown. Pass a CSS selector to target a section.
pub fn visit_website(url: &str, selector: Option<&str>) -> Result<String, String> {
    let mut options = serde_json::json!({
        "max_tokens": 4000, "strip_nav": true, "include_links": true,
    });
    if let Some(sel) = selector {
        options["selector"] = Value::String(sel.to_string());
        options["show_selectors_first"] = Value::Bool(false);
    }
    let result = send_command("fetch", serde_json::json!({"url": url, "options": options}))?;
    if result.get("ok") != Some(&Value::Bool(true)) {
        return Err(result["error"]
            .as_str()
            .unwrap_or("fetch failed")
            .to_string());
    }
    Ok(result["markdown"].as_str().unwrap_or("").to_string())
}

/// Dispatch an LLM tool call by name. Returns the result as a string.
pub fn dispatch_tool(name: &str, args: &Value) -> Result<String, String> {
    match name {
        "web_search" => {
            let query = args["query"].as_str().ok_or("missing query")?;
            let results = web_search(query)?;
            if results.is_empty() {
                return Ok("no results found".into());
            }
            Ok(results
                .iter()
                .map(|r| format!("{} | {}", r.title, r.url))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "visit_website" => {
            let url = args["url"].as_str().ok_or("missing url")?;
            let selector = args.get("selector").and_then(|v| v.as_str());
            visit_website(url, selector)
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Demo
// ---------------------------------------------------------------------------

fn main() {
    println!("=== Tool Definitions ===");
    println!(
        "{}",
        serde_json::to_string_pretty(&tool_definitions()).unwrap()
    );

    println!("\n=== web_search(\"python asyncio\") ===");
    match web_search("python asyncio") {
        Ok(results) => {
            for r in &results {
                println!(
                    "  {} | {}",
                    &r.title[..r.title.len().min(60)],
                    &r.url[..r.url.len().min(70)]
                );
            }
        }
        Err(e) => println!("  error: {e}"),
    }

    println!("\n=== visit_website(\"https://example.com\") ===");
    match visit_website("https://example.com", None) {
        Ok(md) => println!("  ({} chars) {}", md.len(), &md[..md.len().min(200)]),
        Err(e) => println!("  error: {e}"),
    }

    CLIENT.lock().unwrap().quit();
    println!("\nDone.");
}
