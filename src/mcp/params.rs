use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::core::page::{default_true, HttpMethod};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FetchParams {
    /// URL to fetch
    pub url: String,

    /// HTTP method (GET, POST, PUT, PATCH, DELETE). Defaults to GET.
    #[serde(default)]
    pub method: HttpMethod,

    /// Request body (for POST/PUT/PATCH)
    pub body: Option<String>,

    /// Auth profile name from config to attach credentials
    pub auth_profile: Option<String>,

    /// Additional HTTP headers as key-value pairs
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,

    /// Maximum tokens to return. Enables pagination if content exceeds limit.
    pub max_tokens: Option<u64>,

    /// CSS selector to extract specific content from the page
    pub selector: Option<String>,

    /// Byte offset for pagination. Use next_offset from a previous truncated response.
    pub offset: Option<u64>,

    /// When true (default), returns available content selectors instead of full page content.
    /// Re-fetch with a chosen selector for targeted content. Set to false to get the raw page.
    #[serde(default = "default_true")]
    pub show_selectors_first: bool,

    /// File path to save binary content (images, PDFs, etc.) to disk.
    /// When set, binary responses are written to this path instead of returned inline.
    pub download_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// Link index number (from browser39_links output)
    pub index: Option<usize>,

    /// Link text to match (substring match, case-insensitive)
    pub text: Option<String>,

    /// Maximum tokens to return
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DomQueryParams {
    /// CSS selector to query (returns matching elements)
    pub selector: Option<String>,

    /// JavaScript to execute against the page DOM
    pub script: Option<String>,

    /// Attribute to extract from matched elements (default: textContent).
    /// Options: textContent, innerHTML, href, src, or any HTML attribute.
    pub attr: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FillFieldParam {
    /// CSS selector for the form field
    pub selector: String,
    /// Value to fill
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FillParams {
    /// CSS selector for a single field (use with value)
    pub selector: Option<String>,

    /// Value for the single field (use with selector)
    pub value: Option<String>,

    /// Array of fields to fill (alternative to selector+value)
    pub fields: Option<Vec<FillFieldParam>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitParams {
    /// CSS selector for the form element to submit
    pub selector: String,

    /// Maximum tokens to return in the response page
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CookiesParams {
    /// Filter cookies by domain
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetCookieParams {
    /// Cookie name
    pub name: String,

    /// Cookie value
    pub value: String,

    /// Cookie domain
    pub domain: String,

    /// Cookie path (defaults to "/")
    pub path: Option<String>,

    /// Whether the cookie requires HTTPS
    #[serde(default)]
    pub secure: bool,

    /// Whether the cookie is HTTP-only (not accessible to JavaScript)
    #[serde(default)]
    pub http_only: bool,

    /// Cookie expiration in seconds from now
    pub max_age_secs: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteCookieParams {
    /// Cookie name to delete
    pub name: String,

    /// Domain of the cookie to delete
    pub domain: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StorageGetParams {
    /// Storage key to retrieve
    pub key: String,

    /// Origin (scheme://host:port). Defaults to current page origin.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StorageSetParams {
    /// Storage key
    pub key: String,

    /// Value to store
    pub value: String,

    /// Origin (scheme://host:port). Defaults to current page origin.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StorageDeleteParams {
    /// Storage key to delete
    pub key: String,

    /// Origin (scheme://host:port). Defaults to current page origin.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StorageListParams {
    /// Origin (scheme://host:port). Defaults to current page origin.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StorageClearParams {
    /// Origin (scheme://host:port). Defaults to current page origin.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryParams {
    /// Text to search for in URLs and titles. If omitted, lists recent entries.
    pub query: Option<String>,
    /// Maximum number of entries to return (default: 10).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query string
    pub query: String,

    /// Maximum tokens to return
    pub max_tokens: Option<u64>,
}
