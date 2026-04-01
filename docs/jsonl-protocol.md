# browser39 Protocol v0.1

Defines the action schema for browser39. Two independent transports exist — an agent uses one or the other, never both.

- **JSONL transport** — for non-MCP agents (Python scripts, custom harnesses, any language)
- **MCP transport** — for AI agents (Claude, LangChain, any MCP client)

Both transports call the same `BrowserService` underneath, but each is self-sufficient and optimized for its audience.

---

# Part 1: Shared Core

## Actions

These actions are the same regardless of transport. The transport layer wraps them differently.

### `fetch` — Load a page or follow a link

Three modes (exactly one required):

| Mode | Field | Description |
|------|-------|-------------|
| By URL | `url` | Fetch a new URL |
| By link index | `index` | Follow link N from current page |
| By link text | `text` | Follow first link matching text |

**Precedence:** If multiple fields are present, `url` wins, then `index`, then `text`.

**`options` and `headers` apply to ALL modes** — including `index`/`text`. The agent can control `max_tokens`, `selector`, `headers`, etc. regardless of how it navigates.

**Fields:**
```json
{
  "url": "https://example.com",
  "index": 3,
  "text": "More info",
  "method": "GET",
  "body": null,
  "auth_profile": "github",
  "headers": {
    "Accept-Language": "en-US"
  },
  "options": {
    "max_tokens": 4000,
    "offset": 0,
    "selector": "article",
    "strip_nav": true,
    "include_links": true,
    "include_images": false,
    "timeout_secs": 30
  }
}
```

Option defaults:
- `method`: `GET` (supports `GET`, `POST`, `PUT`, `PATCH`, `DELETE`)
- `body`: `null` (string — agent handles encoding; only with `url` mode)
- `auth_profile`: `null` (string — name of auth profile from config; see **Security**)
- `max_tokens`: unlimited
- `offset`: 0 (start of content)
- `selector`: none (full page)
- `strip_nav`: true
- `include_links`: true
- `include_images`: false
- `timeout_secs`: 30 (wall-clock — covers DNS + connect + transfer + processing)

**Manual POST example** (for JS-style forms / API calls):
```json
{
  "action": "fetch",
  "url": "https://api.example.com/login",
  "method": "POST",
  "headers": {"Content-Type": "application/json"},
  "body": "{\"username\":\"agent\",\"password\":\"secret\"}"
}
```

**Pagination:** When `max_tokens` truncates content, the result includes `"truncated": true` and `"next_offset": N`. The agent can re-fetch with `{"action": "fetch", "url": "same-url", "options": {"offset": N, "max_tokens": 4000}}` to get the next chunk. Offset is in estimated tokens, not bytes.

**Result:**
```json
{
  "url": "https://example.com",
  "title": "Example Domain",
  "status": 200,
  "markdown": "# Example Domain\n\nThis domain is for use in illustrative examples...",
  "links": [
    {"i": 0, "text": "More information", "href": "https://www.iana.org/domains/example"}
  ],
  "meta": {
    "lang": "en",
    "description": "Example domain for documentation",
    "content_type": "text/html"
  },
  "stats": {
    "fetch_ms": 230,
    "tokens_est": 42,
    "content_bytes": 1256
  },
  "truncated": false,
  "next_offset": null
}
```

---

### `links` — List links on current page (lightweight)

Returns only the links from the current page without re-rendering markdown. Cheap operation — reads from session cache, no network request.

**Result:**
```json
{
  "links": [
    {"i": 0, "text": "More information", "href": "https://www.iana.org/domains/example"},
    {"i": 1, "text": "IANA", "href": "https://www.iana.org/"}
  ],
  "count": 2
}
```

---

### `dom_query` — Query the current page DOM

Two modes: **CSS selector** (simple, reliable) or **script** (flexible, boa_engine).

**Mode 1 — CSS selector (recommended):**
```json
{
  "selector": "h1",
  "attr": "textContent"
}
```

Returns matching elements. `attr` options: `textContent`, `innerHTML`, `href`, `src`, or any HTML attribute name. Default: `textContent`.

**Result:**
```json
{
  "results": ["Example Domain"],
  "count": 1,
  "exec_ms": 2
}
```

Multiple matches:
```json
{
  "selector": "a",
  "attr": "href"
}
```
```json
{
  "results": ["https://www.iana.org/domains/example", "https://www.iana.org/"],
  "count": 2,
  "exec_ms": 3
}
```

**Mode 2 — Script (advanced, boa_engine):**
```json
{
  "script": "document.querySelectorAll('a').length"
}
```

```json
{
  "result": 2,
  "type": "number",
  "exec_ms": 15
}
```

**boa_engine limitations:** This runs a pure-Rust JS engine against the static parsed HTML. It supports `document.title`, `document.querySelector()`, `document.querySelectorAll()`, and element properties (`textContent`, `innerHTML`, `getAttribute()`, `href`). It does NOT execute page scripts, React/Angular/SPA code, `fetch()`, `setTimeout()`, or any Web API beyond basic DOM traversal. When in doubt, use the CSS selector mode.

**Error:**
```json
{"ok": false, "error": "ReferenceError: foo is not defined", "code": "DOM_QUERY_ERROR"}
```

---

### `fill` — Fill form fields

Set values on form fields in the in-memory DOM. Values are stored in session state and used when submitting.

**Single field:**
```json
{
  "selector": "#username",
  "value": "agent@example.com"
}
```

**Multiple fields:**
```json
{
  "fields": [
    {"selector": "#username", "value": "agent@example.com"},
    {"selector": "#password", "value": "secret123", "sensitive": true},
    {"selector": "select#country", "value": "US"}
  ]
}
```

Supports `<input>`, `<textarea>`, `<select>` elements. If both `selector`/`value` and `fields` are present, `fields` wins.

**`sensitive` flag:** When `true`, the value is stored in the secret store and never echoed back in results. The field is filled normally, but the value is redacted in any output (results, MCP responses, `dom_query` reads). Use for passwords, tokens, and other credentials. See **Security**.

**Result:**
```json
{"ok": true, "filled": 2}
```

Error: `NO_PAGE` if no page loaded, `SELECTOR_NOT_FOUND` if a selector doesn't match.

---

### `submit` — Submit a form

Finds the form element, collects all named fields (including values set by `fill`), builds and sends the HTTP request.

**Fields:**
```json
{
  "selector": "form#login"
}
```

How it works:
1. Finds `<form>` by CSS selector
2. Reads `action` (URL), `method` (GET/POST), `enctype`
3. Collects all named fields (inputs, selects, textareas) with their current values
4. Builds HTTP request (`application/x-www-form-urlencoded` or `multipart/form-data`)
5. Sends request and returns the response page

**Result:** Same shape as `fetch` — `url`, `title`, `status`, `markdown`, `links`, `meta`, `stats`, `truncated`, `next_offset`.

Error: `FORM_NOT_FOUND` if selector doesn't match a `<form>` element. `NO_PAGE` if no page loaded.

---

### `cookies` — List cookies

Returns cookies from the session cookie jar. Optionally filtered by domain.

**Fields:**
```json
{"domain": "example.com"}
```

`domain` is optional — omit to get all cookies.

**Result:**
```json
{
  "cookies": [
    {"name": "session", "value": "abc123", "domain": "example.com", "path": "/", "secure": true, "http_only": true, "expires": "2026-12-31T23:59:59Z"},
    {"name": "lang", "value": "en", "domain": "example.com", "path": "/", "secure": false, "http_only": false, "expires": null}
  ],
  "count": 2
}
```

---

### `set_cookie` — Set a cookie

Inserts or updates a cookie in the session cookie jar.

**Fields:**
```json
{
  "name": "token",
  "value": "xyz789",
  "domain": "example.com",
  "path": "/",
  "secure": true,
  "http_only": false,
  "max_age_secs": 3600,
  "sensitive": true
}
```

Required: `name`, `value`, `domain`. All other fields optional (`path` defaults to `/`, `secure` defaults to `false`, `http_only` defaults to `false`). When `sensitive: true`, the cookie value is stored but redacted in all outputs. See **Security**.

**Result:** `{"ok": true}`

Error: `COOKIE_ERROR` if domain is invalid or cookie can't be built.

---

### `delete_cookie` — Delete a cookie

**Fields:**
```json
{"name": "token", "domain": "example.com"}
```

Both `name` and `domain` required.

**Result:** `{"ok": true, "deleted": true}` (or `"deleted": false` if cookie didn't exist)

---

### `storage_get` — Get a localStorage value

**Fields:**
```json
{"key": "user_pref"}
```

Uses current page's origin. Optional `origin` override: `{"key": "x", "origin": "https://example.com"}`

**Result:** `{"ok": true, "key": "user_pref", "value": "dark_mode"}` — `value` is `null` if key not found.

Error: `NO_PAGE` if no page loaded and no `origin` specified.

---

### `storage_set` — Set a localStorage value

**Fields:**
```json
{"key": "user_pref", "value": "dark_mode", "sensitive": false}
```

Optional `origin` override. Both `key` and `value` are strings. When `sensitive: true`, the value is stored but redacted in `storage_get`/`storage_list` outputs and accessible only via opaque handle. See **Security**.

**Result:** `{"ok": true}`

---

### `storage_delete` — Delete a localStorage key

**Fields:**
```json
{"key": "user_pref"}
```

Optional `origin` override.

**Result:** `{"ok": true, "deleted": true}` (or `"deleted": false` if key didn't exist)

---

### `storage_list` — List all localStorage entries

**Fields:**
```json
{}
```

Optional `origin` override. Returns all key-value pairs for the origin.

**Result:**
```json
{"ok": true, "origin": "https://example.com", "entries": {"user_pref": "dark_mode", "token": "abc"}, "count": 2}
```

---

### `storage_clear` — Clear localStorage for an origin

**Fields:**
```json
{}
```

Optional `origin` override.

**Result:** `{"ok": true, "cleared": 5}`

---

### `history` — Search or list browsing history

Returns visited pages with optional text search. Most recent pages first.

**Fields:**
```json
{
  "query": "google",
  "limit": 10
}
```

Both fields optional. `query` searches URLs and titles (case-insensitive). `limit` defaults to 10.

**Result:**
```json
{
  "entries": [
    {"index": 2, "url": "https://google.com", "title": "Google", "status": 200, "current": true},
    {"index": 0, "url": "https://google.com/search?q=rust", "title": "rust - Google Search", "status": 200, "current": false}
  ],
  "count": 2,
  "total": 5
}
```

`index` is the position in the history stack (usable with `back`/`forward`). `current` marks the active page. `total` is the full history size regardless of query/limit.

---

### `back` / `forward` — Navigate history

Returns the page at that history position (same shape as `fetch` result).
Error with `NO_HISTORY` if no history to navigate.

---

### `info` — Session state and liveness

Also serves as heartbeat — if browser39 responds, it's alive.

**Result:**
```json
{
  "alive": true,
  "current_url": "https://example.com",
  "title": "Example Domain",
  "history_length": 3,
  "history_index": 1,
  "cookies_count": 5,
  "uptime_secs": 120
}
```

If no page loaded yet, `current_url` and `title` are `null`, `alive` is still `true`.

---

### `quit` — Shut down

Graceful shutdown. Kast39 writes the result then exits.

---

## Error Codes

| Code | Meaning |
|------|---------|
| `UNKNOWN_ACTION` | Unrecognized action string |
| `INVALID_COMMAND` | Malformed JSON, missing required fields, or conflicting modes |
| `HTTP_ERROR` | HTTP request failed (network, DNS, TLS) |
| `TIMEOUT` | Wall-clock timeout exceeded |
| `INVALID_URL` | URL could not be parsed |
| `NO_PAGE` | Action requires a current page but none loaded |
| `NO_HISTORY` | back/forward with no history to navigate |
| `LINK_NOT_FOUND` | fetch by index/text doesn't match any link |
| `DOM_QUERY_ERROR` | DOM query execution or selector error |
| `SELECTOR_NOT_FOUND` | fill/submit selector doesn't match any element |
| `FORM_NOT_FOUND` | submit selector doesn't match a `<form>` element |
| `COOKIE_ERROR` | Cookie operation failed (invalid domain, parse error) |
| `STORAGE_ERROR` | Storage operation failed |
| `AUTH_PROFILE_NOT_FOUND` | auth_profile name doesn't exist in config |
| `AUTH_PROFILE_DOMAIN_MISMATCH` | Request URL domain not in profile's allowed domains |
| `SESSION_ERROR` | Internal session state error |

**Retry guidance:** On `TIMEOUT` or `HTTP_ERROR`, the agent may retry the same command (same `id` is fine — results are append-only, duplicates are harmless). On `INVALID_COMMAND` or `UNKNOWN_ACTION`, retrying the same command will always fail.

---

## Configuration

All browser39 configuration lives in a single file: `~/.config/browser39/config.toml`

Override with `--config <path>` or `BROWSER39_CONFIG` env var. See [`docs/config.md`](config.md) for the full reference.

Key sections:

| Section | Purpose |
|---------|---------|
| `[session]` | Start page, user-agent, timeouts, default fetch options |
| `[auth.<name>]` | Auth profiles — credentials stored outside the LLM conversation |
| `[[cookies]]` | Preloaded cookies — injected before first request |
| `[[storage]]` | Preloaded localStorage — injected before first request |
| `[[headers]]` | Default headers — sent with every request to matching domains |
| `[security]` | Redaction patterns, sensitive cookies/headers, transport behavior |

**Loading order:** config → resolve env vars → inject cookies → inject storage → register headers → fetch `start_url` → accept commands.

---

## Security — Preventing Secret Leaks

When browser39 is used by an LLM (via MCP), secrets (passwords, JWTs, session tokens) must not leak into the LLM's context window. Three layers work together to prevent this.

### Layer 1: Auth Profiles

Pre-configured credentials stored outside the LLM conversation. The LLM references a profile name — never sees the actual token.

**Config** (`~/.config/browser39/config.toml`):
```toml
[auth.github]
header = "Authorization"
value = "Bearer ghp_xxxxxxxxxxxx"
domains = ["api.github.com", "github.com"]

[auth.internal]
header = "X-API-Key"
value_env = "INTERNAL_API_KEY"        # read from environment variable
domains = ["internal.company.com"]
```

**Usage:**
```json
{"action": "fetch", "url": "https://api.github.com/repos", "auth_profile": "github"}
```

browser39 resolves the profile, verifies the domain matches, and attaches the header. The token never appears in any result or response.

**Domain enforcement:** If the request URL doesn't match the profile's `domains` list, browser39 returns `AUTH_PROFILE_DOMAIN_MISMATCH` error. This prevents an LLM from exfiltrating credentials to an attacker-controlled domain. Wildcards supported: `*.example.com`.

### Layer 2: Secret Handles (Opaque References)

When browser39 encounters a secret value (JWT in response, token in cookie), it stores the value internally and returns an opaque handle. The LLM can reference the handle without knowing the actual value.

**Auto-detection:** browser39 scans response content using patterns from `[security.patterns]` in the config. Built-in patterns cover JWT, GitHub PAT, OpenAI keys, Slack tokens, Stripe keys, AWS keys.

**Handle format:** `${browser39_secret_N}` where N is a monotonically increasing integer.

**Example flow:**
```json
// Agent logs in via POST
{"action": "fetch", "url": "https://api.example.com/login", "method": "POST",
 "headers": {"Content-Type": "application/json"},
 "body": "{\"user\":\"agent\",\"pass\":\"secret\"}"}

// Response — browser39 detects JWT in body, redacts it
{"ok": true, "markdown": "Login successful.\n\nToken: ${browser39_secret_1}", ...}

// Agent uses the handle — browser39 resolves to real token
{"action": "fetch", "url": "https://api.example.com/data",
 "headers": {"Authorization": "Bearer ${browser39_secret_1}"}}
```

**Handle resolution:** browser39 scans all string fields in the command for `${browser39_secret_N}` patterns and replaces them with actual values before executing. Handles are valid for the session lifetime.

**Explicit marking:** The agent can also create handles manually via the `sensitive` flag:
```json
{"action": "set_cookie", "name": "token", "value": "real-value", "domain": "x.com", "sensitive": true}
// Value stored as browser39_secret_N, redacted in cookies results
```

### Layer 3: Redaction Rules

Configurable patterns from `[security]` in the config:

```toml
[security]
sensitive_cookies = ["session", "sid", "token", "jwt", "auth", "csrf"]
sensitive_headers = ["authorization", "x-api-key", "cookie", "set-cookie"]

[security.patterns]
jwt = 'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}'
github_pat = 'ghp_[A-Za-z0-9]{36}'
openai_key = 'sk-[A-Za-z0-9]{32,}'

[security.mcp]
redact = true           # always on, cannot be disabled

[security.jsonl]
redact = false           # off by default for non-LLM agents
```

**Redacted output examples:**

Cookie values:
```json
{"cookies": [
  {"name": "session", "value": "••••••", "domain": "example.com", "handle": "${browser39_secret_3}"}
]}
```

Page content:
```json
{"markdown": "Your API key is: ${browser39_secret_4}\n\nUse it in the Authorization header."}
```

Storage:
```json
{"ok": true, "key": "token", "value": "••••••", "handle": "${browser39_secret_5}"}
```

### Transport-Specific Behavior

| Behavior | JSONL (non-LLM) | MCP (LLM) |
|----------|-----------------|-----------|
| Cookie values | Visible by default | Always redacted |
| JWT in page content | Visible by default | Auto-redacted → handle |
| Auth profiles | Supported | Supported |
| Secret handles | Available | Auto-generated |
| `sensitive` flag | Respected | Respected |
| Preloaded cookies/storage | Loaded from config | Loaded from config |

JSONL agents are typically scripts that the developer controls — they can opt into redaction via config but don't need it by default. MCP agents are LLMs — redaction is always on and cannot be disabled.

### Pre-Authenticated Startup

The config supports preloading credentials so the agent starts already authenticated — no login flow needed:

```toml
[session]
start_url = "https://app.example.com/dashboard"

[auth.app]
header = "Authorization"
value_env = "APP_JWT"
value_prefix = "Bearer "
domains = ["app.example.com"]

[[cookies]]
name = "session"
value_env = "SESSION_TOKEN"
domain = "app.example.com"
sensitive = true

[[storage]]
origin = "https://app.example.com"
key = "api_token"
value_env = "API_TOKEN"
sensitive = true
```

On startup: cookies and storage are injected → `start_url` is fetched with credentials already active → agent begins with the dashboard loaded and ready.

### Secure Login Flow (MCP, without pre-auth)

```
1. LLM calls: browser39_fetch(url: "https://app.com/login")
   → browser39 returns login page markdown (form visible)

2. LLM calls: browser39_fill(fields: [
     {selector: "#user", value: "agent"},
     {selector: "#pass", value: "secret123", sensitive: true}
   ])
   → browser39 stores password as browser39_secret_1, returns {"filled": 2}
   → LLM provided the password, but it's never echoed back

3. LLM calls: browser39_submit(selector: "form#login")
   → Server responds with Set-Cookie: session=abc123
   → browser39 stores cookie, redacts in response
   → Returns page markdown (dashboard)

4. LLM calls: browser39_fetch(url: "https://app.com/api/data")
   → Cookie jar sends session cookie automatically
   → LLM never saw the session token

5. LLM calls: browser39_cookies()
   → {"cookies": [{"name": "session", "value": "••••••", "handle": "${browser39_secret_2}"}]}
```

The LLM successfully logged in and browsed authenticated pages **without ever seeing the session token**.

---

# Part 2: JSONL Transport

For non-MCP agents. Any language that can read/write files.

## Files

Two append-only JSONL files (one JSON object per line):
- `commands.jsonl` — agent appends commands
- `results.jsonl` — browser39 appends results

## Command Envelope

```json
{"id": "<unique>", "action": "<action>", "v": 1, "seq": 1, ...action fields}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique identifier, echoed in result |
| `action` | string | yes | The action to perform |
| `v` | integer | yes | Protocol version, always `1` |
| `seq` | integer | yes | Monotonically increasing sequence number (1, 2, 3...) |

## Result Envelope

```json
{"id": "<matching-id>", "ok": true, "seq": 1, ...result fields}
```

| Field | Type | Always present | Description |
|-------|------|----------------|-------------|
| `id` | string | yes | Echoes command's id |
| `ok` | boolean | yes | Success or failure |
| `seq` | integer | yes | Echoes command's seq |
| `error` | string | on failure | Human-readable error |
| `code` | string | on failure | Machine-readable error code |

Unknown fields are ignored. Unknown actions return `UNKNOWN_ACTION`.

## Consumption Model

The `seq` field solves read-offset tracking. The agent knows the last `seq` it sent, and scans `results.jsonl` for matching `seq` values. To find new results after reconnect, the agent reads from the end of `results.jsonl` looking for the highest `seq` it hasn't processed.

## File Lifecycle

- **Atomic writes:** browser39 writes each result line in a single `write()` syscall + `fsync()`. No partial lines on crash.
- **Rotation:** When `results.jsonl` exceeds 10MB (configurable), browser39 rotates to `results.jsonl.1`. Agent should tolerate missing old entries.
- **Cleanup:** On `quit`, agent may delete both files. On startup, browser39 truncates both files if they exist (fresh session).
- **Disk full:** browser39 writes error to stderr and continues processing (results are lost but commands still execute).

## CLI Modes

```bash
browser39 fetch <url> [--output text|json]           # one-shot, no files
browser39 batch <commands.jsonl> [--output f.jsonl]   # process file and exit
browser39 watch <commands.jsonl> [--output f.jsonl]   # long-running file watcher
```

## Full JSONL Example

```jsonl
# commands.jsonl
{"id":"a","action":"fetch","v":1,"seq":1,"url":"https://news.ycombinator.com","options":{"max_tokens":2000}}
{"id":"b","action":"links","v":1,"seq":2}
{"id":"c","action":"fetch","v":1,"seq":3,"index":5}
{"id":"d","action":"dom_query","v":1,"seq":4,"selector":"h1","attr":"textContent"}
{"id":"e","action":"back","v":1,"seq":5}
{"id":"f","action":"info","v":1,"seq":6}
{"id":"g","action":"quit","v":1,"seq":7}
```

```jsonl
# results.jsonl
{"id":"a","ok":true,"seq":1,"url":"https://news.ycombinator.com","title":"Hacker News","status":200,"markdown":"# Hacker News\n...","links":[{"i":0,"text":"Show HN: ...","href":"..."}],"meta":{...},"stats":{"fetch_ms":450,"tokens_est":1800,"content_bytes":45000},"truncated":false,"next_offset":null}
{"id":"b","ok":true,"seq":2,"links":[{"i":0,"text":"Show HN: ...","href":"..."},...],"count":30}
{"id":"c","ok":true,"seq":3,"url":"https://example.com/show-hn","title":"Show HN: ...","status":200,"markdown":"...","links":[...],"meta":{...},"stats":{...},"truncated":false,"next_offset":null}
{"id":"d","ok":true,"seq":4,"results":["Show HN: browser39"],"count":1,"exec_ms":2}
{"id":"e","ok":true,"seq":5,"url":"https://news.ycombinator.com","title":"Hacker News","status":200,"markdown":"...","links":[...],"meta":{...},"stats":{...},"truncated":false,"next_offset":null}
{"id":"f","ok":true,"seq":6,"alive":true,"current_url":"https://news.ycombinator.com","title":"Hacker News","history_length":2,"history_index":0,"cookies_count":3,"uptime_secs":12}
{"id":"g","ok":true,"seq":7}
```

---

# Part 3: MCP Transport

For AI agents. Claude Desktop, Claude Code, LangChain, any MCP client.

## Server Startup

```bash
browser39 mcp                              # stdio transport (local)
browser39 mcp --transport sse --port 8039  # HTTP+SSE transport (remote)
```

## Session Model

One MCP connection = one browsing session. By default, session state (cookies, localStorage, history) is persisted to an encrypted file on disk (`~/.local/share/browser39/session.enc`), so sessions survive restarts. Use `--no-persist` to disable persistence and keep everything in memory only. SSE connections always use in-memory sessions (per-connection isolation).

## Tools

Each tool has a distinct parameter schema — no overloaded "one of three" modes. The LLM sees clear, separate tools.

### `browser39_fetch`

Fetch a URL and return the page as markdown.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | The URL to fetch |
| `method` | string | no | HTTP method (default: GET) |
| `body` | string | no | Request body (for POST/PUT/PATCH) |
| `auth_profile` | string | no | Auth profile name (credentials never exposed) |
| `headers` | object | no | Custom HTTP headers |
| `max_tokens` | integer | no | Truncate output to token budget |
| `selector` | string | no | CSS selector to extract specific content |
| `offset` | integer | no | Token offset for pagination |

Returns: markdown content, links, metadata, stats, truncation info. Secrets in response content are auto-redacted and replaced with opaque handles (`${browser39_secret_N}`).

### `browser39_click`

Follow a link from the current page.

| Parameter | Type | Required (one of) | Description |
|-----------|------|----------|-------------|
| `index` | integer | yes* | Link index from the links array |
| `text` | string | yes* | Link text to match |
| `max_tokens` | integer | no | Truncate output |

*One of `index` or `text` is required.

Returns: same shape as `browser39_fetch`.

### `browser39_links`

List links on the current page. Lightweight — no network request.

Returns: array of `{i, text, href}`.

### `browser39_dom_query`

Query the current page DOM.

| Parameter | Type | Required (one of) | Description |
|-----------|------|----------|-------------|
| `selector` | string | yes* | CSS selector |
| `script` | string | yes* | JS expression (boa_engine) |
| `attr` | string | no | Attribute to extract (default: `textContent`) |

*One of `selector` or `script` is required.

Returns: matching results array (selector mode) or single result value (script mode).

**boa_engine Web API shims:** Script mode supports `localStorage.getItem/setItem/removeItem/clear`, `document.cookie` (get/set), `element.value` setter, `form.submit()`, and `element.click()`. These interact with real session state (cookie jar, storage, filled fields). `form.submit()` and `element.click()` can trigger navigation — the result will include the new page if navigation occurred.

### `browser39_fill`

Fill form fields in the in-memory DOM.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `selector` | string | yes* | CSS selector for a single field |
| `value` | string | yes* | Value for the single field |
| `fields` | array | yes* | Array of `{selector, value}` objects |

*Either `selector`+`value` or `fields` is required.

### `browser39_submit`

Submit a form and return the response page.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `selector` | string | yes | CSS selector for the `<form>` element |
| `max_tokens` | integer | no | Truncate output |

Returns: same shape as `browser39_fetch`.

### `browser39_cookies`

List cookies from the session cookie jar.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `domain` | string | no | Filter by domain |

### `browser39_set_cookie`

Set a cookie in the session cookie jar.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `value` | string | yes | Cookie value |
| `domain` | string | yes | Cookie domain |
| `path` | string | no | Cookie path (default: `/`) |
| `secure` | boolean | no | Secure flag (default: false) |
| `http_only` | boolean | no | HttpOnly flag (default: false) |
| `max_age_secs` | integer | no | Max age in seconds |

### `browser39_delete_cookie`

Delete a cookie.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `domain` | string | yes | Cookie domain |

### `browser39_storage_get`

Get a localStorage value.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `key` | string | yes | Storage key |
| `origin` | string | no | Origin override (default: current page) |

### `browser39_storage_set`

Set a localStorage value.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `key` | string | yes | Storage key |
| `value` | string | yes | Storage value |
| `origin` | string | no | Origin override |

### `browser39_storage_delete`

Delete a localStorage key.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `key` | string | yes | Storage key |
| `origin` | string | no | Origin override |

### `browser39_storage_list`

List all localStorage entries for an origin.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | string | no | Origin override (default: current page) |

### `browser39_storage_clear`

Clear all localStorage for an origin.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | string | no | Origin override (default: current page) |

### `browser39_history`

Search or list browsing history.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | no | Text to search in URLs and titles (case-insensitive) |
| `limit` | integer | no | Maximum entries to return (default: 10) |

Returns entries with `index`, `url`, `title`, `status`, `current` flag, plus `count` and `total`.

### `browser39_back`

Navigate back in history. No parameters.

### `browser39_forward`

Navigate forward in history. No parameters.

### `browser39_info`

Get session state and confirm liveness. No parameters.

## Resources

Read-only views of current state, updated after each navigation.

| Resource URI | Description |
|-------------|-------------|
| `browser39://page` | Current page as markdown |
| `browser39://page/links` | Links array (JSON) |
| `browser39://page/meta` | Page metadata (JSON) |
| `browser39://cookies` | Cookies for current domain (JSON) |

## Claude Desktop Configuration

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

---

# Part 4: Future Actions

No protocol changes needed — just new action names with their own fields.

```json
{"action": "screenshot", "format": "png", "path": "/tmp/shot.png"}
{"action": "wait", "selector": ".loaded", "timeout_ms": 5000}
{"action": "scroll", "direction": "down", "amount": 500}
```
