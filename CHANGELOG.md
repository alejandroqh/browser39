# Changelog

## [1.5.0] - 2026-04-04

### Added

- **Full JavaScript DOM API** — massively expanded the JS sandbox beyond basic `querySelector`:
  - **DOM traversal**: `parentElement`, `parentNode`, `children`, `childNodes`, `childElementCount`, `firstChild`, `lastChild`, `firstElementChild`, `lastElementChild`, `nextSibling`, `previousSibling`, `nextElementSibling`, `previousElementSibling`
  - **DOM lookup**: `getElementById`, `getElementsByClassName`, `getElementsByTagName`, `getElementsByName`, `document.forms`, `document.links`
  - **DOM mutation**: `createElement`, `createTextNode`, `appendChild`, `removeChild`, `insertBefore`, `element.remove()`, `setAttribute`, `removeAttribute`, `textContent` setter, `innerHTML` setter
  - **Element properties**: `matches(selector)`, `closest(selector)`, `contains(node)`, `hasAttribute`, `classList` (contains/length/item), `dataset` (data-* to camelCase), `nodeType`, `nodeName`, `disabled`, `checked`, `hidden`, `type`, `name`, `src`, `alt`, `placeholder`
  - **Event system**: `addEventListener`, `removeEventListener`, `dispatchEvent` with listener storage; `new Event`, `new CustomEvent`, `new MouseEvent`, `new KeyboardEvent`, `new InputEvent`, `new FocusEvent` constructors
  - **Console**: `console.log/warn/error/info/debug` — output captured and returned in `console_output` field
  - **Timers**: `setTimeout` (synchronous execution), `setInterval`/`clearInterval` (no-op), `requestAnimationFrame` (synchronous)
  - **Utilities**: `atob`/`btoa` (base64), `getComputedStyle` (stub), `MutationObserver` (no-op stub)
- **`--version` CLI flag** — `browser39 --version` prints the version
- **Config version tracking** — config.toml now includes a `version` field; auto-updated on load when the binary version changes

### Changed

- JS sandbox internals refactored from single file (`dom_script.rs`) into `dom_script/` submodule (mod, element, document, window, events, convert) for maintainability
- DOM backing migrated from `Rc<Html>` to `Rc<RefCell<Html>>` to support mutation
- Mutated DOM is serialized back and stored in session state after script execution
- Default user-agent now matches binary version (`browser39/1.5.0` instead of `browser39/0.1`)
- MCP tool descriptions expanded to document JS capabilities for LLMs
- `DomQueryParams.script` field doc now lists every available DOM API
- Plugin manifests updated to 1.5.0

## [1.1.0] - 2026-04-04

### Added

- **10 MCP config management tools** — agents can now manage browser39's configuration directly via MCP:
  - `browser39_config_show` — view config with sensitive values masked (never exposes raw file)
  - `browser39_config_set` — set scalar settings (search engine, timeouts, session defaults)
  - `browser39_config_auth_set` / `browser39_config_auth_delete` — manage auth profiles (credentials stored securely, never returned via MCP)
  - `browser39_config_cookie_set` / `browser39_config_cookie_delete` — manage preloaded cookies
  - `browser39_config_storage_set` / `browser39_config_storage_delete` — manage preloaded storage entries
  - `browser39_config_header_set` / `browser39_config_header_delete` — manage default header rules
- Config changes are saved to disk atomically and take effect immediately (live reload)
- Security config changes (redaction patterns, sensitive cookie names) are applied to the running redaction engine without restart

### Changed

- Release binaries now use versionless names (`browser39-macos-arm64` instead of `browser39-v1.0.0-macos-arm64`) for stable download URLs
- Install prompts simplified — direct binary download from GitHub releases, no Rust toolchain required
- Config structs now implement `Serialize` for TOML round-tripping
- `Config::resolve()` is now public for use after config mutations

### Fixed

- `build.sh` restored (accidentally deleted in v1.0.0)

## [1.0.0] - 2026-03-28

Initial release.

- Headless web browser for AI agents with HTML-to-Markdown conversion
- 19 MCP tools: fetch, click, links, DOM query, forms, cookies, storage, search, history, navigation
- 4 MCP resources: page markdown, links, metadata, cookies
- JavaScript execution via boa_engine
- Session persistence with AES-256-GCM encryption
- Auth profiles with domain enforcement and secret redaction
- Content preselection with section-level token estimates
- MCP (stdio + HTTP), JSONL (watch + batch), and CLI transports
- Pre-authenticated startup via config (cookies, storage, headers)
