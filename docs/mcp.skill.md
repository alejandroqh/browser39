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
