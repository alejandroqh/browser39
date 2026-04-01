// OpenClaw native plugin for browser39
// Spawns `browser39 mcp` and communicates via MCP JSON-RPC over stdio.

import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";
import { Type } from "@sinclair/typebox";
import { spawn, type ChildProcess } from "node:child_process";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";

const CALL_TIMEOUT_MS = 120_000;

// ---------------------------------------------------------------------------
// MCP JSON-RPC client — talks to `browser39 mcp` over stdio
// ---------------------------------------------------------------------------

interface PendingEntry {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

interface McpClient {
  proc: ChildProcess;
  rl: ReadlineInterface;
  nextId: number;
  pending: Map<number, PendingEntry>;
  initialized: Promise<void>;
}

function destroyClient(client: McpClient) {
  client.rl.close();
  for (const [, entry] of client.pending) {
    clearTimeout(entry.timer);
    entry.reject(new Error("browser39 process exited"));
  }
  client.pending.clear();
  if (client.proc.exitCode === null) {
    client.proc.kill();
  }
}

function createMcpClient(binaryPath: string, configPath?: string): McpClient {
  const args = ["mcp"];
  if (configPath) {
    args.unshift("--config", configPath);
  }

  const proc = spawn(binaryPath, args, {
    stdio: ["pipe", "pipe", "ignore"],
  });

  const rl = createInterface({ input: proc.stdout! });
  const client: McpClient = {
    proc,
    rl,
    nextId: 0,
    pending: new Map(),
    initialized: Promise.resolve(),
  };

  rl.on("line", (line) => {
    try {
      const msg = JSON.parse(line);
      if (msg.id !== undefined) {
        const entry = client.pending.get(msg.id);
        if (entry) {
          clearTimeout(entry.timer);
          client.pending.delete(msg.id);
          if (msg.error) {
            entry.reject(new Error(msg.error.message || JSON.stringify(msg.error)));
          } else {
            entry.resolve(msg.result);
          }
        }
      }
    } catch {
      // ignore non-JSON lines
    }
  });

  proc.on("exit", () => destroyClient(client));

  // MCP initialize handshake
  client.initialized = (async () => {
    await rpcCall(client, "initialize", {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "openclaw-browser39", version: "1.0.0" },
    });
    proc.stdin!.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
  })();

  return client;
}

function rpcCall(client: McpClient, method: string, params?: unknown): Promise<unknown> {
  return new Promise((resolve, reject) => {
    client.nextId++;
    const id = client.nextId;
    const timer = setTimeout(() => {
      client.pending.delete(id);
      reject(new Error(`browser39 RPC timeout: ${method}`));
    }, CALL_TIMEOUT_MS);
    client.pending.set(id, { resolve, reject, timer });
    client.proc.stdin!.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
}

interface ToolResult {
  content?: Array<{ type: string; text?: string }>;
  isError?: boolean;
}

async function callTool(client: McpClient, toolName: string, args?: Record<string, unknown>): Promise<string> {
  await client.initialized;
  const result = await rpcCall(client, "tools/call", { name: toolName, arguments: args ?? {} }) as ToolResult;

  if (result.isError) {
    const text = result.content?.map((c) => c.text ?? "").join("\n") || "Unknown error";
    throw new Error(text);
  }

  return result.content?.map((c) => c.text ?? "").join("\n") || "";
}

// ---------------------------------------------------------------------------
// Shared parameter fragments
// ---------------------------------------------------------------------------

const OriginParam = Type.Optional(Type.String({ description: "Origin (scheme://host:port)" }));
const MaxTokensParam = Type.Optional(Type.Number({ description: "Maximum tokens to return" }));

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

export default definePluginEntry({
  id: "browser39",
  name: "browser39",
  description: "Headless web browser for AI agents — fetches pages as token-optimized markdown",

  register(api) {
    const config = api.pluginConfig as { binaryPath?: string; configPath?: string };
    const binaryPath = config.binaryPath || "browser39";
    const configPath = config.configPath;

    let client: McpClient | null = null;
    let clientCreating = false;

    function getClient(): McpClient {
      if (!client || client.proc.exitCode !== null) {
        if (clientCreating) return client!;
        clientCreating = true;
        client = createMcpClient(binaryPath, configPath);
        clientCreating = false;
      }
      return client;
    }

    function proxyTool(
      name: string,
      description: string,
      parameters: ReturnType<typeof Type.Object>,
    ) {
      api.registerTool({
        name,
        description,
        parameters,
        async execute(_id, params) {
          const text = await callTool(getClient(), name, params as Record<string, unknown>);
          return { content: [{ type: "text" as const, text }] };
        },
      });
    }

    // -- fetch ---------------------------------------------------------------
    proxyTool("browser39_fetch",
      "Fetch a URL and return the page as token-optimized markdown",
      Type.Object({
        url: Type.String({ description: "URL to fetch" }),
        method: Type.Optional(Type.Union([
          Type.Literal("GET"), Type.Literal("POST"), Type.Literal("PUT"),
          Type.Literal("PATCH"), Type.Literal("DELETE"),
        ], { description: "HTTP method. Defaults to GET." })),
        body: Type.Optional(Type.String({ description: "Request body (for POST/PUT/PATCH)" })),
        auth_profile: Type.Optional(Type.String({ description: "Auth profile name from config" })),
        headers: Type.Optional(Type.Record(Type.String(), Type.String(), { description: "Additional HTTP headers" })),
        max_tokens: MaxTokensParam,
        selector: Type.Optional(Type.String({ description: "CSS selector to extract specific content" })),
        offset: Type.Optional(Type.Number({ description: "Byte offset for pagination" })),
        show_selectors_first: Type.Optional(Type.Boolean({ description: "Return available selectors instead of full content (default: true)" })),
        download_path: Type.Optional(Type.String({ description: "File path to save binary content to disk" })),
      }),
    );

    // -- click ---------------------------------------------------------------
    proxyTool("browser39_click",
      "Follow a link on the current page by index number or link text",
      Type.Object({
        index: Type.Optional(Type.Number({ description: "Link index number (from browser39_links)" })),
        text: Type.Optional(Type.String({ description: "Link text to match (substring, case-insensitive)" })),
        max_tokens: MaxTokensParam,
      }),
    );

    // -- links ---------------------------------------------------------------
    proxyTool("browser39_links",
      "List all links on the current page",
      Type.Object({}),
    );

    // -- dom_query ------------------------------------------------------------
    proxyTool("browser39_dom_query",
      "Query the DOM with a CSS selector or JavaScript",
      Type.Object({
        selector: Type.Optional(Type.String({ description: "CSS selector to query" })),
        script: Type.Optional(Type.String({ description: "JavaScript to execute against the page DOM" })),
        attr: Type.Optional(Type.String({ description: "Attribute to extract (textContent, innerHTML, href, src, or any attribute)" })),
      }),
    );

    // -- fill ----------------------------------------------------------------
    proxyTool("browser39_fill",
      "Fill form field(s) by CSS selector",
      Type.Object({
        selector: Type.Optional(Type.String({ description: "CSS selector for a single field" })),
        value: Type.Optional(Type.String({ description: "Value for the single field" })),
        fields: Type.Optional(Type.Array(Type.Object({
          selector: Type.String({ description: "CSS selector for the field" }),
          value: Type.String({ description: "Value to fill" }),
        }), { description: "Array of fields to fill" })),
      }),
    );

    // -- submit --------------------------------------------------------------
    proxyTool("browser39_submit",
      "Submit a form by CSS selector",
      Type.Object({
        selector: Type.String({ description: "CSS selector for the form element" }),
        max_tokens: MaxTokensParam,
      }),
    );

    // -- cookies -------------------------------------------------------------
    proxyTool("browser39_cookies",
      "List cookies for the current session",
      Type.Object({
        domain: Type.Optional(Type.String({ description: "Filter cookies by domain" })),
      }),
    );

    proxyTool("browser39_set_cookie",
      "Set a cookie",
      Type.Object({
        name: Type.String({ description: "Cookie name" }),
        value: Type.String({ description: "Cookie value" }),
        domain: Type.String({ description: "Cookie domain" }),
        path: Type.Optional(Type.String({ description: "Cookie path (defaults to /)" })),
        secure: Type.Optional(Type.Boolean({ description: "Require HTTPS" })),
        http_only: Type.Optional(Type.Boolean({ description: "HTTP-only (no JS access)" })),
        max_age_secs: Type.Optional(Type.Number({ description: "Expiration in seconds" })),
      }),
    );

    proxyTool("browser39_delete_cookie",
      "Delete a cookie",
      Type.Object({
        name: Type.String({ description: "Cookie name to delete" }),
        domain: Type.String({ description: "Domain of the cookie" }),
      }),
    );

    // -- storage -------------------------------------------------------------
    proxyTool("browser39_storage_get",
      "Get a localStorage value",
      Type.Object({
        key: Type.String({ description: "Storage key to retrieve" }),
        origin: OriginParam,
      }),
    );

    proxyTool("browser39_storage_set",
      "Set a localStorage value",
      Type.Object({
        key: Type.String({ description: "Storage key" }),
        value: Type.String({ description: "Value to store" }),
        origin: OriginParam,
      }),
    );

    proxyTool("browser39_storage_delete",
      "Delete a localStorage key",
      Type.Object({
        key: Type.String({ description: "Storage key to delete" }),
        origin: OriginParam,
      }),
    );

    proxyTool("browser39_storage_list",
      "List localStorage entries",
      Type.Object({ origin: OriginParam }),
    );

    proxyTool("browser39_storage_clear",
      "Clear localStorage for an origin",
      Type.Object({ origin: OriginParam }),
    );

    // -- search --------------------------------------------------------------
    proxyTool("browser39_search",
      "Search the web using the configured search engine (default: DuckDuckGo)",
      Type.Object({
        query: Type.String({ description: "Search query string" }),
        max_tokens: MaxTokensParam,
      }),
    );

    // -- navigation ----------------------------------------------------------
    proxyTool("browser39_back", "Navigate back in history", Type.Object({}));
    proxyTool("browser39_forward", "Navigate forward in history", Type.Object({}));

    proxyTool("browser39_history",
      "Search or list browsing history",
      Type.Object({
        query: Type.Optional(Type.String({ description: "Text to search in URLs and titles" })),
        limit: Type.Optional(Type.Number({ description: "Max entries to return (default: 10)" })),
      }),
    );

    proxyTool("browser39_info", "Get session info and liveness status", Type.Object({}));

    // -- cleanup on gateway shutdown -----------------------------------------
    api.on("shutdown", () => {
      if (client) destroyClient(client);
    });

    api.logger.info("browser39 plugin registered");
  },
});
