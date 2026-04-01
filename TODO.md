# browser39 Implementation TODO

Step-by-step implementation plan. Complete each step before moving to the next.

## Phase 1: Foundation

- [x] **Step 1 — Project setup**
  - Create single-binary `Cargo.toml` with module directories: `src/core/`, `src/service/`, `src/cli/`, `src/mcp/`
  - Set up dependencies and Rust edition
  - Create `tests/` directory (gitignored, private integration tests)
  - Verify `cargo build` compiles

- [x] **Step 2 — Core types and protocol**
  - Define `PageResult`, `Link`, `PageMetadata`, `FetchOptions` in `src/core/page.rs`
  - Include pagination fields: `truncated`, `next_offset` in `PageResult`
  - Include `headers` map in fetch request types
  - Define `Command` and `Result` serde types in `src/cli/protocol.rs` (matching JSONL schema)
  - Include `seq` field in JSONL envelope (monotonically increasing sequence number)
  - Implement fetch mode precedence: `url` > `index` > `text`
  - Write unit tests for serialization/deserialization round-trips

- [x] **Step 3 — Configuration**
  - Implement `src/core/config.rs`: load and parse `~/.config/browser39/config.toml`
  - Support `--config <path>` CLI flag and `BROWSER39_CONFIG` env var override
  - Parse all sections: `[session]`, `[auth.*]`, `[[cookies]]`, `[[storage]]`, `[[headers]]`, `[security]`
  - Resolve `value_env` references from environment variables at load time
  - Support `value_prefix` for auth profiles (e.g., `"Bearer "` prepended to env value)
  - Domain wildcard matching for auth profiles and default headers (`*.example.com`)
  - Graceful defaults: missing config file → all defaults, missing sections → skip
  - Test: load config with all sections → verify parsed values
  - Test: `value_env` resolution → verify env var read
  - Test: missing config file → no error, all defaults

- [x] **Step 4 — HTTP client**
  - Wrap `reqwest::Client` in `src/core/http_client.rs`
  - Cookie jar support via `cookie_store`
  - Apply config: user-agent, timeouts, redirect policy from `[session]`
  - Inject preloaded cookies from `[[cookies]]` config into jar at startup
  - Apply default headers from `[[headers]]` config (domain-matched, merged with per-request)
  - Support custom `headers` field from fetch command
  - Test: fetch `https://example.com` and get HTML back
  - Test: preloaded cookie sent with matching domain request

## Phase 2: HTML → Markdown

- [x] **Step 5 — HTML parser**
  - Parse HTML with `scraper` in `src/core/html_to_md.rs`
  - Extract title, metadata, links from parsed DOM

- [x] **Step 6 — Markdown converter**
  - Convert HTML elements to markdown: headings, paragraphs, lists, links, code blocks, tables, emphasis
  - Token optimization: strip nav/header/footer, collapse whitespace
  - CSS selector filtering (`options.selector`)
  - Token budget truncation (`options.max_tokens`)
  - Test: convert known HTML pages and verify markdown output

## Phase 3: Service Layer

- [x] **Step 7 — BrowserService**
  - Implement `BrowserService` in `src/service/service.rs`
  - Constructor takes `Config` — applies session defaults, injects preloaded storage from `[[storage]]`
  - `fetch(url, headers, options) -> PageResult` with pagination (`offset`, `max_tokens` → `truncated`, `next_offset`)
  - `fetch(index/text, options) -> PageResult` (follow link from current page, options/headers apply)
  - Auth profile resolution: if `auth_profile` set, resolve from config, verify domain, attach header
  - `links() -> Vec<Link>` (lightweight, reads from session cache)
  - Session state: current page, cookies, link cache, storage
  - If `start_url` configured, auto-fetch on construction (cookies/headers already active)
  - Test: service-level fetch returns `PageResult`
  - Test: `start_url` → service starts with page already loaded

- [x] **Step 8 — Session history**
  - Add history stack to session (back/forward navigation)
  - `back()` / `forward()` — navigate history
  - `info()` — return session state + liveness (`alive`, `uptime_secs`)
  - Test: fetch → links → fetch(index) → back → forward cycle

## Phase 4: CLI (JSONL Transport)

- [x] **Step 9 — `browser39 fetch` (one-shot)**
  - `clap` CLI with `fetch <url>` subcommand
  - `--config <path>` flag (loads config, applies defaults/auth/cookies)
  - `--output text|json` flag
  - Text mode: print markdown to stdout
  - JSON mode: print full `PageResult` as JSON
  - Test: `browser39 fetch https://example.com` outputs markdown
  - Test: `browser39 fetch --config test.toml` uses auth profile from config

- [x] **Step 10 — `browser39 batch`**
  - Read `commands.jsonl` line by line
  - Validate `seq` field (monotonically increasing), echo in results
  - Dispatch each command to `BrowserService`
  - Atomic writes: single `write()` + `fsync()` per result line
  - File rotation: rotate `results.jsonl` at 10MB
  - Write results to `results.jsonl`
  - Handle all actions: fetch, links, dom_query, back, forward, info
  - Test: write a commands file, run batch, verify results file

- [ ] **Step 11 — `browser39 watch`**
  - Use `notify` crate to watch commands file for new lines
  - Track file position (only process new lines)
  - Atomic writes for results
  - Handle `quit` action to exit
  - Test: start watch, append commands from another process, verify results

## Phase 5: DOM Query

- [x] **Step 12 — DOM query: CSS selector mode**
  - Implement CSS selector mode in `src/core/dom_query.rs`
  - Uses `scraper` directly — no JS engine needed
  - Input: `selector` + `attr` (textContent, innerHTML, href, src, or any attribute)
  - Returns: array of matching values
  - Wire as `dom_query` action in `BrowserService`
  - Test: fetch page, query `h1` textContent, query `a` href

- [x] **Step 13 — DOM query: script mode**
  - Integrate `boa_engine` for advanced script mode
  - Create JS context with `document` global
  - Sandbox: no network, no filesystem from JS
  - Expose `document.title`, `document.querySelector()`, `document.querySelectorAll()`
  - Expose element properties: `textContent`, `innerHTML`, `getAttribute()`, `href`
  - Bridge parsed HTML (from `scraper`) into the JS `document` object
  - Handle errors → `DOM_QUERY_ERROR` code
  - Test: parse HTML, run `document.querySelectorAll('a').length`, verify result

## Phase 5.5: Forms, Cookies & LocalStorage

- [x] **Step 14 — Form support: fill + submit**
  - Implement `src/core/form.rs`: parse `<form>` elements, collect named fields, build HTTP request
  - Support `enctype`: `application/x-www-form-urlencoded` (default) and `multipart/form-data`
  - Extend session to hold filled field overlays (`HashMap<Selector, String>`)
  - `fill()` in BrowserService: validate selectors against `<input>`/`<textarea>`/`<select>`, store values
  - `submit()` in BrowserService: find `<form>`, merge filled values with DOM defaults, build + send HTTP request, return PageResult
  - Extend `fetch` to support `method` (GET/POST/PUT/PATCH/DELETE) and `body` (string) fields
  - Wire `fill` and `submit` as actions in batch/watch dispatch
  - Test: fetch page with form → fill fields → submit → verify correct POST body and response

- [x] **Step 15 — Cookie management**
  - `cookies()` in BrowserService: read from reqwest cookie jar, serialize to JSON, filter by domain
  - `set_cookie()`: build `cookie::Cookie`, insert into jar
  - `delete_cookie()`: remove from jar by name+domain
  - Wire `cookies`, `set_cookie`, `delete_cookie` as actions in batch/watch dispatch
  - Test: fetch page → `cookies` shows server-set cookies → `set_cookie` adds one → re-fetch → verify cookie sent in request

- [x] **Step 16 — LocalStorage**
  - Implement `src/core/storage.rs`: `LocalStorage` struct wrapping `HashMap<Origin, HashMap<String, String>>`
  - Origin = `scheme://host:port` derived from URL
  - Wire into session state
  - `storage_get/set/delete/list/clear` methods in BrowserService
  - Wire all `storage_*` as actions in batch/watch dispatch
  - Test: `storage_set` → navigate to different page → `storage_get` on original origin → verify persistence → `storage_clear` → verify empty

- [x] **Step 17 — boa_engine Web API shims**
  - Register `localStorage` global object in JS context:
    - `getItem(key)` → reads from session's LocalStorage
    - `setItem(key, value)` → writes to session's LocalStorage
    - `removeItem(key)` → deletes from session's LocalStorage
    - `clear()` → clears session's LocalStorage for current origin
  - Register `document.cookie` property:
    - getter → returns cookie string from reqwest cookie jar for current origin
    - setter → parses Set-Cookie string, inserts into jar
  - Register `element.value` setter on input/textarea/select nodes → writes to session filled fields
  - Register `HTMLFormElement.submit()` → triggers form submission pipeline, returns navigation result
  - Register `HTMLElement.click()` → link: navigate; submit button: submit form; other: no-op
  - Test: `localStorage.setItem('k','v'); localStorage.getItem('k')` → returns `'v'`
  - Test: `document.querySelector('#field').value = 'test'` → field is filled in session
  - Test: `document.cookie` → returns cookies string for current domain

## Phase 6: MCP Transport

- [x] **Step 18 — MCP server (stdio)**
  - Integrate `rmcp` crate in `src/mcp/`
  - `browser39 mcp` subcommand starts MCP server over stdio
  - Define MCP tools (distinct parameter schemas, clear for LLMs):
    - `browser39_fetch` → fetch by URL (params: url, method?, body?, headers?, max_tokens?, selector?, offset?)
    - `browser39_click` → follow link (params: index or text, max_tokens?)
    - `browser39_links` → list links (no params)
    - `browser39_dom_query` → query DOM (params: selector or script, attr?)
    - `browser39_fill` → fill form fields (params: selector+value or fields[])
    - `browser39_submit` → submit form (params: selector, max_tokens?)
    - `browser39_cookies` → list cookies (params: domain?)
    - `browser39_set_cookie` → set cookie (params: name, value, domain, path?, secure?, http_only?, max_age_secs?)
    - `browser39_delete_cookie` → delete cookie (params: name, domain)
    - `browser39_storage_get` → get localStorage value (params: key, origin?)
    - `browser39_storage_set` → set localStorage value (params: key, value, origin?)
    - `browser39_storage_delete` → delete localStorage key (params: key, origin?)
    - `browser39_storage_list` → list localStorage entries (params: origin?)
    - `browser39_storage_clear` → clear localStorage (params: origin?)
    - `browser39_back` → navigate back
    - `browser39_forward` → navigate forward
    - `browser39_info` → session state + liveness
  - Session-per-connection (no session IDs)
  - Test: configure in Claude Desktop, ask Claude to fetch a page

- [x] **Step 19 — MCP resources**
  - Expose MCP resources:
    - `browser39://page` → current page markdown
    - `browser39://page/links` → links JSON
    - `browser39://page/meta` → metadata JSON
    - `browser39://cookies` → cookies for current domain JSON
  - Test: read resources via MCP client

- [x] **Step 20 — MCP HTTP+SSE transport**
  - Add `browser39 mcp --transport sse --port 8039` mode
  - Remote agents connect via HTTP+SSE
  - Test: connect from remote MCP client

## Phase 6.5: Security

- [x] **Step 21 — Auth profiles**
  - Implement `src/core/auth.rs`: resolve auth profiles from loaded `Config`
  - Support `value` (inline), `value_env` (env var), and `value_prefix` (prepended to env value)
  - Domain enforcement with wildcard support (`*.example.com`)
  - Wire into BrowserService fetch pipeline: resolve profile → verify domain → attach header
  - Error codes: `AUTH_PROFILE_NOT_FOUND`, `AUTH_PROFILE_DOMAIN_MISMATCH`
  - Test: configure profile → fetch with `auth_profile` → verify header attached → fetch wrong domain → verify error

- [x] **Step 22 — Secret store and handles**
  - Implement `src/core/secrets.rs`: `SecretStore` with monotonic handle counter
  - `store(value) -> handle` — stores value, returns `${browser39_secret_N}`
  - `resolve(text) -> text` — replaces all `${browser39_secret_N}` patterns with real values
  - `redact(text) -> text` — scans for secret patterns (JWT, API keys) and replaces with handles
  - Wire into BrowserService: redact outgoing results, resolve incoming commands
  - `sensitive` flag on `fill`, `set_cookie`, `storage_set` → store value as secret handle
  - Test: store secret → resolve handle → get original value
  - Test: redact JWT in page content → returns handle → resolve handle in next request

- [x] **Step 23 — Redaction engine**
  - Implement `src/core/redaction.rs`: configurable pattern-based redaction
  - Load patterns from `[security.patterns]` in config
  - Built-in patterns: JWT, GitHub PAT, OpenAI keys, Slack tokens
  - Sensitive cookie names: `session`, `sid`, `token`, `jwt`, `auth`, `csrf`
  - Transport-specific behavior: MCP always redacts, JSONL configurable (default: off)
  - Apply redaction to: `markdown` in PageResult, `cookies` values, `storage_get`/`storage_list` values
  - Cookie results include `handle` field when redacted: `{"value": "••••••", "handle": "${browser39_secret_3}"}`
  - Test: MCP response with JWT in markdown → redacted with handle
  - Test: `cookies` via MCP → values redacted; via JSONL → values visible
  - Test: `fill` with `sensitive: true` → value never echoed

## Phase 7: Polish

- [x] **Step 24 — Search new name**
  - Rename all the references to browser39
  - New name showuld be easy to use and understand by llm
  - First option: WebBrowser39
  - Be sure that no exist other software

- [x] **Step 25 — Error handling**
  - Consistent error codes across all actions (see protocol spec)
  - Graceful handling of malformed commands
  - Retry guidance: TIMEOUT/HTTP_ERROR retryable, INVALID_COMMAND not
  - Timeouts on all operations (wall-clock: DNS + connect + transfer + processing)

- [ ] **Step 26 — Integration tests**
  - JSONL: Python script that browses via batch mode
  - JSONL: Python script that browses via watch mode
  - MCP: integration test via Claude Desktop / Claude Code
  - Full session: fetch → links → fetch(link) → dom_query(selector) → dom_query(script) → back → info → quit
  - Forms: fetch form page → fill → submit → verify POST body and response page
  - Manual POST: fetch with method=POST, body, headers → verify request
  - Cookies: fetch → cookies → set_cookie → re-fetch → verify cookie in request → delete_cookie
  - LocalStorage: storage_set → storage_get → navigate → storage_list → storage_clear
  - JS shims: dom_query script with localStorage.setItem → storage_get verifies it; document.cookie reads from jar
  - Security: auth_profile → credential attached without LLM seeing it; domain mismatch → error
  - Security: JWT in response → auto-redacted → handle resolves in next request
  - Security: fill with sensitive:true → password never in any result
  - Security: cookies via MCP → all values redacted; JSONL → values visible
  - Config: start_url → session starts with page loaded
  - Config: preloaded cookies → sent on first request to matching domain
  - Config: preloaded storage → available via storage_get before any storage_set
  - Config: default headers → merged with per-request headers on matching domains
  - Config: auth_profile from config → credential never in LLM context
  - Pagination: fetch with max_tokens, verify truncated/next_offset, fetch with offset

## Phase 8: Integration

- [x] **Step 27 — Integrate with OpenClaw**
  - Claude Bundle plugin (`.claude-plugin/`) — OpenClaw auto-detects `.mcp.json` and exposes browser39 MCP tools via stdio subprocess
  - Native OpenClaw plugin (`openclaw-plugin/`) — TypeScript entry with `definePluginEntry` + `api.registerTool()` for all 19 tools, proxied via MCP JSON-RPC to `browser39 mcp`
  - Native plugin supports `binaryPath` and `configPath` config, auto-spawns/reconnects the browser39 process
  - Install: `openclaw plugins install ./` (bundle) or `openclaw plugins install ./openclaw-plugin` (native)

## Questions to review at end

- [ ] Add `browser39 query <url> --selector <sel> --attr <attr>` one-shot CLI command for quick DOM queries without needing batch mode
- [ ] Handle non-text content types (image/png, application/pdf, etc.) — detect via Content-Type header and return `[Binary content: <mime>, <size> bytes]` instead of dumping raw bytes as markdown (wastes ~19K tokens on a single PNG)
- [ ] Add websearch engine a select by default and the option webserch like a comand that triggers like duckduck go
