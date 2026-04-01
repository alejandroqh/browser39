"""
browser39 tools — drop-in web_search and visit_website for any LLM agent.

Zero dependencies. Requires `browser39` in PATH.
Manages a long-running browser39 watch subprocess via JSONL files.

Usage:
    from browser39_tools import web_search, visit_website, TOOL_DEFINITIONS

    # Pass TOOL_DEFINITIONS to your LLM's tool-calling API
    results = web_search("rust programming")
    page = visit_website("https://example.com")
    page = visit_website("https://example.com", selector="article")
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time
import urllib.parse

# ---------------------------------------------------------------------------
# LLM tool definitions (Anthropic/OpenAI format)
# ---------------------------------------------------------------------------

TOOL_DEFINITIONS = [
    {
        "name": "web_search",
        "description": "Search the web. Returns up to 5 results with title and URL.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"}
            },
            "required": ["query"],
        },
    },
    {
        "name": "visit_website",
        "description": (
            "Fetch a URL and return page content as markdown. "
            "Without a selector, returns the page's content sections so you can "
            'choose which to read. With a selector (e.g. "article", "main"), '
            "returns that section's content directly."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to fetch (http or https)"},
                "selector": {
                    "type": "string",
                    "description": 'CSS selector for a content section (e.g. "article", "main")',
                },
            },
            "required": ["url"],
        },
    },
]

# ---------------------------------------------------------------------------
# BrowserClient — singleton managing browser39 watch subprocess
# ---------------------------------------------------------------------------

class BrowserClient:
    def __init__(self):
        self._proc = None
        self._seq = 0
        self._dir = os.path.join(tempfile.gettempdir(), "browser39_agent")
        self._commands = os.path.join(self._dir, "commands.jsonl")
        self._results = os.path.join(self._dir, "results.jsonl")

    def _ensure_running(self):
        if self._proc and self._proc.poll() is None:
            return
        os.makedirs(self._dir, exist_ok=True)
        open(self._commands, "w").close()
        open(self._results, "w").close()
        self._seq = 0
        self._proc = subprocess.Popen(
            ["browser39", "watch", self._commands, "--output", self._results],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(0.3)

    def send(self, action: str, **fields) -> dict:
        self._ensure_running()
        self._seq += 1
        seq = self._seq
        cmd = {"id": f"cmd-{seq}", "action": action, "v": 1, "seq": seq, **fields}
        with open(self._commands, "a") as f:
            f.write(json.dumps(cmd) + "\n")
            f.flush()
        return self._wait(seq)

    def _wait(self, seq: int, timeout: float = 35.0) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                with open(self._results) as f:
                    lines = [l for l in f if l.strip()]
                for line in reversed(lines[-2:]):
                    obj = json.loads(line)
                    if obj.get("seq") == seq:
                        return obj
            except (json.JSONDecodeError, FileNotFoundError):
                pass
            time.sleep(0.1)
        raise TimeoutError("browser39: timeout waiting for result")

    def quit(self):
        if self._proc and self._proc.poll() is None:
            self._seq += 1
            cmd = {"id": "quit", "action": "quit", "v": 1, "seq": self._seq}
            with open(self._commands, "a") as f:
                f.write(json.dumps(cmd) + "\n")
            self._proc.wait(timeout=5)
            self._proc = None


_client = BrowserClient()

# ---------------------------------------------------------------------------
# Tools
# ---------------------------------------------------------------------------

def web_search(query: str) -> list[dict]:
    """Search the web via DuckDuckGo. Returns list of {"title": ..., "url": ...}."""
    encoded = urllib.parse.quote_plus(query)
    url = f"https://html.duckduckgo.com/html/?q={encoded}"
    result = _client.send(
        "fetch", url=url,
        options={"max_tokens": 4000, "strip_nav": True, "show_selectors_first": False},
    )
    if not result.get("ok"):
        return []

    results = []
    for link in result.get("links", []):
        href = link.get("href", "")
        text = link.get("text", "")
        if not text or "uddg=" not in href:
            continue
        if "ad_domain" in href or "ad_provider" in href:
            continue
        # Extract real URL from DDG redirect
        for part in href.replace("?", "&").split("&"):
            if part.startswith("uddg="):
                real_url = urllib.parse.unquote(part[5:])
                results.append({"title": text, "url": real_url})
                break
        if len(results) >= 5:
            break
    return results


def visit_website(url: str, selector: str | None = None) -> str:
    """Fetch a URL and return markdown. Pass a CSS selector to target a section."""
    options = {"max_tokens": 4000, "strip_nav": True, "include_links": True}
    if selector:
        options["selector"] = selector
        options["show_selectors_first"] = False
    result = _client.send("fetch", url=url, options=options)
    if not result.get("ok"):
        return result.get("error", "fetch failed")
    return result.get("markdown", "")


def dispatch_tool(name: str, args: dict) -> str:
    """Dispatch an LLM tool call by name. Returns the result as a string."""
    if name == "web_search":
        results = web_search(args["query"])
        return "\n".join(f"{r['title']} | {r['url']}" for r in results) or "no results found"
    elif name == "visit_website":
        return visit_website(args["url"], args.get("selector"))
    else:
        return f"unknown tool: {name}"


# ---------------------------------------------------------------------------
# Demo
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import atexit
    atexit.register(_client.quit)

    print("=== Tool Definitions ===")
    print(json.dumps(TOOL_DEFINITIONS, indent=2))

    print("\n=== web_search('python asyncio') ===")
    for r in web_search("python asyncio"):
        print(f"  {r['title'][:60]} | {r['url'][:70]}")

    print("\n=== visit_website('https://example.com') ===")
    md = visit_website("https://example.com")
    print(f"  ({len(md)} chars) {md[:200]}")

    _client.quit()
    print("\nDone.")
