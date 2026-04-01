use reqwest::Url;
use scraper::{Html, Selector};
use std::sync::LazyLock;

use super::dom_script::execute_script;
use super::page::PendingNavigation;

static SEL_SCRIPT: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script").unwrap());
static SEL_META_REFRESH: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("meta[http-equiv]").unwrap());

/// Keywords that indicate a script may perform a client-side redirect.
const REDIRECT_KEYWORDS: &[&str] = &[
    "location.href",
    "location.replace",
    "location.assign",
    "location =",
    "location=",
    "window.location",
    "document.location",
];

/// Detect a client-side redirect by checking for meta-refresh tags and
/// executing inline scripts that contain redirect keywords.
/// Returns the resolved destination URL, or `None`.
pub fn detect_client_redirect(html: &str, base_url: &str) -> Option<String> {
    let doc = Html::parse_document(html);

    // 1) Check <meta http-equiv="refresh">
    for el in doc.select(&SEL_META_REFRESH) {
        let equiv = el.value().attr("http-equiv").unwrap_or_default();
        if equiv.eq_ignore_ascii_case("refresh") {
            if let Some(content) = el.value().attr("content") {
                if let Some(url) = parse_meta_refresh(content) {
                    return Some(resolve_absolute(base_url, &url));
                }
            }
        }
    }

    // 2) Collect inline scripts that contain redirect-related keywords
    let mut combined = String::new();
    for el in doc.select(&SEL_SCRIPT) {
        let text: String = el.text().collect();
        if !text.trim().is_empty() && REDIRECT_KEYWORDS.iter().any(|kw| text.contains(kw)) {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&text);
        }
    }

    if !combined.is_empty() {
        if let Ok((result, _)) = execute_script(html, &combined, None) {
            if let Some(PendingNavigation::Link { href }) = result.pending_navigation {
                return Some(resolve_absolute(base_url, &href));
            }
        }
    }

    None
}

/// Parse the `content` attribute of a meta-refresh tag.
/// Accepts formats like `"5;url=https://example.com"` or `"0; URL=..."`.
fn parse_meta_refresh(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let url_pos = lower.find("url=")?;
    let url_part = content[url_pos + 4..].trim();
    let url = url_part
        .trim_start_matches(|c| c == '\'' || c == '"')
        .trim_end_matches(|c| c == '\'' || c == '"');
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

/// Resolve a possibly-relative href against a base URL string,
/// always returning a full absolute URL.
fn resolve_absolute(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    match Url::parse(base) {
        Ok(b) => b
            .join(href)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| href.to_string()),
        Err(_) => href.to_string(),
    }
}

/// Resolve `href` against an optional base URL.
/// Returns the resolved absolute URL, or `href` unchanged on failure / no base.
pub fn resolve_href(base: Option<&Url>, href: &str) -> String {
    match base {
        Some(b) => b
            .join(href)
            .map(|u| shorten_same_origin(b, &u))
            .unwrap_or_else(|_| href.to_string()),
        None => href.to_string(),
    }
}

/// If `url` shares the same origin as `base`, return just the path+query+fragment.
/// Otherwise return the full URL.
fn shorten_same_origin(base: &Url, url: &Url) -> String {
    if base.scheme() == url.scheme() && base.host() == url.host() && base.port() == url.port() {
        let path = url.path();
        let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
        let fragment = url.fragment().map(|f| format!("#{f}")).unwrap_or_default();
        format!("{path}{query}{fragment}")
    } else {
        url.to_string()
    }
}
