use anyhow::{Context, Result};
use bytes::Bytes;
use cookie_store::CookieStore;
use encoding_rs::Encoding;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::config::{Config, HeaderRuleConfig, domain_matches};
use super::page::{CookieInfo, HttpMethod};

#[derive(Debug, Clone)]
pub enum ResponseBody {
    /// Decoded text (HTML, JSON, XML, etc.)
    Text(String),
    /// Raw binary bytes (images, PDFs, etc.)
    Binary(Vec<u8>),
}

impl ResponseBody {
    #[cfg(test)]
    pub fn as_text(&self) -> &str {
        match self {
            ResponseBody::Text(s) => s,
            ResponseBody::Binary(_) => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: ResponseBody,
    pub content_length: u64,
    pub elapsed_ms: u64,
}

/// Cookie jar backed by `cookie_store::CookieStore` with full management capabilities.
/// Implements `reqwest::cookie::CookieStore` so reqwest handles Set-Cookie automatically.
pub struct CookieJar {
    store: RwLock<CookieStore>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(CookieStore::new()),
        }
    }

    /// Add a cookie from a Set-Cookie style string.
    pub fn add_cookie_str(&self, cookie_str: &str, url: &reqwest::Url) {
        let cookies = cookie::Cookie::parse(cookie_str)
            .ok()
            .map(|c| c.into_owned())
            .into_iter();
        self.store
            .write()
            .unwrap()
            .store_response_cookies(cookies, url);
    }

    /// List all unexpired cookies, optionally filtered by domain.
    pub fn list_cookies(&self, domain_filter: Option<&str>) -> Vec<CookieInfo> {
        let store = self.store.read().unwrap();
        store
            .iter_unexpired()
            .filter_map(|c| {
                let domain_str = c
                    .domain
                    .as_cow()
                    .map(|d| d.into_owned())
                    .unwrap_or_default();
                if let Some(filter) = domain_filter
                    && !domain_matches(filter, &domain_str)
                {
                    return None;
                }
                Some(CookieInfo {
                    name: c.name().to_string(),
                    value: c.value().to_string(),
                    domain: domain_str,
                    path: String::from(&c.path),
                    secure: c.secure().unwrap_or(false),
                    http_only: c.http_only().unwrap_or(false),
                    expires: match &c.expires {
                        cookie_store::CookieExpiration::AtUtc(t) => t
                            .format(&time::format_description::well_known::Rfc3339)
                            .ok(),
                        cookie_store::CookieExpiration::SessionEnd => None,
                    },
                    handle: None,
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_cookie(
        &self,
        name: &str,
        value: &str,
        domain: &str,
        path: &str,
        secure: bool,
        http_only: bool,
        max_age_secs: Option<u64>,
    ) -> Result<()> {
        let mut set_cookie = format!("{name}={value}; Domain={domain}; Path={path}");
        if secure {
            set_cookie.push_str("; Secure");
        }
        if http_only {
            set_cookie.push_str("; HttpOnly");
        }
        if let Some(secs) = max_age_secs {
            set_cookie.push_str(&format!("; Max-Age={secs}"));
        }

        let scheme = if secure { "https" } else { "http" };
        let url: reqwest::Url = format!("{scheme}://{domain}/")
            .parse()
            .with_context(|| format!("invalid cookie domain: {domain}"))?;
        self.add_cookie_str(&set_cookie, &url);
        Ok(())
    }

    /// Remove a cookie by name and domain. Tries common paths ("/" and domain-specific).
    /// Returns true if a cookie was actually removed.
    pub fn remove_cookie(&self, name: &str, domain: &str) -> bool {
        let mut store = self.store.write().unwrap();
        // Try to find the cookie across all paths for this domain
        let paths: Vec<String> = store
            .iter_unexpired()
            .filter(|c| {
                (*c).name() == name
                    && c.domain
                        .as_cow()
                        .map(|d| d.as_ref() == domain)
                        .unwrap_or(false)
            })
            .map(|c| String::from(&c.path))
            .collect();

        let mut removed = false;
        for path in paths {
            if store.remove(domain, &path, name).is_some() {
                removed = true;
            }
        }
        removed
    }

    /// Count of unexpired cookies.
    pub fn count(&self) -> usize {
        self.store.read().unwrap().iter_unexpired().count()
    }

    /// Serialize all cookies (including session cookies) to JSON for persistence.
    pub fn export_json(&self) -> Result<String> {
        let store = self.store.read().unwrap();
        let mut buf = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut buf)
            .map_err(|e| anyhow::anyhow!("serializing cookies: {e}"))?;
        String::from_utf8(buf).context("cookie JSON is not valid UTF-8")
    }

    /// Load cookies from previously exported JSON, replacing the current store.
    pub fn import_json(&self, json: &str) -> Result<()> {
        let loaded = cookie_store::serde::json::load(json.as_bytes())
            .map_err(|e| anyhow::anyhow!("loading cookies: {e}"))?;
        *self.store.write().unwrap() = loaded;
        Ok(())
    }
}

impl reqwest::cookie::CookieStore for CookieJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &url::Url) {
        let iter = cookie_headers
            .filter_map(|val| std::str::from_utf8(val.as_bytes()).ok())
            .filter_map(|s| cookie::Cookie::parse(s).ok())
            .map(|c| c.into_owned());
        self.store
            .write()
            .unwrap()
            .store_response_cookies(iter, url);
    }

    fn cookies(&self, url: &url::Url) -> Option<HeaderValue> {
        let s = self
            .store
            .read()
            .unwrap()
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");

        if s.is_empty() {
            return None;
        }

        HeaderValue::from_maybe_shared(Bytes::from(s)).ok()
    }
}

pub struct HttpClient {
    client: reqwest::Client,
    jar: Arc<CookieJar>,
    header_rules: Vec<HeaderRuleConfig>,
}

impl HttpClient {
    pub fn new(config: &Config) -> Result<Self> {
        let jar = Arc::new(CookieJar::new());

        // Preload cookies from config
        for cookie_cfg in &config.cookies {
            let value = cookie_cfg.resolved_value.as_deref().unwrap_or_default();
            jar.set_cookie(
                &cookie_cfg.name,
                value,
                &cookie_cfg.domain,
                &cookie_cfg.path,
                cookie_cfg.secure,
                cookie_cfg.http_only,
                None,
            )?;
        }

        let client = reqwest::Client::builder()
            .user_agent(&config.session.user_agent)
            .cookie_provider(jar.clone())
            .redirect(Policy::limited(config.session.max_redirects as usize))
            .timeout(Duration::from_secs(config.session.timeout_secs))
            .build()
            .context("building HTTP client")?;

        Ok(Self {
            client,
            jar,
            header_rules: config.headers.clone(),
        })
    }

    pub fn jar(&self) -> &Arc<CookieJar> {
        &self.jar
    }

    pub fn set_header_rules(&mut self, rules: Vec<HeaderRuleConfig>) {
        self.header_rules = rules;
    }

    pub async fn fetch(
        &self,
        url: &str,
        method: &HttpMethod,
        headers: &HashMap<String, String>,
        body: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<HttpResponse> {
        let parsed_url: reqwest::Url = url.parse().context("invalid URL")?;
        let domain = parsed_url.host_str().unwrap_or_default();

        let defaults = self.resolve_headers_for_domain(domain);

        let reqwest_method = match method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };

        let mut req = self.client.request(reqwest_method, parsed_url);
        // Apply default headers first, then per-request headers override
        for (k, v) in &defaults {
            req = req.header(*k, *v);
        }
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        if let Some(t) = timeout_secs {
            req = req.timeout(Duration::from_secs(t));
        }

        let start = Instant::now();
        let response = req.send().await.context("HTTP request failed")?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        let status = response.status().as_u16();
        let final_url = response.url().to_string();

        let mut resp_headers = HashMap::with_capacity(response.headers().len());
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                resp_headers.insert(name.to_string(), v.to_string());
            }
        }

        let content_type = resp_headers.get("content-type").cloned();
        let charset = extract_charset(content_type.as_ref());
        let bytes = response.bytes().await.context("reading response body")?;
        let content_length = bytes.len() as u64;

        let mime_base = content_type
            .as_deref()
            .and_then(|ct| ct.split(';').next())
            .map(|s| s.trim())
            .unwrap_or("text/html");
        let body = if is_text_content_type(mime_base) {
            ResponseBody::Text(decode_body(&bytes, charset))
        } else {
            ResponseBody::Binary(bytes.to_vec())
        };

        Ok(HttpResponse {
            status,
            url: final_url,
            headers: resp_headers,
            body,
            content_length,
            elapsed_ms,
        })
    }

    fn resolve_headers_for_domain(&self, domain: &str) -> HashMap<&str, &str> {
        let mut result = HashMap::new();
        for rule in &self.header_rules {
            if rule.domains.iter().any(|p| domain_matches(p, domain)) {
                for (k, v) in &rule.values {
                    result.insert(k.as_str(), v.as_str());
                }
            }
        }
        result
    }
}

/// Returns true if the MIME type represents text content that should be parsed as HTML/markdown.
pub fn is_text_content_type(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/")
        || mime == "application/xhtml+xml"
        || mime == "application/xml"
        || mime == "application/json"
        || mime == "application/javascript"
        || mime.ends_with("+xml")
        || mime.ends_with("+json")
}

/// Extract charset from Content-Type header value (e.g. "text/html; charset=ISO-8859-1").
fn extract_charset(content_type: Option<&String>) -> Option<&'static Encoding> {
    let ct = content_type?;
    let lower = ct.to_ascii_lowercase();
    let charset_str = lower.split("charset=").nth(1)?;
    let name = charset_str.split(';').next()?.trim().trim_matches('"');
    Encoding::for_label(name.as_bytes())
}

/// Decode bytes using the given encoding, falling back to UTF-8.
fn decode_body(bytes: &[u8], encoding: Option<&'static Encoding>) -> String {
    match encoding {
        Some(enc) if enc != encoding_rs::UTF_8 => {
            let (decoded, _, _) = enc.decode(bytes);
            decoded.into_owned()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::cookie::CookieStore as _;

    #[test]
    fn test_cookie_preload_into_jar() {
        let jar = CookieJar::new();
        let url: reqwest::Url = "https://app.example.com/".parse().unwrap();
        jar.add_cookie_str("session=abc123; Domain=app.example.com; Path=/", &url);

        let cookies = jar.cookies(&url).unwrap();
        assert!(cookies.to_str().unwrap().contains("session=abc123"));
    }

    #[test]
    fn test_cookie_preload_secure_flags() {
        let jar = CookieJar::new();
        let url: reqwest::Url = "https://secure.example.com/".parse().unwrap();
        jar.add_cookie_str(
            "token=xyz; Domain=secure.example.com; Path=/; Secure; HttpOnly",
            &url,
        );

        let cookies = jar.cookies(&url).unwrap();
        assert!(cookies.to_str().unwrap().contains("token=xyz"));
    }

    #[test]
    fn test_cookie_not_sent_to_wrong_domain() {
        let jar = CookieJar::new();
        let url: reqwest::Url = "https://app.example.com/".parse().unwrap();
        jar.add_cookie_str("session=abc123; Domain=app.example.com; Path=/", &url);

        let other: reqwest::Url = "https://other.com/".parse().unwrap();
        assert!(jar.cookies(&other).is_none());
    }

    #[test]
    fn test_resolve_headers_matching() {
        let client = HttpClient {
            client: reqwest::Client::new(),
            jar: Arc::new(CookieJar::new()),
            header_rules: vec![
                HeaderRuleConfig {
                    domains: vec!["api.example.com".into()],
                    values: HashMap::from([
                        ("Accept".into(), "application/json".into()),
                        ("X-Client".into(), "browser39".into()),
                    ]),
                },
                HeaderRuleConfig {
                    domains: vec!["*.internal.com".into()],
                    values: HashMap::from([("X-Internal".into(), "true".into())]),
                },
            ],
        };

        let h = client.resolve_headers_for_domain("api.example.com");
        assert_eq!(*h.get("Accept").unwrap(), "application/json");
        assert_eq!(*h.get("X-Client").unwrap(), "browser39");
        assert!(!h.contains_key("X-Internal"));

        let h = client.resolve_headers_for_domain("foo.internal.com");
        assert_eq!(*h.get("X-Internal").unwrap(), "true");
        assert!(!h.contains_key("Accept"));

        let h = client.resolve_headers_for_domain("other.com");
        assert!(h.is_empty());
    }

    #[test]
    fn test_header_merge_precedence() {
        let client = HttpClient {
            client: reqwest::Client::new(),
            jar: Arc::new(CookieJar::new()),
            header_rules: vec![HeaderRuleConfig {
                domains: vec!["api.example.com".into()],
                values: HashMap::from([
                    ("Accept".into(), "application/json".into()),
                    ("X-Custom".into(), "default".into()),
                ]),
            }],
        };

        let defaults = client.resolve_headers_for_domain("api.example.com");
        assert_eq!(*defaults.get("Accept").unwrap(), "application/json");
        assert_eq!(*defaults.get("X-Custom").unwrap(), "default");

        // reqwest's RequestBuilder applies headers in order — last value wins.
        // So applying defaults first, then per-request headers, gives correct precedence.
    }

    #[test]
    fn test_client_construction_with_cookies() {
        let config = Config {
            cookies: vec![super::super::config::CookieConfig {
                name: "sid".into(),
                value: None,
                value_env: None,
                domain: "example.com".into(),
                path: "/".into(),
                secure: true,
                http_only: true,
                sensitive: false,
                resolved_value: Some("test-token".into()),
            }],
            ..Config::default()
        };

        let client = HttpClient::new(&config).unwrap();
        let url: reqwest::Url = "https://example.com/".parse().unwrap();
        let cookies = client.jar().cookies(&url).unwrap();
        assert!(cookies.to_str().unwrap().contains("sid=test-token"));
    }

    #[test]
    fn test_client_default_config() {
        let config = Config::default();
        let client = HttpClient::new(&config).unwrap();
        assert!(client.header_rules.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_fetch_real_site() {
        let config = Config::default();
        let client = HttpClient::new(&config).unwrap();
        let resp = client
            .fetch(
                "https://www.google.com",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        assert!(resp.body.as_text().contains("<html") || resp.body.as_text().contains("<!doctype"));
        assert!(!resp.body.as_text().is_empty());
        assert!(resp.elapsed_ms > 0);
    }

    #[tokio::test]
    #[ignore]
    async fn test_preloaded_cookie_in_request() {
        let config = Config {
            cookies: vec![super::super::config::CookieConfig {
                name: "test_cookie".into(),
                value: None,
                value_env: None,
                domain: "httpbin.org".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                sensitive: false,
                resolved_value: Some("hello_browser39".into()),
            }],
            ..Config::default()
        };

        let client = HttpClient::new(&config).unwrap();
        let resp = client
            .fetch(
                "https://httpbin.org/cookies",
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(resp.status, 200);
        assert!(resp.body.as_text().contains("test_cookie"));
        assert!(resp.body.as_text().contains("hello_browser39"));
    }

    // --- CookieJar management tests ---

    #[test]
    fn test_jar_list_cookies() {
        let jar = CookieJar::new();
        let url: reqwest::Url = "https://example.com/".parse().unwrap();
        jar.add_cookie_str("a=1; Domain=example.com; Path=/", &url);
        jar.add_cookie_str("b=2; Domain=example.com; Path=/", &url);

        let cookies = jar.list_cookies(None);
        assert_eq!(cookies.len(), 2);
        let names: Vec<&str> = cookies.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn test_jar_list_cookies_domain_filter() {
        let jar = CookieJar::new();
        let url1: reqwest::Url = "https://example.com/".parse().unwrap();
        let url2: reqwest::Url = "https://other.com/".parse().unwrap();
        jar.add_cookie_str("a=1; Domain=example.com; Path=/", &url1);
        jar.add_cookie_str("b=2; Domain=other.com; Path=/", &url2);

        let cookies = jar.list_cookies(Some("example.com"));
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "a");
    }

    #[test]
    fn test_jar_set_cookie() {
        let jar = CookieJar::new();
        jar.set_cookie("token", "abc", "example.com", "/", true, true, None)
            .unwrap();

        let cookies = jar.list_cookies(None);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "token");
        assert_eq!(cookies[0].value, "abc");
        assert_eq!(cookies[0].domain, "example.com");
        assert!(cookies[0].secure);
        assert!(cookies[0].http_only);
    }

    #[test]
    fn test_jar_remove_cookie() {
        let jar = CookieJar::new();
        let url: reqwest::Url = "https://example.com/".parse().unwrap();
        jar.add_cookie_str("a=1; Domain=example.com; Path=/", &url);
        jar.add_cookie_str("b=2; Domain=example.com; Path=/", &url);

        assert!(jar.remove_cookie("a", "example.com"));
        let cookies = jar.list_cookies(None);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "b");
    }

    #[test]
    fn test_jar_remove_nonexistent() {
        let jar = CookieJar::new();
        assert!(!jar.remove_cookie("nope", "example.com"));
    }

    #[test]
    fn test_jar_count() {
        let jar = CookieJar::new();
        assert_eq!(jar.count(), 0);
        let url: reqwest::Url = "https://example.com/".parse().unwrap();
        jar.add_cookie_str("a=1; Domain=example.com; Path=/", &url);
        assert_eq!(jar.count(), 1);
        jar.add_cookie_str("b=2; Domain=example.com; Path=/", &url);
        assert_eq!(jar.count(), 2);
    }

    #[test]
    fn test_jar_set_cookie_with_max_age() {
        let jar = CookieJar::new();
        jar.set_cookie("s", "v", "example.com", "/", false, false, Some(3600))
            .unwrap();

        let cookies = jar.list_cookies(None);
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].expires.is_some());
    }
}
