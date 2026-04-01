/**
 * browser39 tools — drop-in webSearch and visitWebsite for any LLM agent.
 *
 * Zero dependencies. Requires `browser39` in PATH.
 * Manages a long-running browser39 watch subprocess via JSONL files.
 *
 * Usage:
 *   import { webSearch, visitWebsite, dispatchTool, TOOL_DEFINITIONS } from "./browser39_tools";
 *
 *   // Pass TOOL_DEFINITIONS to your LLM's tool-calling API
 *   const results = await webSearch("rust programming");
 *   const page = await visitWebsite("https://example.com");
 *   const page = await visitWebsite("https://example.com", "article");
 */

import { spawn, execSync, ChildProcess } from "child_process";
import { mkdirSync, writeFileSync, appendFileSync, readFileSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";

// ---------------------------------------------------------------------------
// LLM tool definitions (Anthropic/OpenAI format)
// ---------------------------------------------------------------------------

export const TOOL_DEFINITIONS = [
  {
    name: "web_search",
    description: "Search the web. Returns up to 5 results with title and URL.",
    input_schema: {
      type: "object",
      properties: {
        query: { type: "string", description: "Search query" },
      },
      required: ["query"],
    },
  },
  {
    name: "visit_website",
    description:
      "Fetch a URL and return page content as markdown. " +
      "Without a selector, returns the page's content sections so you can " +
      'choose which to read. With a selector (e.g. "article", "main"), ' +
      "returns that section's content directly.",
    input_schema: {
      type: "object",
      properties: {
        url: { type: "string", description: "URL to fetch (http or https)" },
        selector: {
          type: "string",
          description:
            'CSS selector for a content section (e.g. "article", "main")',
        },
      },
      required: ["url"],
    },
  },
];

// ---------------------------------------------------------------------------
// BrowserClient — singleton managing browser39 watch subprocess
// ---------------------------------------------------------------------------

interface SearchResult {
  title: string;
  url: string;
}

class BrowserClient {
  private proc: ChildProcess | null = null;
  private seq = 0;
  private dir: string;
  private commandsPath: string;
  private resultsPath: string;

  constructor() {
    this.dir = join(tmpdir(), "browser39_agent");
    this.commandsPath = join(this.dir, "commands.jsonl");
    this.resultsPath = join(this.dir, "results.jsonl");
  }

  private ensureRunning(): void {
    if (this.proc && this.proc.exitCode === null) return;

    mkdirSync(this.dir, { recursive: true });
    writeFileSync(this.commandsPath, "");
    writeFileSync(this.resultsPath, "");
    this.seq = 0;

    this.proc = spawn(
      "browser39",
      ["watch", this.commandsPath, "--output", this.resultsPath],
      { stdio: "ignore" }
    );
    // Give it time to start watching
    execSync("sleep 0.3");
  }

  async send(action: string, fields: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    this.ensureRunning();
    this.seq++;
    const seq = this.seq;
    const cmd = { id: `cmd-${seq}`, action, v: 1, seq, ...fields };
    appendFileSync(this.commandsPath, JSON.stringify(cmd) + "\n");
    return this.waitForResult(seq);
  }

  private waitForResult(seq: number, timeoutMs = 35000): Promise<Record<string, unknown>> {
    const deadline = Date.now() + timeoutMs;
    return new Promise((resolve, reject) => {
      const poll = () => {
        if (Date.now() > deadline) {
          return reject(new Error("browser39: timeout"));
        }
        try {
          const content = readFileSync(this.resultsPath, "utf-8");
          const lines = content.split("\n").filter((l) => l.trim());
          // Only inspect last 2 lines
          const tail = lines.slice(-2);
          for (const line of tail.reverse()) {
            try {
              const obj = JSON.parse(line);
              if (obj.seq === seq) return resolve(obj);
            } catch {}
          }
        } catch {}
        setTimeout(poll, 100);
      };
      poll();
    });
  }

  quit(): void {
    if (this.proc && this.proc.exitCode === null) {
      this.seq++;
      const cmd = { id: "quit", action: "quit", v: 1, seq: this.seq };
      appendFileSync(this.commandsPath, JSON.stringify(cmd) + "\n");
      this.proc.kill();
      this.proc = null;
    }
  }
}

const client = new BrowserClient();

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/** Search the web via DuckDuckGo. Returns up to 5 results. */
export async function webSearch(query: string): Promise<SearchResult[]> {
  const encoded = encodeURIComponent(query).replace(/%20/g, "+");
  const url = `https://html.duckduckgo.com/html/?q=${encoded}`;
  const result = await client.send("fetch", {
    url,
    options: { max_tokens: 4000, strip_nav: true, show_selectors_first: false },
  });

  if (!result.ok) return [];

  const links = (result.links as Array<{ href?: string; text?: string }>) || [];
  const results: SearchResult[] = [];

  for (const link of links) {
    const href = link.href || "";
    const text = link.text || "";
    if (!text || !href.includes("uddg=")) continue;
    if (href.includes("ad_domain") || href.includes("ad_provider")) continue;

    const match = href
      .replace("?", "&")
      .split("&")
      .find((p) => p.startsWith("uddg="));
    if (match) {
      results.push({ title: text, url: decodeURIComponent(match.slice(5)) });
    }
    if (results.length >= 5) break;
  }
  return results;
}

/** Fetch a URL and return markdown. Pass a CSS selector to target a section. */
export async function visitWebsite(url: string, selector?: string): Promise<string> {
  const options: Record<string, unknown> = {
    max_tokens: 4000,
    strip_nav: true,
    include_links: true,
  };
  if (selector) {
    options.selector = selector;
    options.show_selectors_first = false;
  }
  const result = await client.send("fetch", { url, options });
  if (!result.ok) return (result.error as string) || "fetch failed";
  return (result.markdown as string) || "";
}

/** Dispatch an LLM tool call by name. */
export async function dispatchTool(name: string, args: Record<string, string>): Promise<string> {
  if (name === "web_search") {
    const results = await webSearch(args.query);
    return results.map((r) => `${r.title} | ${r.url}`).join("\n") || "no results found";
  } else if (name === "visit_website") {
    return await visitWebsite(args.url, args.selector);
  }
  return `unknown tool: ${name}`;
}

// ---------------------------------------------------------------------------
// Demo
// ---------------------------------------------------------------------------

if (process.argv[1]?.endsWith("browser39_tools.ts")) {
  (async () => {
    console.log("=== Tool Definitions ===");
    console.log(JSON.stringify(TOOL_DEFINITIONS, null, 2));

    console.log("\n=== webSearch('python asyncio') ===");
    const results = await webSearch("python asyncio");
    for (const r of results) {
      console.log(`  ${r.title.slice(0, 60)} | ${r.url.slice(0, 70)}`);
    }

    console.log("\n=== visitWebsite('https://example.com') ===");
    const md = await visitWebsite("https://example.com");
    console.log(`  (${md.length} chars) ${md.slice(0, 200)}`);

    client.quit();
    console.log("\nDone.");
  })();
}
