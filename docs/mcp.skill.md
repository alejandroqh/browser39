# browser39 — MCP Web Browser for AI Agents

You have access to browser39, a headless web browser that fetches pages and returns token-optimized Markdown. Use it to browse the web, fill forms, manage cookies and localStorage, and query the DOM.

## MCP Tools

### Navigation

| Tool | Description |
|------|-------------|
| `browser39_fetch` | Fetch a URL and return the page as markdown |
| `browser39_click` | Follow a link by index number or link text |
| `browser39_links` | List all links on the current page |
| `browser39_history` | Search or list browsing history |
| `browser39_back` | Navigate back in history |
| `browser39_forward` | Navigate forward in history |
| `browser39_info` | Get session info (URL, history, cookies count, uptime) |

### DOM

| Tool | Description |
|------|-------------|
| `browser39_dom_query` | Query the DOM with a CSS selector or JavaScript |

### Forms

| Tool | Description |
|------|-------------|
| `browser39_fill` | Fill form field(s) by CSS selector |
| `browser39_submit` | Submit a form by CSS selector |

### Cookies

| Tool | Description |
|------|-------------|
| `browser39_cookies` | List cookies (optionally filtered by domain) |
| `browser39_set_cookie` | Set a cookie |
| `browser39_delete_cookie` | Delete a cookie by name and domain |

### LocalStorage

| Tool | Description |
|------|-------------|
| `browser39_storage_get` | Get a localStorage value |
| `browser39_storage_set` | Set a localStorage value |
| `browser39_storage_delete` | Delete a localStorage key |
| `browser39_storage_list` | List localStorage entries for an origin |
| `browser39_storage_clear` | Clear localStorage for an origin |

### Configuration

Manage the browser39 config file (`~/.config/browser39/config.toml`) via MCP. Sensitive values (credentials, tokens) are stored securely and **never returned** — only masked with `••••••` in `config_show`.

| Tool | Description |
|------|-------------|
| `browser39_config_show` | View config (all or by section) with sensitive values masked |
| `browser39_config_set` | Set a scalar config value (search engine, timeouts, defaults) |
| `browser39_config_auth_set` | Add/update an auth profile (credentials never returned) |
| `browser39_config_auth_delete` | Delete an auth profile |
| `browser39_config_cookie_set` | Add/update a preloaded cookie |
| `browser39_config_cookie_delete` | Delete a preloaded cookie |
| `browser39_config_storage_set` | Add/update a preloaded storage entry |
| `browser39_config_storage_delete` | Delete a preloaded storage entry |
| `browser39_config_header_set` | Add/update default header rules for domains |
| `browser39_config_header_delete` | Delete default header rules |

## MCP Resources

| URI | Description | MIME |
|-----|-------------|------|
| `browser39://page` | Current page markdown | text/markdown |
| `browser39://page/links` | Links on current page | application/json |
| `browser39://page/meta` | Page metadata (lang, description) | application/json |
| `browser39://cookies` | Cookies for current domain | application/json |

## Tool Details

### browser39_fetch

Fetch a URL and return the page as token-optimized markdown.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | URL to fetch |
| `method` | string | no | HTTP method (GET, POST, PUT, PATCH, DELETE). Default: GET |
| `body` | string | no | Request body (for POST/PUT/PATCH) |
| `headers` | object | no | Additional HTTP headers |
| `auth_profile` | string | no | Auth profile name from config |
| `max_tokens` | integer | no | Limit output size; enables pagination |
| `selector` | string | no | CSS selector to extract specific content. When the selector matches a heading element (h1–h6), browser39 auto-expands to include all content until the next same-level heading — so `selector: "#Astronauts"` returns the full section, not just the heading text. |
| `offset` | integer | no | Resume from `next_offset` of a truncated response |
| `show_selectors_first` | boolean | no | Default: `true`. Returns available content selectors and section headings instead of full page content. Re-fetch with a chosen `selector` for targeted content, or set to `false` to get the raw page. |

The response includes the page markdown, URL, title, status code, link count, estimated token count, and truncation info.

When `show_selectors_first` is `true` (default), the first fetch returns a compact list of content selectors (e.g. `main`, `article`, `#mw-content-text`) and section headings (e.g. `#Astronauts`, `#History`) with estimated token counts. The LLM picks the relevant selector and re-fetches with it.

### browser39_click

Follow a link on the current page. Provide either `index` (from `browser39_links` output) or `text` (substring match).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `index` | integer | one of | Link index number |
| `text` | string | one of | Link text to match |
| `max_tokens` | integer | no | Limit output size |

### browser39_dom_query

Query the current page DOM using CSS selectors or JavaScript.

**CSS selector mode:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `selector` | string | yes | CSS selector |
| `attr` | string | no | Attribute to extract (default: `textContent`). Options: `textContent`, `innerHTML`, `href`, `src`, or any HTML attribute |

**JavaScript mode:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `script` | string | yes | JavaScript to execute. Has access to `document.querySelector()`, `document.querySelectorAll()`, `document.title`, `localStorage`, and `document.cookie` |

### browser39_fill

Fill form fields by CSS selector. Values persist until form submission or page navigation.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `selector` | string | one of | CSS selector for a single field |
| `value` | string | one of | Value for the single field |
| `fields` | array | one of | Array of `{selector, value}` objects for multiple fields |

### browser39_submit

Submit a form. Merges filled fields with form defaults and sends the HTTP request.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `selector` | string | yes | CSS selector for the `<form>` element |
| `max_tokens` | integer | no | Limit response page size |

### browser39_set_cookie

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `value` | string | yes | Cookie value |
| `domain` | string | yes | Cookie domain |
| `path` | string | no | Cookie path (default: `/`) |
| `secure` | boolean | no | Require HTTPS (default: false) |
| `http_only` | boolean | no | HTTP-only flag (default: false) |
| `max_age_secs` | integer | no | Expiration in seconds |

### browser39_delete_cookie

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `domain` | string | yes | Cookie domain |

### browser39_storage_get / browser39_storage_set / browser39_storage_delete

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `key` | string | yes | Storage key |
| `value` | string | yes (set only) | Value to store |
| `origin` | string | no | Origin (default: current page origin) |

### browser39_history

Search or list browsing history. Most recent pages first.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | no | Search URLs and titles (case-insensitive) |
| `limit` | integer | no | Max entries to return (default: 10) |

Returns `{entries: [{index, url, title, status, current}], count, total}`.

### browser39_storage_list / browser39_storage_clear

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | string | no | Origin (default: current page origin) |

### browser39_config_show

View the current configuration with sensitive values masked. The raw config file is never exposed.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `section` | string | no | Filter by section: `session`, `search`, `auth`, `cookies`, `storage`, `headers`, `security`. Omit to show all. |

### browser39_config_set

Set a scalar config value. Changes are saved to disk and take effect immediately (except `user_agent` and `max_redirects`, which require restart).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `key` | string | yes | Setting key in dot notation (see below) |
| `value` | string | yes | New value (parsed to appropriate type) |

**Allowed keys:** `session.start_url`, `session.user_agent`, `session.timeout_secs`, `session.max_redirects`, `session.persistence`, `session.defaults.max_tokens`, `session.defaults.strip_nav`, `session.defaults.include_links`, `session.defaults.include_images`, `search.engine`

Use `"null"` or `""` to clear optional fields (`start_url`, `max_tokens`).

### browser39_config_auth_set

Add or update an auth profile. Credential values are stored on disk but **never returned via MCP**.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Profile name (e.g., `"github"`) |
| `header` | string | yes | HTTP header name (e.g., `"Authorization"`) |
| `value` | string | one of | Credential value (stored securely) |
| `value_env` | string | one of | Environment variable containing the credential |
| `value_prefix` | string | no | Prefix prepended to the value (e.g., `"Bearer "`) |
| `domains` | array | yes | Domains this profile applies to |

### browser39_config_auth_delete

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Auth profile name to delete |

### browser39_config_cookie_set

Add or update a preloaded cookie in the config. Cookies marked `sensitive` are masked in `config_show`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `value` | string | one of | Cookie value |
| `value_env` | string | one of | Environment variable containing the value |
| `domain` | string | yes | Cookie domain |
| `path` | string | no | Cookie path (default: `/`) |
| `secure` | boolean | no | Require HTTPS (default: false) |
| `http_only` | boolean | no | HTTP-only flag (default: false) |
| `sensitive` | boolean | no | Mask value in `config_show` (default: false) |

### browser39_config_cookie_delete

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `domain` | string | yes | Cookie domain |

### browser39_config_storage_set

Add or update a preloaded storage entry. Entries marked `sensitive` are masked in `config_show`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | string | yes | Origin (e.g., `https://app.example.com`) |
| `key` | string | yes | Storage key |
| `value` | string | one of | Storage value |
| `value_env` | string | one of | Environment variable containing the value |
| `sensitive` | boolean | no | Mask value in `config_show` (default: false) |

### browser39_config_storage_delete

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | string | yes | Origin |
| `key` | string | yes | Storage key |

### browser39_config_header_set

Add or update default header rules for matching domains.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `domains` | array | yes | Domain patterns (supports `*` wildcard) |
| `values` | object | yes | Header key-value pairs |

### browser39_config_header_delete

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `domains` | array | yes | Domain list of the rule to delete (must match exactly) |

## CLI

```bash
# MCP server over stdio
browser39 mcp

# MCP server over Streamable HTTP
browser39 mcp --transport sse --port 8039

# One-shot fetch (markdown)
browser39 fetch <url>

# One-shot fetch (JSON with compact link refs)
browser39 fetch --output json <url>

# Custom config
browser39 --config <path> fetch <url>

# Batch mode
browser39 batch commands.jsonl --output results.jsonl

# Watch mode
browser39 watch commands.jsonl --output results.jsonl
```

## Token Optimization

browser39 minimizes token usage automatically:

- **HTML to Markdown** — strips scripts, styles, and non-content elements
- **Link deduplication** — same-URL links (image + headline cards) emitted once
- **Heading-in-link cleanup** — heading markers inside `<a>` tags suppressed
- **Compact link refs** (JSON) — `[text][N]` instead of `[text](long/url)`
- **Same-origin shortening** — links on the same domain show path-only

## Guidelines

- The first fetch of any page returns content selectors and section headings by default. Pick the most relevant selector and re-fetch with it to get focused content.
- To fetch a specific section of an article, use its heading ID as the selector (e.g. `selector: "#Astronauts"`). The heading auto-expands to include all content until the next same-level heading.
- For large content regions, combine `selector` with `max_tokens` and paginate with `offset` when the response shows truncation.
- Set `show_selectors_first=false` to skip selector discovery and get the raw page directly.
- Use `browser39_links` to discover available links, then `browser39_click` with an `index` to navigate.
- Use `browser39_dom_query` with a CSS selector for structured data extraction, or with JavaScript for complex logic.
- Fill forms with `browser39_fill` then submit with `browser39_submit`.
