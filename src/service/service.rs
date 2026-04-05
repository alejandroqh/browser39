use anyhow::{Context, Result};
use std::collections::HashMap;
use std::mem;
use std::time::Instant;

use scraper::Html;

use crate::core::auth;
use crate::core::config::Config;
use crate::core::dom_query::query_selector;
use crate::core::dom_script::execute_script;
use crate::core::error::ErrorCode;
use crate::core::form;
use crate::core::html_to_md::ParsedHtml;
use crate::core::http_client::{HttpClient, ResponseBody};
use crate::core::page::*;
use crate::core::redaction::{RedactionEngine, Transport};
use crate::core::secrets::SecretStore;
use crate::core::session_store::{HistoryEntry, SessionSnapshot, SessionStore};
use crate::core::url::detect_client_redirect;

// --- ServiceError ---

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ServiceError {
    pub code: ErrorCode,
    pub message: String,
}

impl ServiceError {
    pub fn new(code: ErrorCode, message: String) -> Self {
        Self { code, message }
    }
}

fn classify_reqwest(re: &reqwest::Error) -> ServiceError {
    if re.is_timeout() {
        return ServiceError::new(ErrorCode::Timeout, format!("request timed out: {re}"));
    }
    if re.is_connect() {
        return ServiceError::new(ErrorCode::HttpError, format!("connection failed: {re}"));
    }
    if re.is_redirect() {
        return ServiceError::new(ErrorCode::HttpError, format!("too many redirects: {re}"));
    }
    if let Some(status) = re.status() {
        return ServiceError::new(ErrorCode::HttpError, format!("HTTP {status}: {re}"));
    }
    ServiceError::new(ErrorCode::HttpError, format!("HTTP request failed: {re}"))
}

/// Inspect an `anyhow::Error` chain and produce the most specific `ServiceError`.
///
/// Walks the error chain so `.context()`-wrapped reqwest/URL errors are also detected.
pub fn classify_error(err: anyhow::Error) -> ServiceError {
    for cause in err.chain() {
        if let Some(se) = cause.downcast_ref::<ServiceError>() {
            return ServiceError::new(se.code.clone(), se.message.clone());
        }
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            return classify_reqwest(re);
        }
        if cause.downcast_ref::<url::ParseError>().is_some() {
            return ServiceError::new(ErrorCode::InvalidUrl, err.to_string());
        }
    }
    ServiceError::new(ErrorCode::SessionError, err.to_string())
}

// --- Internal page state ---

struct PageState {
    html: String,
    links: Vec<Link>,
    result: PageResult,
}

// --- BrowserService ---

pub struct BrowserService {
    http: HttpClient,
    config: Config,
    history: Vec<PageState>,
    history_index: usize,
    storage: HashMap<String, HashMap<String, String>>,
    /// Filled form field overlays: CSS selector → value.
    filled_fields: HashMap<String, String>,
    started_at: Instant,
    secrets: SecretStore,
    redaction: RedactionEngine,
    transport: Transport,
    store: Box<dyn SessionStore>,
}

impl BrowserService {
    pub async fn new(config: Config, store: Box<dyn SessionStore>) -> Result<Self> {
        Self::with_transport(config, Transport::Jsonl, store).await
    }

    pub async fn with_transport(
        config: Config,
        transport: Transport,
        store: Box<dyn SessionStore>,
    ) -> Result<Self> {
        let http = HttpClient::new(&config)?;

        // Initialize storage from config [[storage]] entries
        let mut storage: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut secrets = SecretStore::new();

        for entry in &config.storage {
            if let Some(ref val) = entry.resolved_value {
                if entry.sensitive {
                    let handle = secrets.store(val);
                    storage
                        .entry(entry.origin.clone())
                        .or_default()
                        .insert(entry.key.clone(), handle);
                } else {
                    storage
                        .entry(entry.origin.clone())
                        .or_default()
                        .insert(entry.key.clone(), val.clone());
                }
            }
        }

        // Pre-store sensitive cookie values as secrets
        for cookie_cfg in &config.cookies {
            if cookie_cfg.sensitive
                && let Some(ref val) = cookie_cfg.resolved_value
            {
                secrets.store(val);
            }
        }

        let redaction = RedactionEngine::new(&config.security, transport);
        let start_url = config.session.start_url.clone();

        let mut history = Vec::new();
        let mut history_index = 0;

        // Restore session from store
        if let Ok(Some(snapshot)) = store.load() {
            // Restore cookies
            if !snapshot.cookies_json.is_empty()
                && let Err(e) = http.jar().import_json(&snapshot.cookies_json)
            {
                eprintln!("warning: could not restore cookies: {e}");
            }

            // Restore localStorage (merge: config entries take precedence)
            for (origin, entries) in snapshot.storage {
                let origin_map = storage.entry(origin).or_default();
                for (key, value) in entries {
                    origin_map.entry(key).or_insert(value);
                }
            }

            // Restore history as stub entries
            for entry in &snapshot.history {
                history.push(PageState {
                    html: String::new(),
                    links: Vec::new(),
                    result: PageResult {
                        url: entry.url.clone(),
                        title: entry.title.clone(),
                        status: entry.status,
                        markdown: format!(
                            "*Session restored — re-fetch to load content for {}*",
                            entry.url
                        ),
                        links: None,
                        meta: PageMetadata::default(),
                        stats: PageStats {
                            fetch_ms: 0,
                            tokens_est: 0,
                            content_bytes: 0,
                        },
                        truncated: false,
                        next_offset: None,
                        content_selectors: None,
                    },
                });
            }
            history_index = snapshot.history_index.min(history.len().saturating_sub(1));
        }

        let mut service = Self {
            http,
            config,
            history,
            history_index,
            storage,
            filled_fields: HashMap::new(),
            started_at: Instant::now(),
            secrets,
            redaction,
            transport,
            store,
        };

        // Auto-fetch start_url if configured
        if let Some(url) = start_url {
            let opts = service.default_fetch_options();
            service
                .fetch(&url, &HttpMethod::Get, &HashMap::new(), None, None, &opts)
                .await?;
        }

        Ok(service)
    }

    /// Build a snapshot of current state and persist it.
    fn save_snapshot(&self) {
        let cookies_json = self.http.jar().export_json().unwrap_or_default();

        let history: Vec<HistoryEntry> = self
            .history
            .iter()
            .map(|s| HistoryEntry {
                url: s.result.url.clone(),
                title: s.result.title.clone(),
                status: s.result.status,
            })
            .collect();

        // Resolve secret handles back to real values before persisting.
        // The session file is encrypted, so storing real values is safe.
        // Handles are ephemeral and would become unresolvable after restart.
        let storage: HashMap<String, HashMap<String, String>> = self
            .storage
            .iter()
            .map(|(origin, entries)| {
                let resolved = entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.secrets.resolve(v)))
                    .collect();
                (origin.clone(), resolved)
            })
            .collect();

        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            cookies_json,
            storage,
            history,
            history_index: self.history_index,
        };

        if let Err(e) = self.store.save(&snapshot) {
            eprintln!("warning: failed to save session: {e}");
        }
    }

    pub fn search_url(&self, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        self.config.search.engine.replacen("{}", &encoded, 1)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Reload config after a change. Updates in-memory config, re-resolves env vars,
    /// rebuilds redaction engine, updates header rules, and preloads cookies/storage/auth secrets.
    pub fn reload_config(&mut self, mut config: Config) -> Result<()> {
        config.resolve()?;

        self.redaction = RedactionEngine::new(&config.security, self.transport);
        self.http.set_header_rules(config.headers.clone());

        // Preload cookies into cookie jar
        for cookie_cfg in &config.cookies {
            if let Some(ref val) = cookie_cfg.resolved_value {
                self.http.jar().set_cookie(
                    &cookie_cfg.name,
                    val,
                    &cookie_cfg.domain,
                    &cookie_cfg.path,
                    cookie_cfg.secure,
                    cookie_cfg.http_only,
                    None,
                )?;
                if cookie_cfg.sensitive {
                    self.secrets.store(val);
                }
            }
        }

        // Preload new storage entries
        for entry in &config.storage {
            if let Some(ref val) = entry.resolved_value {
                let stored_val = if entry.sensitive {
                    self.secrets.store(val)
                } else {
                    val.clone()
                };
                self.storage
                    .entry(entry.origin.clone())
                    .or_default()
                    .insert(entry.key.clone(), stored_val);
            }
        }

        // Pre-store sensitive auth values as secrets
        for profile in config.auth.values() {
            if let Some(ref val) = profile.resolved_value {
                self.secrets.store(val);
            }
        }

        self.config = config;
        Ok(())
    }

    pub fn default_fetch_options(&self) -> FetchOptions {
        let d = &self.config.session.defaults;
        FetchOptions {
            max_tokens: d.max_tokens,
            offset: 0,
            selector: None,
            strip_nav: d.strip_nav,
            include_links: d.include_links,
            include_images: d.include_images,
            timeout_secs: self.config.session.timeout_secs,
            compact_links: false,
            show_selectors_first: true,
            download_path: None,
        }
    }

    pub fn merge_options(&self, options: &FetchOptions) -> FetchOptions {
        let mut opts = options.clone();
        if opts.max_tokens.is_none() {
            opts.max_tokens = self.config.session.defaults.max_tokens;
        }
        opts
    }

    pub async fn fetch(
        &mut self,
        url: &str,
        method: &HttpMethod,
        headers: &HashMap<String, String>,
        body: Option<String>,
        auth_profile: Option<&str>,
        options: &FetchOptions,
    ) -> Result<PageResult> {
        let options = self.merge_options(options);

        // Resolve auth profile if specified
        let mut all_headers = headers.clone();
        if let Some(profile_name) = auth_profile {
            self.apply_auth_profile(profile_name, url, &mut all_headers)?;
        }

        // Make HTTP request
        let response = self
            .http
            .fetch(url, method, &all_headers, body, Some(options.timeout_secs))
            .await?;

        // Handle binary content early — skip redirect detection and HTML parsing
        if let ResponseBody::Binary(ref raw_bytes) = response.body {
            let mime_base = response
                .headers
                .get("content-type")
                .and_then(|ct| ct.split(';').next())
                .map(|s| s.trim())
                .unwrap_or("application/octet-stream");
            let markdown = if let Some(ref path) = options.download_path {
                std::fs::write(path, raw_bytes)
                    .with_context(|| format!("failed to write download to {}", path))?;
                format!(
                    "[Downloaded {}, {} bytes to {}]",
                    mime_base, response.content_length, path
                )
            } else {
                format!(
                    "[Binary content: {}, {} bytes]",
                    mime_base, response.content_length
                )
            };
            let result = PageResult {
                url: response.url.clone(),
                title: None,
                status: response.status,
                markdown,
                links: None,
                meta: PageMetadata {
                    lang: None,
                    description: None,
                    content_type: Some(mime_base.to_string()),
                },
                stats: PageStats {
                    fetch_ms: response.elapsed_ms,
                    tokens_est: 10,
                    content_bytes: response.content_length,
                },
                truncated: false,
                next_offset: None,
                content_selectors: None,
            };
            let state = PageState {
                html: String::new(),
                links: vec![],
                result: result.clone(),
            };
            self.push_history(state);
            return Ok(result);
        }

        // Text content — extract the HTML string
        let html = match response.body {
            ResponseBody::Text(s) => s,
            ResponseBody::Binary(_) => unreachable!(),
        };

        // Follow client-side redirects (meta-refresh, JS location changes) up to 5 hops
        let mut final_url = response.url;
        let mut final_html = html;
        let mut final_elapsed = response.elapsed_ms;
        let mut final_content_length = response.content_length;
        let mut final_status = response.status;
        for _ in 0..5 {
            match detect_client_redirect(&final_html, &final_url) {
                Some(redirect_url) if redirect_url != final_url => {
                    let redir = self
                        .http
                        .fetch(
                            &redirect_url,
                            &HttpMethod::Get,
                            &all_headers,
                            None,
                            Some(options.timeout_secs),
                        )
                        .await?;
                    final_url = redir.url;
                    final_elapsed = redir.elapsed_ms;
                    final_content_length = redir.content_length;
                    final_status = redir.status;
                    final_html = match redir.body {
                        ResponseBody::Text(s) => s,
                        ResponseBody::Binary(_) => break,
                    };
                }
                _ => break,
            }
        }

        // Parse HTML
        let parsed = ParsedHtml::parse(&final_html);
        let title = parsed.title();
        let meta = parsed.metadata();

        // Detect content selectors before expensive markdown conversion
        let content_selectors = if options.show_selectors_first && options.selector.is_none() {
            let selectors = parsed.detect_content_selectors();
            if selectors.is_empty() {
                None
            } else {
                Some(selectors)
            }
        } else {
            None
        };

        // Build result: skip full markdown conversion when returning selectors
        let (markdown, tokens_est, truncated, next_offset, links) = match content_selectors {
            Some(ref _sels) => {
                let tokens = parsed.estimate_page_tokens();
                let msg = "Page has content sections. Re-fetch with a `selector` for targeted content, or set `show_selectors_first=false` for the raw page.".to_string();
                let links = parsed.links(Some(&final_url));
                (msg, tokens, false, None, links)
            }
            None => {
                let mut convert = parsed.to_markdown_with_options(&options, Some(&final_url));
                let links = if options.compact_links {
                    mem::take(&mut convert.links)
                } else {
                    parsed.links(Some(&final_url))
                };
                (
                    convert.markdown,
                    convert.tokens_est,
                    convert.truncated,
                    convert.next_offset,
                    links,
                )
            }
        };

        let result = PageResult {
            url: final_url,
            title,
            status: final_status,
            markdown,
            links: if options.include_links {
                Some(links.clone())
            } else {
                None
            },
            meta,
            stats: PageStats {
                fetch_ms: final_elapsed,
                tokens_est,
                content_bytes: final_content_length,
            },
            truncated,
            next_offset,
            content_selectors,
        };

        let state = PageState {
            html: final_html,
            links,
            result: result.clone(),
        };
        self.push_history(state);

        Ok(result)
    }

    fn push_history(&mut self, state: PageState) {
        if !self.history.is_empty() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(state);
        self.history_index = self.history.len() - 1;
        self.save_snapshot();
    }

    pub async fn fetch_by_index(
        &mut self,
        index: usize,
        method: &HttpMethod,
        headers: &HashMap<String, String>,
        body: Option<String>,
        auth_profile: Option<&str>,
        options: &FetchOptions,
    ) -> Result<PageResult> {
        let url = self.resolve_link_by_index(index)?;
        self.fetch(&url, method, headers, body, auth_profile, options)
            .await
    }

    pub async fn fetch_by_text(
        &mut self,
        text: &str,
        method: &HttpMethod,
        headers: &HashMap<String, String>,
        body: Option<String>,
        auth_profile: Option<&str>,
        options: &FetchOptions,
    ) -> Result<PageResult> {
        let url = self.resolve_link_by_text(text)?;
        self.fetch(&url, method, headers, body, auth_profile, options)
            .await
    }

    pub fn links(&self) -> Result<LinksResult> {
        let current = self.current_page()?;
        Ok(LinksResult {
            links: current.links.clone(),
            count: current.links.len(),
        })
    }

    pub fn history(&self, query: Option<&str>, limit: usize) -> HistoryResult {
        let total = self.history.len();
        let entries: Vec<HistoryEntryInfo> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, state)| match query {
                Some(q) => {
                    let q = q.to_lowercase();
                    state.result.url.to_lowercase().contains(&q)
                        || state
                            .result
                            .title
                            .as_ref()
                            .is_some_and(|t| t.to_lowercase().contains(&q))
                }
                None => true,
            })
            .rev()
            .take(limit)
            .map(|(i, state)| HistoryEntryInfo {
                index: i,
                url: state.result.url.clone(),
                title: state.result.title.clone(),
                status: state.result.status,
                current: i == self.history_index,
            })
            .collect();

        let count = entries.len();
        HistoryResult {
            entries,
            count,
            total,
        }
    }

    pub fn back(&mut self) -> Result<PageResult> {
        if self.history.is_empty() || self.history_index == 0 {
            return Err(ServiceError::new(
                ErrorCode::NoHistory,
                "no previous page in history".into(),
            )
            .into());
        }
        self.history_index -= 1;
        Ok(self.history[self.history_index].result.clone())
    }

    pub fn forward(&mut self) -> Result<PageResult> {
        if self.history.is_empty() || self.history_index >= self.history.len() - 1 {
            return Err(
                ServiceError::new(ErrorCode::NoHistory, "no next page in history".into()).into(),
            );
        }
        self.history_index += 1;
        Ok(self.history[self.history_index].result.clone())
    }

    pub fn info(&self) -> InfoResult {
        let (current_url, title) = match self.current_page() {
            Ok(state) => (Some(state.result.url.clone()), state.result.title.clone()),
            Err(_) => (None, None),
        };

        InfoResult {
            version: env!("CARGO_PKG_VERSION").to_string(),
            alive: true,
            current_url,
            title,
            history_length: self.history.len(),
            history_index: self.history_index,
            cookies_count: self.http.jar().count(),
            uptime_secs: self.started_at.elapsed().as_secs(),
        }
    }

    // --- Cookie management ---

    pub fn cookies(&self, domain: Option<&str>) -> Result<CookiesResult> {
        let cookies = self.http.jar().list_cookies(domain);
        let count = cookies.len();
        Ok(CookiesResult { cookies, count })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_cookie(
        &mut self,
        name: &str,
        value: &str,
        domain: &str,
        path: &str,
        secure: bool,
        http_only: bool,
        max_age_secs: Option<u64>,
    ) -> Result<SetCookieResult> {
        self.http
            .jar()
            .set_cookie(name, value, domain, path, secure, http_only, max_age_secs)
            .map_err(|e| {
                ServiceError::new(ErrorCode::CookieError, format!("failed to set cookie: {e}"))
            })?;
        self.save_snapshot();
        Ok(SetCookieResult {
            name: name.to_string(),
            domain: domain.to_string(),
        })
    }

    pub fn delete_cookie(&mut self, name: &str, domain: &str) -> Result<DeleteResult> {
        let deleted = self.http.jar().remove_cookie(name, domain);
        if deleted {
            self.save_snapshot();
        }
        Ok(DeleteResult { deleted })
    }

    /// Fill form fields by CSS selector. Validates that each selector matches
    /// an <input>, <textarea>, or <select> element in the current page.
    pub fn fill(&mut self, fields: &[(String, String)]) -> Result<FillResult> {
        let document = Html::parse_document(self.current_html()?);
        let mut filled = 0;

        for (selector, value) in fields {
            form::validate_field_selector(&document, selector).map_err(form::map_form_error)?;
            self.filled_fields
                .insert(selector.to_string(), value.to_string());
            filled += 1;
        }

        Ok(FillResult { filled })
    }

    /// Submit a form by CSS selector. Merges filled field overlays with DOM
    /// defaults, builds the HTTP request, and navigates to the result.
    pub async fn submit(
        &mut self,
        form_selector: &str,
        options: &FetchOptions,
    ) -> Result<PageResult> {
        let html = self.current_html()?.to_string();
        let current_url = self.current_page()?.result.url.clone();

        let parsed = form::parse_form(&html, form_selector).map_err(form::map_form_error)?;

        // Start with DOM default values, then overlay filled fields.
        // Scoped block because Html is not Send and can't live across .await.
        let (fields, action_url, method) = {
            let mut fields = parsed.fields;
            let document = Html::parse_document(&html);
            for (selector, value) in &self.filled_fields {
                if let Ok(name) = form::validate_field_selector(&document, selector) {
                    fields.insert(name, value.clone());
                }
            }

            let action_url = match parsed.action {
                Some(ref action) if !action.is_empty() => resolve_url(&current_url, action)?,
                _ => current_url.clone(),
            };

            (fields, action_url, parsed.method)
        };

        let body = form::encode_form_urlencoded(&fields);
        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );

        self.filled_fields.clear();

        let options = self.merge_options(options);
        self.fetch(&action_url, &method, &headers, Some(body), None, &options)
            .await
    }

    // --- LocalStorage management ---

    /// Derive origin (scheme://host[:port]) from the current page URL.
    fn current_origin(&self) -> Result<String> {
        let url_str = &self.current_page()?.result.url;
        let url = reqwest::Url::parse(url_str).map_err(|e| {
            ServiceError::new(ErrorCode::SessionError, format!("invalid current URL: {e}"))
        })?;
        let origin = url.origin();
        if !origin.is_tuple() {
            return Err(ServiceError::new(ErrorCode::SessionError, "opaque origin".into()).into());
        }
        Ok(origin.ascii_serialization())
    }

    /// Resolve the origin to use: explicit or derived from current page.
    fn resolve_origin(&self, origin: Option<&str>) -> Result<String> {
        match origin {
            Some(o) => Ok(o.to_string()),
            None => self.current_origin(),
        }
    }

    pub fn storage_get(&self, key: &str, origin: Option<&str>) -> Result<StorageGetResult> {
        let origin = self.resolve_origin(origin)?;
        let value = self.storage.get(&origin).and_then(|m| m.get(key)).cloned();
        Ok(StorageGetResult {
            key: key.to_string(),
            value,
            handle: None,
        })
    }

    pub fn storage_set(
        &mut self,
        key: &str,
        value: &str,
        origin: Option<&str>,
    ) -> Result<StorageGetResult> {
        let origin = self.resolve_origin(origin)?;
        self.storage
            .entry(origin)
            .or_default()
            .insert(key.to_string(), value.to_string());
        self.save_snapshot();
        Ok(StorageGetResult {
            key: key.to_string(),
            value: Some(value.to_string()),
            handle: None,
        })
    }

    pub fn storage_delete(&mut self, key: &str, origin: Option<&str>) -> Result<DeleteResult> {
        let origin = self.resolve_origin(origin)?;
        let deleted = self
            .storage
            .get_mut(&origin)
            .map(|m| m.remove(key).is_some())
            .unwrap_or(false);
        if deleted {
            self.save_snapshot();
        }
        Ok(DeleteResult { deleted })
    }

    pub fn storage_list(&self, origin: Option<&str>) -> Result<StorageListResult> {
        let origin = self.resolve_origin(origin)?;
        let entries = self.storage.get(&origin).cloned().unwrap_or_default();
        let count = entries.len();
        Ok(StorageListResult {
            origin,
            entries,
            count,
        })
    }

    pub fn storage_clear(&mut self, origin: Option<&str>) -> Result<StorageClearResult> {
        let origin = self.resolve_origin(origin)?;
        let cleared = self.storage.remove(&origin).map(|m| m.len()).unwrap_or(0);
        if cleared > 0 {
            self.save_snapshot();
        }
        Ok(StorageClearResult { cleared })
    }

    pub fn dom_query(&self, selector: &str, attr: &str) -> Result<DomSelectorResult> {
        let html = self.current_html()?;
        query_selector(html, selector, attr)
            .map_err(|e| ServiceError::new(ErrorCode::DomQueryError, e.to_string()).into())
    }

    pub fn dom_script(&mut self, script: &str) -> Result<DomScriptResult> {
        let page = self.current_page()?;
        let html = page.html.to_string();
        let current_url = page.result.url.clone();

        // Build script context from session state
        let origin = self.current_origin().unwrap_or_default();
        let ctx = crate::core::dom_script::ScriptContext {
            storage: self.storage.get(&origin).cloned().unwrap_or_default(),
            origin: origin.clone(),
            cookie_jar: self.http.jar().clone(),
            current_url,
            filled_fields: self.filled_fields.clone(),
        };

        let (result, side_effects) = execute_script(&html, script, Some(ctx))
            .map_err(|e| ServiceError::new(ErrorCode::DomQueryError, e.to_string()))?;

        // Merge side effects back into session
        if let Some(effects) = side_effects {
            let storage_changed = {
                let current = self.storage.get(&origin);
                current
                    .map(|c| *c != effects.storage)
                    .unwrap_or(!effects.storage.is_empty())
            };

            if !effects.storage.is_empty() {
                self.storage.insert(origin, effects.storage);
            } else {
                self.storage.remove(&origin);
            }
            self.filled_fields = effects.filled_fields;

            // If DOM was mutated, update the stored HTML
            if let Some(mutated_html) = effects.mutated_html
                && let Some(page) = self.history.get_mut(self.history_index)
            {
                page.html = mutated_html;
            }

            if storage_changed {
                self.save_snapshot();
            }
        }

        Ok(result)
    }

    // --- Redaction convenience methods ---

    /// Redact a PageResult in place, returning the modified result.
    pub fn redact_page(&mut self, page: &mut PageResult) {
        self.redaction.redact_page_result(page, &mut self.secrets);
    }

    /// Redact a CookiesResult in place.
    pub fn redact_cookies(&mut self, result: &mut CookiesResult) {
        self.redaction
            .redact_cookies_result(result, &mut self.secrets);
    }

    /// Redact a StorageGetResult in place.
    pub fn redact_storage_get_result(&mut self, result: &mut StorageGetResult) {
        self.redaction.redact_storage_get(result, &mut self.secrets);
    }

    /// Redact a StorageListResult in place.
    pub fn redact_storage_list_result(&mut self, result: &mut StorageListResult) {
        self.redaction
            .redact_storage_list(result, &mut self.secrets);
    }

    /// Resolve secret handles in a string (for incoming commands).
    pub fn resolve_secrets(&self, text: &str) -> String {
        self.secrets.resolve(text)
    }

    /// Returns raw HTML for DOM queries.
    pub fn current_html(&self) -> Result<&str> {
        Ok(&self.current_page()?.html)
    }

    #[allow(dead_code)]
    pub fn current_url(&self) -> Option<&str> {
        self.current_page().ok().map(|p| p.result.url.as_str())
    }

    /// Returns the PageResult for the current page (for MCP resources).
    pub fn current_page_result(&self) -> Result<&PageResult> {
        Ok(&self.current_page()?.result)
    }

    // --- Internal helpers ---

    fn current_page(&self) -> Result<&PageState> {
        if self.history.is_empty() {
            return Err(ServiceError::new(ErrorCode::NoPage, "no page loaded".into()).into());
        }
        Ok(&self.history[self.history_index])
    }

    fn resolve_link_by_index(&self, index: usize) -> Result<String> {
        let current = self.current_page()?;
        let link = current.links.get(index).ok_or_else(|| {
            ServiceError::new(
                ErrorCode::LinkNotFound,
                format!(
                    "link index {index} not found (page has {} links)",
                    current.links.len()
                ),
            )
        })?;
        resolve_url(&current.result.url, &link.href)
    }

    fn resolve_link_by_text(&self, text: &str) -> Result<String> {
        let current = self.current_page()?;
        let text_lower = text.to_ascii_lowercase();
        let link = current
            .links
            .iter()
            .find(|l| l.text.to_ascii_lowercase().contains(&text_lower))
            .ok_or_else(|| {
                ServiceError::new(
                    ErrorCode::LinkNotFound,
                    format!("no link with text matching '{text}'"),
                )
            })?;
        resolve_url(&current.result.url, &link.href)
    }

    fn apply_auth_profile(
        &self,
        profile_name: &str,
        url: &str,
        headers: &mut HashMap<String, String>,
    ) -> Result<()> {
        auth::apply_auth_profile(&self.config.auth, profile_name, url, headers)
    }
}

/// Resolve a (possibly relative) href against a base URL for navigation.
/// Unlike `resolve_href` (which shortens for display), this always returns
/// an absolute URL suitable for HTTP requests.
fn resolve_url(base: &str, href: &str) -> Result<String> {
    let base_url: reqwest::Url = base.parse().context("invalid base URL")?;
    Ok(base_url.join(href).context("invalid href")?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{AuthProfileConfig, StorageConfig};

    fn test_config() -> Config {
        Config::default()
    }

    fn config_with_storage() -> Config {
        Config {
            storage: vec![
                StorageConfig {
                    origin: "https://app.example.com".into(),
                    key: "theme".into(),
                    value: Some("dark".into()),
                    value_env: None,
                    sensitive: false,
                    resolved_value: Some("dark".into()),
                },
                StorageConfig {
                    origin: "https://app.example.com".into(),
                    key: "lang".into(),
                    value: Some("en".into()),
                    value_env: None,
                    sensitive: false,
                    resolved_value: Some("en".into()),
                },
                StorageConfig {
                    origin: "https://other.example.com".into(),
                    key: "mode".into(),
                    value: Some("compact".into()),
                    value_env: None,
                    sensitive: false,
                    resolved_value: Some("compact".into()),
                },
            ],
            ..Config::default()
        }
    }

    fn config_with_auth() -> Config {
        let mut auth = HashMap::new();
        auth.insert(
            "github".into(),
            AuthProfileConfig {
                header: "Authorization".into(),
                value: Some("Bearer ghp_test".into()),
                value_env: None,
                value_prefix: None,
                domains: vec!["api.github.com".into(), "*.github.com".into()],
                resolved_value: Some("Bearer ghp_test".into()),
            },
        );
        Config {
            auth,
            ..Config::default()
        }
    }

    // --- Unit tests (no networking) ---

    #[tokio::test]
    async fn test_service_construction() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        assert!(service.history.is_empty());
        assert_eq!(service.history_index, 0);
        assert!(service.storage.is_empty());
    }

    #[tokio::test]
    async fn test_service_storage_init() {
        let service = BrowserService::new(
            config_with_storage(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();

        let app_storage = service.storage.get("https://app.example.com").unwrap();
        assert_eq!(app_storage.get("theme").unwrap(), "dark");
        assert_eq!(app_storage.get("lang").unwrap(), "en");

        let other_storage = service.storage.get("https://other.example.com").unwrap();
        assert_eq!(other_storage.get("mode").unwrap(), "compact");

        assert_eq!(service.storage.len(), 2);
    }

    #[tokio::test]
    async fn test_service_info_empty() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let info = service.info();
        assert!(info.alive);
        assert_eq!(info.current_url, None);
        assert_eq!(info.title, None);
        assert_eq!(info.history_length, 0);
        assert_eq!(info.history_index, 0);
        assert!(info.uptime_secs < 2);
    }

    // --- Cookie management tests ---

    #[tokio::test]
    async fn test_cookies_empty() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let result = service.cookies(None).unwrap();
        assert_eq!(result.count, 0);
        assert!(result.cookies.is_empty());
    }

    #[tokio::test]
    async fn test_set_cookie_and_list() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();

        let result = service
            .set_cookie("session", "abc123", "example.com", "/", false, false, None)
            .unwrap();
        assert_eq!(result.name, "session");
        assert_eq!(result.domain, "example.com");

        let cookies = service.cookies(None).unwrap();
        assert_eq!(cookies.count, 1);
        assert_eq!(cookies.cookies[0].name, "session");
        assert_eq!(cookies.cookies[0].value, "abc123");
    }

    #[tokio::test]
    async fn test_set_cookie_with_flags() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();

        service
            .set_cookie(
                "token",
                "xyz",
                "secure.example.com",
                "/api",
                true,
                true,
                Some(3600),
            )
            .unwrap();

        let cookies = service.cookies(None).unwrap();
        assert_eq!(cookies.count, 1);
        let c = &cookies.cookies[0];
        assert_eq!(c.name, "token");
        assert!(c.secure);
        assert!(c.http_only);
        assert!(c.expires.is_some());
    }

    #[tokio::test]
    async fn test_cookies_domain_filter() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();

        service
            .set_cookie("a", "1", "example.com", "/", false, false, None)
            .unwrap();
        service
            .set_cookie("b", "2", "other.com", "/", false, false, None)
            .unwrap();

        let all = service.cookies(None).unwrap();
        assert_eq!(all.count, 2);

        let filtered = service.cookies(Some("example.com")).unwrap();
        assert_eq!(filtered.count, 1);
        assert_eq!(filtered.cookies[0].name, "a");
    }

    #[tokio::test]
    async fn test_delete_cookie() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();

        service
            .set_cookie("session", "abc", "example.com", "/", false, false, None)
            .unwrap();
        assert_eq!(service.cookies(None).unwrap().count, 1);

        let result = service.delete_cookie("session", "example.com").unwrap();
        assert!(result.deleted);
        assert_eq!(service.cookies(None).unwrap().count, 0);
    }

    #[tokio::test]
    async fn test_delete_cookie_nonexistent() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let result = service.delete_cookie("nope", "example.com").unwrap();
        assert!(!result.deleted);
    }

    #[tokio::test]
    async fn test_info_cookies_count() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        assert_eq!(service.info().cookies_count, 0);

        service
            .set_cookie("a", "1", "example.com", "/", false, false, None)
            .unwrap();
        assert_eq!(service.info().cookies_count, 1);

        service
            .set_cookie("b", "2", "other.com", "/", false, false, None)
            .unwrap();
        assert_eq!(service.info().cookies_count, 2);
    }

    #[tokio::test]
    async fn test_preloaded_cookies_visible() {
        let config = Config {
            cookies: vec![crate::core::config::CookieConfig {
                name: "preloaded".into(),
                value: None,
                value_env: None,
                domain: "example.com".into(),
                path: "/".into(),
                secure: false,
                http_only: false,
                sensitive: false,
                resolved_value: Some("val".into()),
            }],
            ..Config::default()
        };
        let service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        let cookies = service.cookies(None).unwrap();
        assert_eq!(cookies.count, 1);
        assert_eq!(cookies.cookies[0].name, "preloaded");
        assert_eq!(cookies.cookies[0].value, "val");
        assert_eq!(service.info().cookies_count, 1);
    }

    #[tokio::test]
    async fn test_service_links_no_page() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let err = service.links().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoPage);
    }

    #[tokio::test]
    async fn test_service_back_no_history() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let err = service.back().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoHistory);
    }

    #[tokio::test]
    async fn test_service_forward_no_history() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let err = service.forward().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoHistory);
    }

    #[tokio::test]
    async fn test_service_current_html_no_page() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let err = service.current_html().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoPage);
    }

    #[tokio::test]
    async fn test_service_current_url_empty() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        assert_eq!(service.current_url(), None);
    }

    #[test]
    fn test_resolve_url_absolute() {
        let result = resolve_url("https://example.com/page", "https://other.com/foo").unwrap();
        assert_eq!(result, "https://other.com/foo");
    }

    #[test]
    fn test_resolve_url_relative() {
        let result = resolve_url("https://example.com/dir/page", "other.html").unwrap();
        assert_eq!(result, "https://example.com/dir/other.html");
    }

    #[test]
    fn test_resolve_url_root_relative() {
        let result = resolve_url("https://example.com/dir/page", "/root.html").unwrap();
        assert_eq!(result, "https://example.com/root.html");
    }

    #[test]
    fn test_resolve_url_protocol_relative() {
        let result = resolve_url("https://example.com/page", "//other.com/foo").unwrap();
        assert_eq!(result, "https://other.com/foo");
    }

    #[test]
    fn test_resolve_url_parent_relative() {
        let result = resolve_url("https://example.com/a/b/page", "../c.html").unwrap();
        assert_eq!(result, "https://example.com/a/c.html");
    }

    #[test]
    fn test_resolve_url_fragment() {
        let result = resolve_url("https://example.com/page", "#section").unwrap();
        assert_eq!(result, "https://example.com/page#section");
    }

    #[tokio::test]
    async fn test_merge_options_applies_session_max_tokens() {
        let config = Config {
            session: crate::core::config::SessionConfig {
                defaults: crate::core::config::SessionDefaults {
                    max_tokens: Some(8000),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Config::default()
        };
        let service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        // Options without max_tokens → session default applied
        let opts = FetchOptions::default();
        let merged = service.merge_options(&opts);
        assert_eq!(merged.max_tokens, Some(8000));

        // Options with explicit max_tokens → preserved
        let opts = FetchOptions {
            max_tokens: Some(2000),
            ..Default::default()
        };
        let merged = service.merge_options(&opts);
        assert_eq!(merged.max_tokens, Some(2000));
    }

    #[tokio::test]
    async fn test_default_fetch_options_from_config() {
        let config = Config {
            session: crate::core::config::SessionConfig {
                timeout_secs: 60,
                defaults: crate::core::config::SessionDefaults {
                    max_tokens: Some(4000),
                    strip_nav: false,
                    include_links: false,
                    include_images: true,
                },
                ..Default::default()
            },
            ..Config::default()
        };
        let service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();
        let opts = service.default_fetch_options();

        assert_eq!(opts.max_tokens, Some(4000));
        assert!(!opts.strip_nav);
        assert!(!opts.include_links);
        assert!(opts.include_images);
        assert_eq!(opts.timeout_secs, 60);
    }

    #[tokio::test]
    async fn test_search_url_default_engine() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let url = service.search_url("rust programming");
        assert_eq!(url, "https://html.duckduckgo.com/html/?q=rust+programming");
    }

    #[tokio::test]
    async fn test_search_url_custom_engine() {
        let config = Config {
            search: crate::core::config::SearchConfig {
                engine: "https://www.google.com/search?q={}".into(),
            },
            ..Config::default()
        };
        let service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();
        let url = service.search_url("hello world");
        assert_eq!(url, "https://www.google.com/search?q=hello+world");
    }

    #[tokio::test]
    async fn test_search_url_special_chars() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let url = service.search_url("what is 2+2?");
        assert_eq!(url, "https://html.duckduckgo.com/html/?q=what+is+2%2B2%3F");
    }

    #[tokio::test]
    async fn test_auth_profile_not_found() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let mut headers = HashMap::new();
        let err = service
            .apply_auth_profile("nonexistent", "https://example.com", &mut headers)
            .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::AuthProfileNotFound);
    }

    #[tokio::test]
    async fn test_auth_profile_domain_mismatch() {
        let service = BrowserService::new(
            config_with_auth(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let mut headers = HashMap::new();
        let err = service
            .apply_auth_profile("github", "https://evil.com/steal", &mut headers)
            .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::AuthProfileDomainMismatch);
    }

    #[tokio::test]
    async fn test_auth_profile_domain_match() {
        let service = BrowserService::new(
            config_with_auth(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let mut headers = HashMap::new();
        service
            .apply_auth_profile("github", "https://api.github.com/repos", &mut headers)
            .unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer ghp_test");
    }

    #[tokio::test]
    async fn test_auth_profile_wildcard_match() {
        let service = BrowserService::new(
            config_with_auth(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let mut headers = HashMap::new();
        service
            .apply_auth_profile("github", "https://raw.github.com/file", &mut headers)
            .unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer ghp_test");
    }

    /// Push a synthetic page into history for unit-testing without networking.
    fn inject_page(service: &mut BrowserService, url: &str, html: &str) {
        let parsed = ParsedHtml::parse(html);
        let title = parsed.title();
        let links = parsed.links(Some(url));
        let convert = parsed.to_markdown_with_options(&FetchOptions::default(), Some(url));
        let result = PageResult {
            url: url.to_string(),
            title,
            status: 200,
            markdown: convert.markdown,
            links: Some(links.clone()),
            meta: parsed.metadata(),
            stats: PageStats {
                fetch_ms: 0,
                tokens_est: convert.tokens_est,
                content_bytes: html.len() as u64,
            },
            truncated: false,
            next_offset: None,
            content_selectors: None,
        };
        let state = PageState {
            html: html.to_string(),
            links,
            result,
        };
        if !service.history.is_empty() {
            service.history.truncate(service.history_index + 1);
        }
        service.history.push(state);
        service.history_index = service.history.len() - 1;
    }

    const FORM_HTML: &str = r#"
    <html>
    <body>
        <form id="login" action="/login" method="POST">
            <input type="text" name="username" value="">
            <input type="password" name="password" value="">
            <input type="hidden" name="csrf" value="tok123">
            <input type="submit" value="Log In">
        </form>
    </body>
    </html>
    "#;

    #[tokio::test]
    async fn test_fill_no_page() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let err = service
            .fill(&[("#username".into(), "agent".into())])
            .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoPage);
    }

    #[tokio::test]
    async fn test_fill_single_field() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://example.com/form", FORM_HTML);

        let result = service
            .fill(&[("input[name='username']".into(), "agent@test.com".into())])
            .unwrap();
        assert_eq!(result.filled, 1);
        assert_eq!(
            service.filled_fields.get("input[name='username']").unwrap(),
            "agent@test.com"
        );
    }

    #[tokio::test]
    async fn test_fill_multiple_fields() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://example.com/form", FORM_HTML);

        let result = service
            .fill(&[
                ("input[name='username']".into(), "agent@test.com".into()),
                ("input[name='password']".into(), "s3cret".into()),
            ])
            .unwrap();
        assert_eq!(result.filled, 2);
    }

    #[tokio::test]
    async fn test_fill_invalid_selector() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://example.com/form", FORM_HTML);

        let err = service
            .fill(&[("input[name='nonexistent']".into(), "value".into())])
            .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::SelectorNotFound);
    }

    #[tokio::test]
    async fn test_fill_not_a_field() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://example.com/form", FORM_HTML);

        let err = service
            .fill(&[("form#login".into(), "value".into())])
            .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::FormNotFound);
    }

    #[tokio::test]
    async fn test_fill_clears_on_overwrite() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://example.com/form", FORM_HTML);

        service
            .fill(&[("input[name='username']".into(), "first".into())])
            .unwrap();
        service
            .fill(&[("input[name='username']".into(), "second".into())])
            .unwrap();
        assert_eq!(
            service.filled_fields.get("input[name='username']").unwrap(),
            "second"
        );
    }

    // --- Integration tests (require networking) ---

    #[tokio::test]
    #[ignore]
    async fn test_fetch_returns_page_result() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let opts = FetchOptions::default();
        let result = service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();

        assert_eq!(result.status, 200);
        assert!(result.title.is_some());
        assert!(!result.markdown.is_empty());
        assert!(result.stats.fetch_ms > 0);
        assert!(result.stats.content_bytes > 0);

        // History updated
        assert_eq!(service.history.len(), 1);
        assert_eq!(service.history_index, 0);
        assert!(service.current_url().unwrap().contains("example.com"));

        // Info reflects current state
        let info = service.info();
        assert!(info.alive);
        assert!(info.current_url.unwrap().contains("example.com"));
        assert_eq!(info.history_length, 1);
        assert_eq!(info.history_index, 0);
    }

    #[tokio::test]
    #[ignore]
    async fn test_fetch_links_fetch_index_back_forward() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let opts = FetchOptions::default();

        // Step 1: Fetch a page
        let result = service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        assert_eq!(result.status, 200);
        let first_url = result.url.clone();

        // Step 2: Get links
        let links = service.links().unwrap();
        assert!(links.count > 0);

        // Step 3: Follow first link by index
        let result2 = service
            .fetch_by_index(0, &HttpMethod::Get, &HashMap::new(), None, None, &opts)
            .await
            .unwrap();
        assert_eq!(result2.status, 200);
        assert_ne!(result2.url, first_url);
        assert_eq!(service.history.len(), 2);
        assert_eq!(service.history_index, 1);

        // Step 4: Go back
        let back_result = service.back().unwrap();
        assert_eq!(back_result.url, first_url);
        assert_eq!(service.history_index, 0);

        // Step 5: Go forward
        let fwd_result = service.forward().unwrap();
        assert_eq!(fwd_result.url, result2.url);
        assert_eq!(service.history_index, 1);

        // Step 6: Can't go forward further
        let err = service.forward().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoHistory);

        // Step 7: Info shows correct state
        let info = service.info();
        assert_eq!(info.history_length, 2);
        assert_eq!(info.history_index, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_history_truncation_on_new_navigation() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let opts = FetchOptions::default();

        // Fetch 3 pages
        service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        service
            .fetch(
                "https://www.iana.org/",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        service
            .fetch(
                "https://example.org",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        assert_eq!(service.history.len(), 3);
        assert_eq!(service.history_index, 2);

        // Go back twice to index 0
        service.back().unwrap();
        service.back().unwrap();
        assert_eq!(service.history_index, 0);

        // Fetch a new page — should truncate forward history
        service
            .fetch(
                "https://httpbin.org/html",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        assert_eq!(service.history.len(), 2); // original first page + new page
        assert_eq!(service.history_index, 1);

        // Forward should fail since forward history was truncated
        let err = service.forward().unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::NoHistory);
    }

    #[tokio::test]
    #[ignore]
    async fn test_fetch_by_text() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let opts = FetchOptions::default();

        service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();

        // Use the actual first link text from the page
        let links = service.links().unwrap();
        assert!(links.count > 0);
        let link_text = &links.links[0].text;

        let result = service
            .fetch_by_text(
                link_text,
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();
        assert_eq!(result.status, 200);
    }

    #[tokio::test]
    #[ignore]
    async fn test_start_url_auto_fetch() {
        let config = Config {
            session: crate::core::config::SessionConfig {
                start_url: Some("https://example.com".into()),
                ..Default::default()
            },
            ..Config::default()
        };
        let service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        // Page should already be loaded
        assert_eq!(service.history.len(), 1);
        assert!(service.current_url().unwrap().contains("example.com"));

        let links = service.links().unwrap();
        assert!(links.count > 0);

        let info = service.info();
        assert!(info.alive);
        assert!(info.current_url.is_some());
        assert_eq!(info.history_length, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_fetch_with_pagination() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let opts = FetchOptions {
            max_tokens: Some(5),
            ..Default::default()
        };

        let result = service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
            .unwrap();

        assert!(result.truncated);
        assert!(result.next_offset.is_some());

        // Fetch next page with offset
        let opts2 = FetchOptions {
            max_tokens: Some(5),
            offset: result.next_offset.unwrap(),
            ..Default::default()
        };
        let result2 = service
            .fetch(
                "https://example.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts2,
            )
            .await
            .unwrap();

        // Should get different content
        assert_ne!(result.markdown, result2.markdown);
    }

    #[test]
    fn test_classify_service_error_passthrough() {
        let se = ServiceError::new(ErrorCode::NoPage, "no page".into());
        let err: anyhow::Error = se.into();
        let classified = classify_error(err);
        assert_eq!(classified.code, ErrorCode::NoPage);
        assert_eq!(classified.message, "no page");
    }

    #[test]
    fn test_classify_url_parse_error() {
        let err: anyhow::Error = url::ParseError::EmptyHost.into();
        let classified = classify_error(err);
        assert_eq!(classified.code, ErrorCode::InvalidUrl);
    }

    #[test]
    fn test_classify_unknown_error_fallback() {
        let err = anyhow::anyhow!("something unexpected");
        let classified = classify_error(err);
        assert_eq!(classified.code, ErrorCode::SessionError);
        assert!(classified.message.contains("something unexpected"));
    }

    #[tokio::test]
    async fn test_history_list_empty() {
        let service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        let result = service.history(None, 10);
        assert_eq!(result.count, 0);
        assert_eq!(result.total, 0);
        assert!(result.entries.is_empty());
    }

    #[tokio::test]
    async fn test_history_list_with_pages() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(
            &mut service,
            "https://example.com",
            "<html><head><title>Example</title></head><body>Hello</body></html>",
        );
        inject_page(
            &mut service,
            "https://google.com",
            "<html><head><title>Google</title></head><body>Search</body></html>",
        );
        inject_page(
            &mut service,
            "https://rust-lang.org",
            "<html><head><title>Rust</title></head><body>Lang</body></html>",
        );

        let result = service.history(None, 10);
        assert_eq!(result.count, 3);
        assert_eq!(result.total, 3);
        // Most recent first
        assert_eq!(result.entries[0].url, "https://rust-lang.org");
        assert_eq!(result.entries[1].url, "https://google.com");
        assert_eq!(result.entries[2].url, "https://example.com");
        // Current page marker
        assert!(result.entries[0].current);
        assert!(!result.entries[2].current);
    }

    #[tokio::test]
    async fn test_history_list_with_limit() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(&mut service, "https://a.com", "<html><body>A</body></html>");
        inject_page(&mut service, "https://b.com", "<html><body>B</body></html>");
        inject_page(&mut service, "https://c.com", "<html><body>C</body></html>");

        let result = service.history(None, 2);
        assert_eq!(result.count, 2);
        assert_eq!(result.total, 3);
        assert_eq!(result.entries[0].url, "https://c.com");
        assert_eq!(result.entries[1].url, "https://b.com");
    }

    #[tokio::test]
    async fn test_history_search() {
        let mut service = BrowserService::new(
            test_config(),
            Box::new(crate::core::session_store::InMemoryStore),
        )
        .await
        .unwrap();
        inject_page(
            &mut service,
            "https://example.com",
            "<html><head><title>Example</title></head><body>Hello</body></html>",
        );
        inject_page(
            &mut service,
            "https://google.com",
            "<html><head><title>Google Search</title></head><body>Search</body></html>",
        );
        inject_page(
            &mut service,
            "https://rust-lang.org",
            "<html><head><title>Rust</title></head><body>Lang</body></html>",
        );

        // Search by URL
        let result = service.history(Some("google"), 10);
        assert_eq!(result.count, 1);
        assert_eq!(result.entries[0].url, "https://google.com");

        // Search by title (case-insensitive)
        let result = service.history(Some("RUST"), 10);
        assert_eq!(result.count, 1);
        assert_eq!(result.entries[0].url, "https://rust-lang.org");

        // No matches
        let result = service.history(Some("nonexistent"), 10);
        assert_eq!(result.count, 0);
    }
}
