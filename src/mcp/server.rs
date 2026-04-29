use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use tokio::sync::mpsc;

use super::commands::{McpCommand, send_cmd};
use super::params::*;

pub struct McpServer {
    tool_router: ToolRouter<Self>,
    cmd_tx: mpsc::Sender<McpCommand>,
}

impl McpServer {
    pub fn new(cmd_tx: mpsc::Sender<McpCommand>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            cmd_tx,
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        description = r#"Fetch a URL and return the page as token-optimized markdown. Supports GET/POST/PUT/PATCH/DELETE, auth profiles, custom headers, CSS selector extraction, pagination, and binary downloads.

USE WHEN: loading a new page by URL, calling an HTTP API, or downloading a binary asset. NOT WHEN: following a link already listed by browser39_links (use browser39_click for that — it preserves Referer and is cheaper).

Effect: read for GET, external-mutate for POST/PUT/PATCH/DELETE.

Selector parameter accepts CSS only. See docs/selectors.md.

Common failures:
- INVALID_URL → URL didn't parse; ensure scheme (https://) is present.
- TIMEOUT → server slow or unreachable; retryable, or raise timeout in config.
- HTTP_ERROR → 4xx/5xx response. Inspect status; for 401/403 attach an auth_profile.
- AUTH_PROFILE_DOMAIN_MISMATCH → the requested URL is outside the profile's allowed domains; pick a different profile or extend `domains` in config.
- SELECTOR_NOT_FOUND → CSS selector matched zero elements; re-fetch without selector to see the page, or pick from content_selectors in the response.
- Page came back giant → set `max_tokens` and use `show_selectors_first: true` (default) to preview section sizes before committing tokens."#
    )]
    async fn browser39_fetch(
        &self,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Fetch { params, tx }).await
    }

    #[tool(
        description = r#"Follow a link on the current page by index number or visible link text.

USE WHEN: navigating to a link returned by browser39_fetch or browser39_links. NOT WHEN: clicking a button that is not an `<a href>` (use browser39_dom_query with a script that calls `.click()`), or submitting a form (use browser39_submit).

Effect: read (GET request).

Common failures:
- NO_PAGE → no current page; call browser39_fetch first.
- LINK_NOT_FOUND → index out of range, or text didn't match any link substring (case-insensitive). Call browser39_links to see what is actually on the page; the link list is authoritative.
- HTTP_ERROR / TIMEOUT → target URL failed; retryable."#
    )]
    async fn browser39_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Click { params, tx }).await
    }

    #[tool(
        description = r#"List every link on the current page with its index, visible text, and href. Cheap — reads from session cache, no network request.

USE WHEN: deciding which link to follow; resolving ambiguous click(text=...) calls; building a navigation plan. NOT WHEN: you already know the URL — use browser39_fetch directly.

Effect: read.

Common failures:
- NO_PAGE → no current page; call browser39_fetch first."#
    )]
    async fn browser39_links(&self) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Links { tx }).await
    }

    #[tool(
        description = r#"Query the DOM of the current page using a CSS selector OR a JavaScript expression. Exactly one of `selector` or `script` is required.

USE WHEN: extracting structured data, or running custom DOM logic. NOT WHEN: matching by visible text — CSS does not support `:contains()`; use a script: `Array.from(document.querySelectorAll('button')).find(b => b.textContent.includes('Submit'))`. NOT WHEN: you want to navigate — element.click() and form.submit() inside script DO trigger navigation, but the result shape is different; use browser39_click or browser39_submit when navigation is the goal.

Effect: read in selector mode; local-mutate or external-mutate in script mode (scripts can fill fields, dispatch events, or trigger requests via .click()/.submit()).

Selector parameter accepts CSS only. See docs/selectors.md.

Script mode runs against the static parsed HTML in a deno_core sandbox with full DOM API: traversal (parentElement, children, closest, matches), lookup (getElementById, getElementsByClassName/TagName), mutation (createElement, appendChild, setAttribute, innerHTML/textContent setters), events (addEventListener, dispatchEvent, new Event), and globals (console.log, setTimeout, atob/btoa, localStorage). Console output is captured.

Common failures:
- NO_PAGE → no current page; call browser39_fetch first.
- DOM_QUERY_ERROR → invalid CSS selector or JS exception; the message will name the line/symbol. Reading the relevant section with browser39_fetch + selector first usually clarifies the structure.
- Script returned undefined → ensure the last expression is the value you want; the engine returns the final expression, not all intermediate results."#
    )]
    async fn browser39_dom_query(
        &self,
        Parameters(params): Parameters<DomQueryParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::DomQuery { params, tx }).await
    }

    #[tool(
        description = r#"Fill form field(s) by CSS selector. Works on `<input>`, `<textarea>`, and `<select>` elements. Values persist in session state until form submission.

USE WHEN: setting form values before browser39_submit. NOT WHEN: clicking a submit button — values are sent automatically by browser39_submit; you do not need to fill the submit input itself.

Effect: local-mutate (changes session DOM state; does not send a request).

Selector parameter accepts CSS only. See docs/selectors.md.

Pass either `selector` + `value` for one field, or `fields: [{selector, value}, ...]` for many. If both are present, `fields` wins.

Common failures:
- NO_PAGE → no current page; call browser39_fetch first.
- SELECTOR_NOT_FOUND → selector matched zero elements; verify with browser39_dom_query first.
- Field is read-only / disabled → fill silently no-ops on disabled inputs; the form will not submit them.
- For passwords or tokens: pass values through secret handles (${browser39_secret_N}) so they don't leak into the conversation."#
    )]
    async fn browser39_fill(
        &self,
        Parameters(params): Parameters<FillParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Fill { params, tx }).await
    }

    #[tool(
        description = r#"Submit a form by CSS selector pointing at the `<form>` element. Sends every named field (including values set by browser39_fill) using the form's method and action. Returns the response page as markdown.

USE WHEN: completing a form interaction after browser39_fill. NOT WHEN: the action is a remote API and you already know the URL — use browser39_fetch with method=POST and a body instead.

Effect: external-mutate. The submit hits the form's action URL, which may change remote state (login, post, purchase, send). Always read the form's action and method (browser39_dom_query selector=form, attr=action) before calling on high-stakes flows.

Selector parameter accepts CSS only — must resolve to a `<form>` element. See docs/selectors.md.

Common failures:
- NO_PAGE → no current page; call browser39_fetch first.
- FORM_NOT_FOUND → selector matched something other than a `<form>`; double-check with browser39_dom_query.
- SELECTOR_NOT_FOUND → selector matched zero elements.
- HTTP_ERROR → server rejected the submission; inspect the returned page for validation messages, fix fields with browser39_fill, retry.
- Submitted but nothing changed → the form may rely on JS that browser39 doesn't run; inspect the form's onsubmit handler with browser39_dom_query."#
    )]
    async fn browser39_submit(
        &self,
        Parameters(params): Parameters<SubmitParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Submit { params, tx }).await
    }

    #[tool(
        description = r#"List cookies in the session cookie jar, optionally filtered by domain.

USE WHEN: debugging an auth flow, verifying a Set-Cookie landed, or building a request that needs to mirror the jar. NOT WHEN: you only need the cookie value for an outbound request — browser39 attaches jar cookies to matching requests automatically.

Effect: read. Sensitive cookie values are masked (only the secret handle is returned).

Common failures: none typical. Empty result simply means no cookies in jar for the domain filter."#
    )]
    async fn browser39_cookies(
        &self,
        Parameters(params): Parameters<CookiesParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Cookies { params, tx }).await
    }

    #[tool(
        description = r#"Insert or update a cookie in the session cookie jar.

USE WHEN: priming auth state from a known token, replicating a Set-Cookie observed out of band, or testing cookie-driven behavior. NOT WHEN: the server already issued the cookie via Set-Cookie — browser39 stored it automatically.

Effect: local-mutate (cookie jar only; does not send a request).

Common failures:
- COOKIE_ERROR → invalid domain or malformed value; ensure `domain` matches the host you intend (no scheme, no port).
- The cookie won't be sent → check `secure: true` requires HTTPS, and `domain` matches the request URL's host (or is a parent domain)."#
    )]
    async fn browser39_set_cookie(
        &self,
        Parameters(params): Parameters<SetCookieParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::SetCookie { params, tx }).await
    }

    #[tool(
        description = r#"Delete a cookie from the session cookie jar by name + domain.

USE WHEN: clearing auth between flows, testing logout, or removing a stale jar entry. NOT WHEN: you want the server to invalidate the session — call the logout endpoint instead.

Effect: local-mutate.

Common failures:
- Returns deleted: false → the (name, domain) pair didn't exist; not an error. Use browser39_cookies to confirm what's actually in the jar."#
    )]
    async fn browser39_delete_cookie(
        &self,
        Parameters(params): Parameters<DeleteCookieParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::DeleteCookie { params, tx }).await
    }

    #[tool(
        description = r#"Read a localStorage value for an origin. Defaults to the current page's origin.

USE WHEN: inspecting client-side state set by the page or by a previous browser39 script. NOT WHEN: the value lives in cookies — use browser39_cookies. NOT WHEN: you want sessionStorage — that's not stored across the headless model.

Effect: read. Sensitive values are masked.

Common failures:
- NO_PAGE → no current page and no `origin` provided. Either fetch a page first or pass `origin` explicitly (e.g., `https://example.com`).
- value is null → key doesn't exist for that origin; not an error."#
    )]
    async fn browser39_storage_get(
        &self,
        Parameters(params): Parameters<StorageGetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageGet { params, tx }).await
    }

    #[tool(
        description = r#"Write a localStorage value for an origin. Defaults to the current page's origin.

USE WHEN: priming app state for an SPA, or writing a value the page would normally set itself (e.g., consent flags). NOT WHEN: the value is sensitive — pass it through a secret handle (${browser39_secret_N}) instead of inline so it doesn't leak.

Effect: local-mutate (browser-side storage only; no network).

Common failures:
- NO_PAGE → no current page and no `origin`; pass `origin` or fetch a page first."#
    )]
    async fn browser39_storage_set(
        &self,
        Parameters(params): Parameters<StorageSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageSet { params, tx }).await
    }

    #[tool(
        description = r#"Delete a localStorage key for an origin.

USE WHEN: clearing a stale or invalid client-side flag. NOT WHEN: clearing the entire origin's storage — use browser39_storage_clear.

Effect: local-mutate.

Common failures:
- NO_PAGE → no current page and no `origin`; pass `origin` or fetch a page first.
- Returns deleted: false → key didn't exist; not an error."#
    )]
    async fn browser39_storage_delete(
        &self,
        Parameters(params): Parameters<StorageDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageDelete { params, tx }).await
    }

    #[tool(
        description = r#"List every localStorage key/value pair for an origin.

USE WHEN: discovering what an SPA stores client-side; debugging a flow that depends on storage. NOT WHEN: you already know the key — use browser39_storage_get.

Effect: read. Sensitive values are masked.

Common failures:
- NO_PAGE → no current page and no `origin`; pass `origin` explicitly or fetch first."#
    )]
    async fn browser39_storage_list(
        &self,
        Parameters(params): Parameters<StorageListParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageList { params, tx }).await
    }

    #[tool(
        description = r#"Wipe all localStorage entries for an origin.

USE WHEN: resetting an app's client state to defaults. NOT WHEN: only one key is stale — use browser39_storage_delete to avoid losing other state.

Effect: local-mutate. Returns the count of cleared entries.

Common failures:
- NO_PAGE → no current page and no `origin`; pass `origin` or fetch a page first."#
    )]
    async fn browser39_storage_clear(
        &self,
        Parameters(params): Parameters<StorageClearParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageClear { params, tx }).await
    }

    #[tool(
        description = r#"Search the web using the configured search engine (default: DuckDuckGo) and return the result page as markdown.

USE WHEN: discovering URLs you don't know yet. NOT WHEN: you already have a URL — call browser39_fetch directly. NOT WHEN: searching within a single site — use the site's own search box via browser39_fill + browser39_submit.

Effect: read (the search engine is queried; this is technically external, but no state-changing request).

Common failures:
- HTTP_ERROR / TIMEOUT → search engine unreachable; retryable.
- Results page schema changed → markdown will still render but extracting links by index may shift; rely on link text or re-call browser39_links after."#
    )]
    async fn browser39_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Search { params, tx }).await
    }

    #[tool(
        description = r#"Navigate one step back in the session's browsing history. Returns the prior page.

USE WHEN: undoing a navigation; you went somewhere and want to return. NOT WHEN: you want a specific past URL — use browser39_history to find it then browser39_fetch directly.

Effect: read (history is in-memory; the page is re-rendered from cache, no network).

Common failures:
- NO_HISTORY → already at the oldest entry; nowhere to go back to."#
    )]
    async fn browser39_back(&self) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Back { tx }).await
    }

    #[tool(
        description = r#"Navigate one step forward in the session's browsing history. Inverse of browser39_back.

USE WHEN: redoing a navigation after browser39_back. NOT WHEN: you haven't gone back — there's nothing forward to go to.

Effect: read.

Common failures:
- NO_HISTORY → already at the newest entry; nowhere to go forward to."#
    )]
    async fn browser39_forward(&self) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Forward { tx }).await
    }

    #[tool(
        description = r#"Search or list the session's browsing history. Returns recent pages, optionally filtered by a query that matches URL or title (case-insensitive).

USE WHEN: finding a specific past URL; inspecting the navigation trail. Defaults to the 10 most recent entries; pass `limit` to widen.

Effect: read.

Common failures: none typical. Empty `entries` simply means no matches (or no history yet)."#
    )]
    async fn browser39_history(
        &self,
        Parameters(params): Parameters<HistoryParams>,
    ) -> Result<String, String> {
        let result = send_cmd(&self.cmd_tx, |tx| McpCommand::History { params, tx })
            .await
            .map_err(|e| e.to_string())?;
        to_json(&result)
    }

    #[tool(
        description = r#"Get session info and confirm liveness — current URL, title, history depth, cookie count, uptime. Doubles as a heartbeat.

USE WHEN: orienting at session start, after a long pause, or to verify browser39 is still responsive.

Effect: read.

Common failures: none. Always returns even if no page has been loaded yet (current_url and title will be null)."#
    )]
    async fn browser39_info(&self) -> Result<String, String> {
        let result = send_cmd(&self.cmd_tx, |tx| McpCommand::Info { tx })
            .await
            .map_err(|e| e.to_string())?;
        to_json(&result)
    }

    // ─── Config Management Tools ───────────────────────────────────

    #[tool(
        description = r#"Show browser39 configuration with sensitive values masked. Never returns the raw config file. Use `section` to filter: session, search, auth, cookies, storage, headers, security. Omit to show all sections.

USE WHEN: inspecting current settings; verifying an auth profile is wired correctly. NOT WHEN: trying to read a credential — masked values cannot be unmasked through MCP.

Effect: read.

Common failures:
- Unknown section → message lists the seven valid sections."#
    )]
    async fn browser39_config_show(
        &self,
        Parameters(params): Parameters<ConfigShowParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigShow { params, tx }).await
    }

    #[tool(
        description = r#"Set a browser39 config value by dot-key. Allowed keys: session.start_url, session.user_agent, session.timeout_secs, session.max_redirects, session.persistence, session.defaults.max_tokens, session.defaults.strip_nav, session.defaults.include_links, session.defaults.include_images, search.engine.

USE WHEN: persisting a tweak across restarts. NOT WHEN: you want a one-shot override for a single fetch — pass the option inline on the call instead.

Effect: local-mutate (writes the config file on disk and reloads the session).

Common failures:
- Unknown key → message lists every allowed key.
- Type error (e.g., timeout_secs not an integer) → message names the expected type.
- session.persistence values must be 'disk' or 'memory'."#
    )]
    async fn browser39_config_set(
        &self,
        Parameters(params): Parameters<ConfigSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigSet { params, tx }).await
    }

    #[tool(
        description = r#"Add or update a named auth profile. Credentials are stored on disk and never returned via MCP — agents reference profiles by name and never see the value.

USE WHEN: priming an auth flow that needs persistent credentials (Bearer tokens, API keys). Pass `value` for a literal, or `value_env` to read from a process env var (preferred — avoids the value passing through MCP at all).

Effect: local-mutate.

Common failures:
- domains list is empty → at least one domain (with optional `*.` wildcard) is required, or all requests using this profile will be rejected.
- Wildcard misuse → `*.example.com` matches subdomains only, not the bare apex; include both if needed."#
    )]
    async fn browser39_config_auth_set(
        &self,
        Parameters(params): Parameters<ConfigAuthSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigAuthSet { params, tx }).await
    }

    #[tool(
        description = r#"Delete an auth profile by name.

USE WHEN: rotating credentials, or removing access to a service. NOT WHEN: you want to keep the profile but disable temporarily — clear the `value`/`value_env` field with browser39_config_auth_set instead.

Effect: local-mutate.

Common failures:
- profile not found → message names the missing profile; use browser39_config_show to list current profiles."#
    )]
    async fn browser39_config_auth_delete(
        &self,
        Parameters(params): Parameters<ConfigAuthDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigAuthDelete {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Add or update a preloaded cookie that browser39 injects into the jar at session startup. Sensitive cookies are masked in browser39_config_show.

USE WHEN: priming an authenticated session that uses cookies. Set `sensitive: true` for credential cookies so they're never echoed.

Effect: local-mutate (config file).

Common failures:
- Identical (name, domain) is replaced silently — this is by design; check browser39_config_show to confirm the active value."#
    )]
    async fn browser39_config_cookie_set(
        &self,
        Parameters(params): Parameters<ConfigCookieSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigCookieSet {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Remove a preloaded cookie from the config (matched by name + domain).

USE WHEN: pruning stale preloads. NOT WHEN: clearing a runtime cookie — use browser39_delete_cookie for the live jar.

Effect: local-mutate.

Common failures:
- not found → name and domain must match exactly."#
    )]
    async fn browser39_config_cookie_delete(
        &self,
        Parameters(params): Parameters<ConfigCookieDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigCookieDelete {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Add or update a preloaded localStorage entry that browser39 injects at session startup. Sensitive entries are masked in browser39_config_show.

USE WHEN: priming an SPA with a known token or flag. NOT WHEN: the value should not survive restarts — use browser39_storage_set at runtime instead.

Effect: local-mutate (config file).

Common failures:
- Identical (origin, key) is replaced silently."#
    )]
    async fn browser39_config_storage_set(
        &self,
        Parameters(params): Parameters<ConfigStorageSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigStorageSet {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Remove a preloaded localStorage entry from the config (matched by origin + key).

Effect: local-mutate.

Common failures:
- not found → origin and key must match exactly."#
    )]
    async fn browser39_config_storage_delete(
        &self,
        Parameters(params): Parameters<ConfigStorageDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigStorageDelete {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Add or update a default-header rule that applies to every matching request. Domains list defines which hosts the rule applies to (wildcards supported).

USE WHEN: a service requires a constant header (Accept, X-Client-Version) on every request. NOT WHEN: a single fetch needs a one-off header — pass `headers` inline on browser39_fetch instead.

Effect: local-mutate.

Common failures:
- Replacing an existing rule requires the domain list to match exactly — otherwise both rules end up in config; check with browser39_config_show."#
    )]
    async fn browser39_config_header_set(
        &self,
        Parameters(params): Parameters<ConfigHeaderSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigHeaderSet {
            params,
            tx,
        })
        .await
    }

    #[tool(
        description = r#"Remove a default-header rule by its domain list. The domain list must match the existing rule exactly.

Effect: local-mutate.

Common failures:
- not found → the rule's `domains` array must match exactly (same set, same order); verify via browser39_config_show."#
    )]
    async fn browser39_config_header_delete(
        &self,
        Parameters(params): Parameters<ConfigHeaderDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::ConfigHeaderDelete {
            params,
            tx,
        })
        .await
    }
}

/// Helper for tools that return a page (fetch, click, back, forward, submit).
async fn page_cmd(
    tx: &mpsc::Sender<McpCommand>,
    f: impl FnOnce(tokio::sync::oneshot::Sender<anyhow::Result<PageResult>>) -> McpCommand,
) -> Result<String, String> {
    send_cmd(tx, f)
        .await
        .map_err(|e| e.to_string())?
        .map(|page| format_page_result(&page))
        .map_err(format_error)
}

/// Helper for tools that return serializable data.
async fn data_cmd<T: serde::Serialize>(
    tx: &mpsc::Sender<McpCommand>,
    f: impl FnOnce(tokio::sync::oneshot::Sender<anyhow::Result<T>>) -> McpCommand,
) -> Result<String, String> {
    send_cmd(tx, f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(format_error)
        .and_then(|v| to_json(&v))
}

fn to_json(v: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(v).map_err(|e| format!("serialization failed: {e}"))
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("browser39", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "browser39 is a headless web browser for AI agents. \
             Use browser39_fetch to load pages as markdown, browser39_links to list links, \
             browser39_click to follow links, and browser39_dom_query for CSS/JS queries. \
             The JS sandbox supports full DOM traversal (parentElement, children, siblings, closest), \
             DOM lookup (getElementById, getElementsByClassName, getElementsByTagName), \
             DOM mutation (createElement, appendChild, setAttribute, innerHTML setter), \
             event handling (addEventListener, dispatchEvent, new Event/CustomEvent), \
             and Web APIs (localStorage, document.cookie, console.log, setTimeout, atob/btoa). \
             Use browser39_fill + browser39_submit for form interactions, \
             and browser39_search to search the web.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + Send + '_
    {
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![
            make_resource(
                "browser39://page",
                "Current page",
                Some("text/markdown"),
                Some("Current page content as markdown"),
            ),
            make_resource(
                "browser39://page/links",
                "Page links",
                Some("application/json"),
                Some("Links on the current page as JSON"),
            ),
            make_resource(
                "browser39://page/meta",
                "Page metadata",
                Some("application/json"),
                Some("Current page metadata as JSON"),
            ),
            make_resource(
                "browser39://cookies",
                "Cookies",
                Some("application/json"),
                Some("Cookies for the current domain as JSON"),
            ),
        ])))
    }

    #[allow(clippy::manual_async_fn)]
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let uri = &request.uri;
            match uri.as_str() {
                "browser39://page" => {
                    let markdown = send_cmd(&self.cmd_tx, |tx| McpCommand::GetPageMarkdown { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        markdown, uri,
                    )]))
                }
                "browser39://page/links" => {
                    let links = send_cmd(&self.cmd_tx, |tx| McpCommand::Links { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&links)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                "browser39://page/meta" => {
                    let meta = send_cmd(&self.cmd_tx, |tx| McpCommand::GetPageMeta { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&meta)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                "browser39://cookies" => {
                    let cookies = send_cmd(&self.cmd_tx, |tx| McpCommand::Cookies {
                        params: super::params::CookiesParams { domain: None },
                        tx,
                    })
                    .await
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&cookies)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                _ => Err(rmcp::ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("unknown resource: {uri}"),
                    None,
                )),
            }
        }
    }
}

use crate::core::page::PageResult;

fn make_resource(
    uri: &str,
    name: &str,
    mime_type: Option<&str>,
    description: Option<&str>,
) -> Resource {
    Resource::new(
        RawResource {
            uri: uri.into(),
            name: name.into(),
            title: None,
            description: description.map(Into::into),
            mime_type: mime_type.map(Into::into),
            size: None,
            icons: None,
            meta: None,
        },
        None,
    )
}

fn format_page_result(page: &PageResult) -> String {
    let mut out = page.markdown.clone();
    // Append metadata summary
    out.push_str("\n\n---\n");
    if let Some(ref title) = page.title {
        out.push_str(&format!("**Title:** {title}\n"));
    }
    out.push_str(&format!("**URL:** {}\n", page.url));
    out.push_str(&format!("**Status:** {}\n", page.status));
    if let Some(ref links) = page.links {
        out.push_str(&format!("**Links:** {}\n", links.len()));
    }
    out.push_str(&format!("**Tokens (est):** {}\n", page.stats.tokens_est));
    if page.truncated {
        if let Some(next) = page.next_offset {
            out.push_str(&format!(
                "**Truncated:** true (use offset={next} to continue)\n"
            ));
        } else {
            out.push_str("**Truncated:** true\n");
        }
    }
    if let Some(ref selectors) = page.content_selectors {
        out.push_str("\n**Content selectors:**\n");
        for cs in selectors {
            if let Some(ref label) = cs.label {
                out.push_str(&format!(
                    "- `{}` — {label} (~{} tokens)\n",
                    cs.selector, cs.tokens_est
                ));
            } else {
                out.push_str(&format!(
                    "- `{}` — ~{} tokens\n",
                    cs.selector, cs.tokens_est
                ));
            }
        }
    }
    out
}

fn format_error(err: anyhow::Error) -> String {
    let se = crate::service::service::classify_error(err);
    let retry_hint = if se.code.retryable() {
        " (retryable)"
    } else {
        ""
    };
    format!("[{:?}] {}{retry_hint}", se.code, se.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot the rendered MCP tool list (name + description + input schema)
    /// so any unintended description or schema change surfaces as a reviewable
    /// diff in PRs. To accept changes intentionally:
    ///
    ///     UPDATE_SNAPSHOTS=1 cargo test tool_schema_snapshot -- --nocapture
    #[test]
    fn tool_schema_snapshot() {
        let router = McpServer::tool_router();
        let mut tools: Vec<&rmcp::model::Tool> =
            router.map.values().map(|r| &r.attr).collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        let mut rendered = String::new();
        for tool in &tools {
            rendered.push_str(&format!("## {}\n\n", tool.name));
            if let Some(desc) = &tool.description {
                rendered.push_str(desc.as_ref());
                rendered.push_str("\n\n");
            }
            let schema = serde_json::to_string_pretty(&*tool.input_schema)
                .expect("input_schema serializes");
            rendered.push_str("```json\n");
            rendered.push_str(&schema);
            rendered.push_str("\n```\n\n");
        }

        let snapshot_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/mcp/snapshots/tool_schema.snap");

        if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
            std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
            std::fs::write(&snapshot_path, &rendered).unwrap();
            eprintln!("snapshot written to {}", snapshot_path.display());
            return;
        }

        let expected = match std::fs::read_to_string(&snapshot_path) {
            Ok(s) => s,
            Err(_) => {
                std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
                std::fs::write(&snapshot_path, &rendered).unwrap();
                panic!(
                    "snapshot did not exist; wrote initial copy to {}. Re-run tests.",
                    snapshot_path.display()
                );
            }
        };

        if expected != rendered {
            panic!(
                "MCP tool schema snapshot mismatch.\n\
                 To accept the new output:\n    \
                 UPDATE_SNAPSHOTS=1 cargo test tool_schema_snapshot\n\n\
                 (snapshot file: {})",
                snapshot_path.display(),
            );
        }
    }
}
