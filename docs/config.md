# browser39 Configuration

All configuration lives in a single file: `~/.config/browser39/config.toml`

Override with `--config <path>` or `BROWSER39_CONFIG` env var.

---

## Full Example

```toml
# ~/.config/browser39/config.toml

# ─── Session Defaults ───────────────────────────────────────────────

[session]
# Page to load automatically on startup (before any agent commands)
start_url = "https://dashboard.example.com"

# Session persistence: "disk" (default) or "memory"
persistence = "disk"
# session_path = "/custom/path/session.enc"  # override default location

# HTTP client defaults
user_agent = "browser39/0.1"
timeout_secs = 30
max_redirects = 10

# Default fetch options (can be overridden per-request)
[session.defaults]
max_tokens = 8000
strip_nav = true
include_links = true
include_images = false

# ─── Auth Profiles ──────────────────────────────────────────────────
# Credentials stored outside the LLM conversation.
# Agent references by name: {"auth_profile": "github"}
# browser39 resolves and attaches the header. LLM never sees the value.

[auth.github]
header = "Authorization"
value = "Bearer ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
domains = ["api.github.com", "github.com"]

[auth.openai]
header = "Authorization"
value_env = "OPENAI_API_KEY"             # read from environment variable
value_prefix = "Bearer "                  # prepended to env value
domains = ["api.openai.com"]

[auth.internal]
header = "X-API-Key"
value_env = "INTERNAL_API_KEY"
domains = ["internal.company.com", "*.internal.company.com"]

[auth.ci]
header = "Authorization"
value_env = "CI_TOKEN"
value_prefix = "Bearer "
domains = ["ci.internal.com"]

# ─── Preloaded Cookies ──────────────────────────────────────────────
# Injected into the cookie jar before the first request.
# Use for pre-authenticated sessions — agent starts already logged in.

[[cookies]]
name = "session"
value_env = "SESSION_TOKEN"              # load from env (recommended)
domain = "app.example.com"
path = "/"
secure = true
http_only = true
sensitive = true                          # redacted in MCP responses

[[cookies]]
name = "csrf_token"
value_env = "CSRF_TOKEN"
domain = "app.example.com"
path = "/"
sensitive = true

[[cookies]]
name = "lang"
value = "en"                              # inline value (non-sensitive)
domain = "app.example.com"

# ─── Preloaded LocalStorage ─────────────────────────────────────────
# Injected into the in-memory storage before the first request.
# Useful for tokens, preferences, feature flags.

[[storage]]
origin = "https://app.example.com"
key = "api_token"
value_env = "APP_API_TOKEN"
sensitive = true                          # redacted in MCP responses

[[storage]]
origin = "https://app.example.com"
key = "theme"
value = "dark"

[[storage]]
origin = "https://app.example.com"
key = "feature_flags"
value = '{"beta_ui": true, "new_api": false}'

# ─── Default Headers ────────────────────────────────────────────────
# Sent with every request to matching domains.
# Merged with per-request headers (per-request wins on conflict).

[[headers]]
domains = ["api.example.com", "*.api.example.com"]
values = { "Accept" = "application/json", "X-Client" = "browser39" }

[[headers]]
domains = ["internal.company.com"]
values = { "X-Request-Source" = "agent" }

# ─── Security & Redaction ───────────────────────────────────────────

[security]
# Cookie values to always redact (matched by name, case-insensitive)
sensitive_cookies = ["session", "sid", "token", "jwt", "auth", "csrf", "csrf_token"]

# Headers to never include in results
sensitive_headers = ["authorization", "x-api-key", "cookie", "set-cookie"]

# Regex patterns — matched values are auto-redacted and replaced with handles
[security.patterns]
jwt = 'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}'
github_pat = 'ghp_[A-Za-z0-9]{36}'
github_fine = 'github_pat_[A-Za-z0-9_]{82}'
openai_key = 'sk-[A-Za-z0-9]{32,}'
anthropic_key = 'sk-ant-[A-Za-z0-9-]{32,}'
slack_bot = 'xoxb-[A-Za-z0-9-]+'
slack_user = 'xoxp-[A-Za-z0-9-]+'
stripe_key = 'sk_live_[A-Za-z0-9]{24,}'
aws_key = 'AKIA[A-Z0-9]{16}'

# Transport-specific redaction behavior
[security.mcp]
redact = true                             # always on for MCP (cannot be disabled)

[security.jsonl]
redact = false                            # off by default for JSONL agents
```

---

## Section Reference

### `[session]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `start_url` | string | none | URL to fetch automatically on startup |
| `user_agent` | string | `"browser39/0.1"` | User-Agent header for all requests |
| `timeout_secs` | integer | `30` | Default wall-clock timeout |
| `max_redirects` | integer | `10` | Maximum HTTP redirects to follow |
| `persistence` | string | `"disk"` | `"disk"` (encrypted file) or `"memory"` (no persistence) |
| `session_path` | string | auto | Override session file path (default: `~/.local/share/browser39/session.enc`) |

**`start_url` behavior:** browser39 fetches this URL before processing any commands. The page is loaded into session state — agent starts with a page already available. Useful for dashboards, authenticated portals, or any "home page" workflow. If the fetch fails (network error, auth required), browser39 logs a warning and continues normally.

### `[session.defaults]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max_tokens` | integer | unlimited | Default token budget for responses |
| `strip_nav` | boolean | `true` | Strip nav/header/footer from HTML |
| `include_links` | boolean | `true` | Include links in response |
| `include_images` | boolean | `false` | Include image references |

These are defaults — any per-request option overrides them.

### `[auth.<name>]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `header` | string | yes | HTTP header name |
| `value` | string | one of | Inline credential value |
| `value_env` | string | one of | Environment variable name |
| `value_prefix` | string | no | Prefix prepended to env value (e.g., `"Bearer "`) |
| `domains` | array | yes | Allowed domains (supports `*` wildcard) |

**Resolution order:** `value` takes precedence over `value_env`. If both are absent, startup error.

**Domain wildcards:** `*.example.com` matches `api.example.com`, `app.example.com`, etc. Exact matches are checked first.

**Security:** Domain enforcement prevents credential exfiltration. If a request URL doesn't match the profile's domains, browser39 returns `AUTH_PROFILE_DOMAIN_MISMATCH`.

### `[[cookies]]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `name` | string | yes | Cookie name |
| `value` | string | one of | Inline cookie value |
| `value_env` | string | one of | Environment variable name |
| `domain` | string | yes | Cookie domain |
| `path` | string | no | Cookie path (default: `/`) |
| `secure` | boolean | no | Secure flag (default: `false`) |
| `http_only` | boolean | no | HttpOnly flag (default: `false`) |
| `sensitive` | boolean | no | Redact value in MCP responses (default: `false`) |

Cookies are injected into the cookie jar at session start, before `start_url` is fetched. This means `start_url` requests already include these cookies — enabling pre-authenticated startup.

### `[[storage]]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `origin` | string | yes | Origin (e.g., `https://app.example.com`) |
| `key` | string | yes | Storage key |
| `value` | string | one of | Inline value |
| `value_env` | string | one of | Environment variable name |
| `sensitive` | boolean | no | Redact value in MCP responses (default: `false`) |

Storage is populated at session start. Values persist for the session lifetime.

### `[[headers]]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `domains` | array | yes | Domain patterns (supports `*` wildcard) |
| `values` | object | yes | Header name → value map |

Default headers are merged with per-request headers. Per-request headers win on conflict. Unlike auth profiles, default headers are **not** redacted — use auth profiles for credentials.

### `[security]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `sensitive_cookies` | array | see example | Cookie names to always redact |
| `sensitive_headers` | array | see example | Headers to exclude from results |

### `[security.patterns]`

Key-value pairs where key is a label and value is a regex pattern. Matched content in responses is auto-redacted and replaced with secret handles.

### `[security.mcp]` / `[security.jsonl]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `redact` | boolean | `true` (MCP) / `false` (JSONL) | Enable redaction for this transport |

MCP redaction is always on and cannot be disabled (safety by design).

---

## Loading Order

1. Load `~/.config/browser39/config.toml` (or `--config` / `BROWSER39_CONFIG`)
2. Resolve all `value_env` references from environment
3. Load persisted session from disk (if `persistence = "disk"` and session file exists)
4. Inject preloaded cookies into cookie jar (config entries take precedence over restored)
5. Inject preloaded storage into LocalStorage (config entries take precedence over restored)
6. Register default headers
7. If `start_url` is set, fetch it (cookies and headers already active)
8. Begin accepting commands (JSONL) or connections (MCP)

Session state is saved to disk after each mutation (fetch, cookie/storage changes). Use `--no-persist` CLI flag to force in-memory mode. One-shot `fetch` and `batch` commands always use in-memory sessions.

---

## Minimal Configs

**Just a start page:**
```toml
[session]
start_url = "https://news.ycombinator.com"
```

**Pre-authenticated API access:**
```toml
[auth.api]
header = "Authorization"
value_env = "API_TOKEN"
value_prefix = "Bearer "
domains = ["api.example.com"]
```

**Pre-authenticated browser session:**
```toml
[session]
start_url = "https://app.example.com/dashboard"

[[cookies]]
name = "session"
value_env = "SESSION_COOKIE"
domain = "app.example.com"
secure = true
http_only = true
sensitive = true
```

**Full agent setup (start page + auth + storage + security):**
```toml
[session]
start_url = "https://app.example.com"

[auth.app]
header = "Authorization"
value_env = "APP_JWT"
value_prefix = "Bearer "
domains = ["app.example.com", "api.example.com"]

[[cookies]]
name = "session"
value_env = "SESSION_ID"
domain = "app.example.com"
sensitive = true

[[storage]]
origin = "https://app.example.com"
key = "user_prefs"
value = '{"lang": "en", "timezone": "UTC"}'

[security]
sensitive_cookies = ["session", "token", "jwt"]

[security.mcp]
redact = true
```
