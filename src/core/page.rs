use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Core domain types ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContentSelector {
    pub selector: String,
    pub tokens_est: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Link {
    pub i: usize,
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PageMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageStats {
    pub fetch_ms: u64,
    pub tokens_est: u64,
    pub content_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageResult {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: u16,
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Link>>,
    pub meta: PageMetadata,
    pub stats: PageStats,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_selectors: Option<Vec<ContentSelector>>,
}

// --- Fetch configuration ---

pub(crate) fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default)]
    pub strip_nav: bool,
    #[serde(default = "default_true")]
    pub include_links: bool,
    #[serde(default)]
    pub include_images: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub compact_links: bool,
    #[serde(default = "default_true")]
    pub show_selectors_first: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_path: Option<String>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            max_tokens: None,
            offset: 0,
            selector: None,
            strip_nav: false,
            include_links: true,
            include_images: false,
            timeout_secs: 30,
            compact_links: false,
            show_selectors_first: true,
            download_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// Resolved fetch mode after applying precedence: url > index > text.
/// Not a serde type — produced by resolving the raw optional fields.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchMode {
    Url(String),
    Index(usize),
    Text(String),
}

impl FetchMode {
    pub fn resolve(
        url: Option<&str>,
        index: Option<usize>,
        text: Option<&str>,
    ) -> Option<FetchMode> {
        if let Some(url) = url {
            Some(FetchMode::Url(url.to_owned()))
        } else if let Some(index) = index {
            Some(FetchMode::Index(index))
        } else {
            text.map(|t| FetchMode::Text(t.to_owned()))
        }
    }
}

// --- Result types for non-fetch actions ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinksResult {
    pub links: Vec<Link>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomSelectorResult {
    pub results: Vec<serde_json::Value>,
    pub count: usize,
    pub exec_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomScriptResult {
    pub result: serde_json::Value,
    #[serde(rename = "type")]
    pub result_type: String,
    pub exec_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_navigation: Option<PendingNavigation>,
}

/// A navigation action requested by JS code (form.submit(), element.click()).
/// Since JS execution is synchronous, these are deferred for the caller to execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PendingNavigation {
    #[serde(rename = "link")]
    Link { href: String },
    #[serde(rename = "form_submit")]
    FormSubmit { selector: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfoResult {
    pub alive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub history_length: usize,
    pub history_index: usize,
    pub cookies_count: usize,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntryInfo {
    pub index: usize,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: u16,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryResult {
    pub entries: Vec<HistoryEntryInfo>,
    pub count: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookiesResult {
    pub cookies: Vec<CookieInfo>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetCookieResult {
    pub name: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FillResult {
    pub filled: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteResult {
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageGetResult {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageListResult {
    pub origin: String,
    pub entries: HashMap<String, String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageClearResult {
    pub cleared: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetch_options_default() {
        let opts = FetchOptions::default();
        assert_eq!(opts.max_tokens, None);
        assert_eq!(opts.offset, 0);
        assert_eq!(opts.selector, None);
        assert!(!opts.strip_nav);
        assert!(opts.include_links);
        assert!(!opts.include_images);
        assert_eq!(opts.timeout_secs, 30);
    }

    #[test]
    fn test_fetch_options_partial_json() {
        let json = r#"{"max_tokens": 2000}"#;
        let opts: FetchOptions = serde_json::from_str(json).unwrap();
        assert_eq!(opts.max_tokens, Some(2000));
        assert_eq!(opts.offset, 0);
        assert_eq!(opts.selector, None);
        assert!(!opts.strip_nav);
        assert!(opts.include_links);
        assert!(!opts.include_images);
        assert_eq!(opts.timeout_secs, 30);
    }

    #[test]
    fn test_fetch_options_roundtrip() {
        let opts = FetchOptions {
            max_tokens: Some(4000),
            offset: 100,
            selector: Some("article".into()),
            strip_nav: false,
            include_links: false,
            include_images: true,
            timeout_secs: 60,
            compact_links: false,
            show_selectors_first: true,
            download_path: None,
        };
        let json = serde_json::to_string(&opts).unwrap();
        let back: FetchOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, opts);
    }

    #[test]
    fn test_page_result_roundtrip() {
        let result = PageResult {
            url: "https://example.com".into(),
            title: Some("Example Domain".into()),
            status: 200,
            markdown: "# Example Domain\n\nThis domain is for use in illustrative examples...".into(),
            links: Some(vec![Link {
                i: 0,
                text: "More information".into(),
                href: "https://www.iana.org/domains/example".into(),
            }]),
            meta: PageMetadata {
                lang: Some("en".into()),
                description: Some("Example domain for documentation".into()),
                content_type: Some("text/html".into()),
            },
            stats: PageStats {
                fetch_ms: 230,
                tokens_est: 42,
                content_bytes: 1256,
            },
            truncated: false,
            next_offset: None,
            content_selectors: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: PageResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn test_page_result_from_spec() {
        let json = r##"{
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
        }"##;
        let result: PageResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.title, Some("Example Domain".into()));
        assert_eq!(result.status, 200);
        assert_eq!(result.links.as_ref().unwrap().len(), 1);
        assert_eq!(result.links.as_ref().unwrap()[0].i, 0);
        assert_eq!(result.meta.lang, Some("en".into()));
        assert_eq!(result.stats.fetch_ms, 230);
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);
    }

    #[test]
    fn test_page_result_truncated() {
        let result = PageResult {
            url: "https://example.com".into(),
            title: None,
            status: 200,
            markdown: "partial content...".into(),
            links: None,
            meta: PageMetadata::default(),
            stats: PageStats {
                fetch_ms: 100,
                tokens_est: 4000,
                content_bytes: 50000,
            },
            truncated: true,
            next_offset: Some(4000),
            content_selectors: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: PageResult = serde_json::from_str(&json).unwrap();
        assert!(back.truncated);
        assert_eq!(back.next_offset, Some(4000));
    }

    #[test]
    fn test_link_serialization() {
        let link = Link {
            i: 0,
            text: "More info".into(),
            href: "https://example.com".into(),
        };
        let json = serde_json::to_string(&link).unwrap();
        let expected: serde_json::Value = serde_json::json!({
            "i": 0,
            "text": "More info",
            "href": "https://example.com"
        });
        let actual: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_fetch_mode_precedence_url_wins() {
        let mode = FetchMode::resolve(
            Some("https://example.com"),
            Some(3),
            Some("click me"),
        );
        assert_eq!(mode, Some(FetchMode::Url("https://example.com".into())));
    }

    #[test]
    fn test_fetch_mode_precedence_index_wins() {
        let mode = FetchMode::resolve(None, Some(3), Some("click me"));
        assert_eq!(mode, Some(FetchMode::Index(3)));
    }

    #[test]
    fn test_fetch_mode_precedence_text() {
        let mode = FetchMode::resolve(None, None, Some("click me"));
        assert_eq!(mode, Some(FetchMode::Text("click me".into())));
    }

    #[test]
    fn test_fetch_mode_none() {
        let mode = FetchMode::resolve(None, None, None);
        assert_eq!(mode, None);
    }

    #[test]
    fn test_http_method_serialization() {
        assert_eq!(serde_json::to_string(&HttpMethod::Post).unwrap(), "\"POST\"");
        assert_eq!(serde_json::to_string(&HttpMethod::Get).unwrap(), "\"GET\"");

        let method: HttpMethod = serde_json::from_str("\"GET\"").unwrap();
        assert_eq!(method, HttpMethod::Get);

        let method: HttpMethod = serde_json::from_str("\"DELETE\"").unwrap();
        assert_eq!(method, HttpMethod::Delete);
    }

    #[test]
    fn test_http_method_default() {
        assert_eq!(HttpMethod::default(), HttpMethod::Get);
    }

    #[test]
    fn test_info_result_roundtrip() {
        let info = InfoResult {
            alive: true,
            current_url: Some("https://example.com".into()),
            title: Some("Example".into()),
            history_length: 3,
            history_index: 1,
            cookies_count: 5,
            uptime_secs: 120,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: InfoResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn test_cookies_result_roundtrip() {
        let result = CookiesResult {
            cookies: vec![CookieInfo {
                name: "session".into(),
                value: "abc123".into(),
                domain: "example.com".into(),
                path: "/".into(),
                secure: true,
                http_only: true,
                expires: Some("2026-12-31T23:59:59Z".into()),
                handle: None,
            }],
            count: 1,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: CookiesResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
    }
}
