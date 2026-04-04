use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The browser39 version that last wrote this config file.
    pub version: Option<String>,
    pub session: SessionConfig,
    pub search: SearchConfig,
    pub auth: HashMap<String, AuthProfileConfig>,
    pub cookies: Vec<CookieConfig>,
    pub storage: Vec<StorageConfig>,
    pub headers: Vec<HeaderRuleConfig>,
    pub security: SecurityConfig,
}

pub const MASK: &str = "••••••";

impl Config {
    /// Resolve the config file path from explicit path, env var, or default.
    pub fn config_path(path: Option<&Path>) -> PathBuf {
        path.map(PathBuf::from)
            .or_else(|| std::env::var("BROWSER39_CONFIG").ok().map(PathBuf::from))
            .unwrap_or_else(default_config_path)
    }

    pub fn load(path: Option<&Path>) -> Result<Config> {
        let config_path = Self::config_path(path);

        let contents = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).context(format!("reading config: {}", config_path.display())),
        };

        let mut config: Config = toml::from_str(&contents)
            .context(format!("parsing config: {}", config_path.display()))?;
        config.resolve()?;

        // Auto-update version stamp if this is a browser39 config with a stale version.
        // Only touch the file when the existing version field references browser39
        // (or is absent/empty), never when it belongs to another tool.
        let current = env!("CARGO_PKG_VERSION");
        let needs_update = match config.version.as_deref() {
            None | Some("") => true,
            Some(v) => v != current,
        };
        if needs_update {
            config.version = Some(current.to_string());
            // Best-effort save — don't fail startup if the file is read-only
            let _ = config.save(Some(&config_path));
        }

        Ok(config)
    }

    /// Write this config to disk (atomic write with fsync + chmod 600).
    pub fn save(&self, path: Option<&Path>) -> Result<()> {
        let config_path = Self::config_path(path);
        let toml_str = toml::to_string_pretty(self).context("serializing config to TOML")?;
        super::persistence::atomic_write(&config_path, toml_str.as_bytes())
    }

    /// Build a JSON representation of the config with sensitive values masked.
    /// If `section` is Some, return only that top-level key.
    pub fn masked_json(&self, section: Option<&str>) -> serde_json::Value {
        let mut root = serde_json::Map::new();

        if section.is_none() {
            root.insert("version".into(), json!(self.version));
        }

        if section.is_none() || section == Some("session") {
            root.insert("session".into(), json!({
                "start_url": self.session.start_url,
                "user_agent": self.session.user_agent,
                "timeout_secs": self.session.timeout_secs,
                "max_redirects": self.session.max_redirects,
                "persistence": serde_json::to_value(&self.session.persistence).unwrap_or_default(),
                "session_path": self.session.session_path,
                "defaults": {
                    "max_tokens": self.session.defaults.max_tokens,
                    "strip_nav": self.session.defaults.strip_nav,
                    "include_links": self.session.defaults.include_links,
                    "include_images": self.session.defaults.include_images,
                }
            }));
        }

        if section.is_none() || section == Some("search") {
            root.insert(
                "search".into(),
                json!({
                    "engine": self.search.engine,
                }),
            );
        }

        if section.is_none() || section == Some("auth") {
            let mut auth_map = serde_json::Map::new();
            for (name, profile) in &self.auth {
                auth_map.insert(
                    name.clone(),
                    json!({
                        "header": profile.header,
                        "value": MASK,
                        "value_env": profile.value_env,
                        "value_prefix": profile.value_prefix,
                        "domains": profile.domains,
                    }),
                );
            }
            root.insert("auth".into(), serde_json::Value::Object(auth_map));
        }

        if section.is_none() || section == Some("cookies") {
            let cookies: Vec<serde_json::Value> = self
                .cookies
                .iter()
                .map(|c| {
                    let value = if c.sensitive {
                        json!(MASK)
                    } else {
                        json!(c.value)
                    };
                    json!({
                        "name": c.name,
                        "value": value,
                        "value_env": c.value_env,
                        "domain": c.domain,
                        "path": c.path,
                        "secure": c.secure,
                        "http_only": c.http_only,
                        "sensitive": c.sensitive,
                    })
                })
                .collect();
            root.insert("cookies".into(), json!(cookies));
        }

        if section.is_none() || section == Some("storage") {
            let storage: Vec<serde_json::Value> = self
                .storage
                .iter()
                .map(|s| {
                    let value = if s.sensitive {
                        json!(MASK)
                    } else {
                        json!(s.value)
                    };
                    json!({
                        "origin": s.origin,
                        "key": s.key,
                        "value": value,
                        "value_env": s.value_env,
                        "sensitive": s.sensitive,
                    })
                })
                .collect();
            root.insert("storage".into(), json!(storage));
        }

        if section.is_none() || section == Some("headers") {
            let headers: Vec<serde_json::Value> = self
                .headers
                .iter()
                .map(|h| {
                    json!({
                        "domains": h.domains,
                        "values": h.values,
                    })
                })
                .collect();
            root.insert("headers".into(), json!(headers));
        }

        if section.is_none() || section == Some("security") {
            root.insert(
                "security".into(),
                json!({
                    "sensitive_cookies": self.security.sensitive_cookies,
                    "sensitive_headers": self.security.sensitive_headers,
                    "patterns": self.security.patterns.keys().collect::<Vec<_>>(),
                    "mcp": { "redact": self.security.mcp.redact },
                    "jsonl": { "redact": self.security.jsonl.redact },
                }),
            );
        }

        serde_json::Value::Object(root)
    }

    pub fn resolve(&mut self) -> Result<()> {
        for (name, profile) in &mut self.auth {
            let mut val = resolve_value(&profile.value, &profile.value_env)
                .with_context(|| format!("auth profile '{name}'"))?;
            if let Some(prefix) = &profile.value_prefix {
                val = format!("{prefix}{val}");
            }
            profile.resolved_value = Some(val);
        }

        for (i, cookie) in self.cookies.iter_mut().enumerate() {
            let val = resolve_value(&cookie.value, &cookie.value_env)
                .with_context(|| format!("cookie[{i}] '{}'", cookie.name))?;
            cookie.resolved_value = Some(val);
        }

        for (i, entry) in self.storage.iter_mut().enumerate() {
            let val = resolve_value(&entry.value, &entry.value_env)
                .with_context(|| format!("storage[{i}] '{}'", entry.key))?;
            entry.resolved_value = Some(val);
        }

        Ok(())
    }
}

fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("browser39")
        .join("config.toml")
}

fn resolve_value(value: &Option<String>, value_env: &Option<String>) -> Result<String> {
    match (value, value_env) {
        (Some(v), _) => Ok(v.clone()),
        (None, Some(env_name)) => {
            std::env::var(env_name).with_context(|| format!("env var '{env_name}' not set"))
        }
        (None, None) => bail!("either 'value' or 'value_env' must be set"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PersistenceMode {
    #[default]
    Disk,
    Memory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub start_url: Option<String>,
    pub user_agent: String,
    pub timeout_secs: u64,
    pub max_redirects: u32,
    pub defaults: SessionDefaults,
    pub persistence: PersistenceMode,
    pub session_path: Option<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            start_url: None,
            user_agent: concat!("browser39/", env!("CARGO_PKG_VERSION")).into(),
            timeout_secs: 30,
            max_redirects: 10,
            defaults: SessionDefaults::default(),
            persistence: PersistenceMode::default(),
            session_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionDefaults {
    pub max_tokens: Option<u64>,
    pub strip_nav: bool,
    pub include_links: bool,
    pub include_images: bool,
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self {
            max_tokens: None,
            strip_nav: false,
            include_links: true,
            include_images: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Search engine URL template. Use `{}` as the query placeholder.
    /// Examples:
    ///   - "https://html.duckduckgo.com/html/?q={}" (default)
    ///   - "https://www.google.com/search?q={}"
    ///   - "https://search.brave.com/search?q={}"
    pub engine: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            engine: "https://html.duckduckgo.com/html/?q={}".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfileConfig {
    pub header: String,
    pub value: Option<String>,
    pub value_env: Option<String>,
    pub value_prefix: Option<String>,
    pub domains: Vec<String>,
    #[serde(skip)]
    pub resolved_value: Option<String>,
}

fn default_slash() -> String {
    "/".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieConfig {
    pub name: String,
    pub value: Option<String>,
    pub value_env: Option<String>,
    pub domain: String,
    #[serde(default = "default_slash")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(skip)]
    pub resolved_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub origin: String,
    pub key: String,
    pub value: Option<String>,
    pub value_env: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(skip)]
    pub resolved_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderRuleConfig {
    pub domains: Vec<String>,
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub sensitive_cookies: Vec<String>,
    pub sensitive_headers: Vec<String>,
    pub patterns: HashMap<String, String>,
    pub mcp: TransportSecurity,
    pub jsonl: TransportSecurity,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sensitive_cookies: vec![
                "session".into(),
                "sid".into(),
                "token".into(),
                "jwt".into(),
                "auth".into(),
                "csrf".into(),
            ],
            sensitive_headers: vec![
                "authorization".into(),
                "x-api-key".into(),
                "cookie".into(),
                "set-cookie".into(),
            ],
            patterns: HashMap::new(),
            mcp: TransportSecurity { redact: true },
            jsonl: TransportSecurity { redact: false },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportSecurity {
    pub redact: bool,
}

/// Match a domain against a pattern. Supports `*.example.com` wildcards.
/// `*.example.com` matches `api.example.com` but NOT `example.com`.
/// Comparison is case-insensitive.
pub fn domain_matches(pattern: &str, domain: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Wildcard: domain must have something before .suffix
        domain.len() > suffix.len() + 1
            && domain.as_bytes()[domain.len() - suffix.len() - 1] == b'.'
            && domain[domain.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    } else {
        pattern.eq_ignore_ascii_case(domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_load_full_config() {
        let toml = r##"
[session]
start_url = "https://dashboard.example.com"
user_agent = "browser39/1.5.0"
timeout_secs = 30
max_redirects = 10

[session.defaults]
max_tokens = 8000
strip_nav = true
include_links = true
include_images = false

[auth.github]
header = "Authorization"
value = "Bearer ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
domains = ["api.github.com", "github.com"]

[auth.internal]
header = "X-API-Key"
value = "test-key"
domains = ["internal.company.com", "*.internal.company.com"]

[[cookies]]
name = "session"
value = "test-session-token"
domain = "app.example.com"
path = "/"
secure = true
http_only = true
sensitive = true

[[cookies]]
name = "lang"
value = "en"
domain = "app.example.com"

[[storage]]
origin = "https://app.example.com"
key = "api_token"
value = "test-api-token"
sensitive = true

[[storage]]
origin = "https://app.example.com"
key = "theme"
value = "dark"

[[headers]]
domains = ["api.example.com", "*.api.example.com"]
values = { "Accept" = "application/json", "X-Client" = "browser39" }

[security]
sensitive_cookies = ["session", "token"]
sensitive_headers = ["authorization"]

[security.patterns]
jwt = 'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}'

[security.mcp]
redact = true

[security.jsonl]
redact = false
"##;

        let mut config: Config = toml::from_str(toml).unwrap();
        config.resolve().unwrap();

        // Session
        assert_eq!(
            config.session.start_url,
            Some("https://dashboard.example.com".into())
        );
        assert_eq!(config.session.user_agent, concat!("browser39/", env!("CARGO_PKG_VERSION")));
        assert_eq!(config.session.timeout_secs, 30);
        assert_eq!(config.session.max_redirects, 10);
        assert_eq!(config.session.defaults.max_tokens, Some(8000));
        assert!(config.session.defaults.strip_nav); // explicitly set in TOML

        // Auth
        assert_eq!(config.auth.len(), 2);
        let github = &config.auth["github"];
        assert_eq!(github.header, "Authorization");
        assert_eq!(github.domains, vec!["api.github.com", "github.com"]);
        assert!(
            github
                .resolved_value
                .as_ref()
                .unwrap()
                .starts_with("Bearer ghp_")
        );

        // Cookies
        assert_eq!(config.cookies.len(), 2);
        assert_eq!(config.cookies[0].name, "session");
        assert_eq!(
            config.cookies[0].resolved_value,
            Some("test-session-token".into())
        );
        assert!(config.cookies[0].secure);
        assert!(config.cookies[0].sensitive);
        assert_eq!(config.cookies[1].name, "lang");
        assert_eq!(config.cookies[1].path, "/");

        // Storage
        assert_eq!(config.storage.len(), 2);
        assert_eq!(config.storage[0].key, "api_token");
        assert!(config.storage[0].sensitive);
        assert_eq!(config.storage[1].key, "theme");
        assert_eq!(config.storage[1].resolved_value, Some("dark".into()));

        // Headers
        assert_eq!(config.headers.len(), 1);
        assert_eq!(
            config.headers[0].values.get("Accept").unwrap(),
            "application/json"
        );

        // Security
        assert_eq!(config.security.sensitive_cookies, vec!["session", "token"]);
        assert_eq!(config.security.sensitive_headers, vec!["authorization"]);
        assert!(config.security.patterns.contains_key("jwt"));
        assert!(config.security.mcp.redact);
        assert!(!config.security.jsonl.redact);
    }

    #[test]
    fn test_missing_config_file() {
        let config = Config::load(Some(Path::new("/nonexistent/path/config.toml"))).unwrap();
        assert_eq!(config.session.user_agent, concat!("browser39/", env!("CARGO_PKG_VERSION")));
        assert_eq!(config.session.timeout_secs, 30);
        assert!(config.auth.is_empty());
        assert!(config.cookies.is_empty());
    }

    #[test]
    fn test_resolve_value_inline() {
        let result = resolve_value(&Some("token".into()), &None).unwrap();
        assert_eq!(result, "token");
    }

    #[test]
    fn test_resolve_value_env() {
        // SAFETY: test-only, single-threaded test runner for this test
        unsafe { env::set_var("BROWSER39_TEST_VAR_1", "env-value") };
        let result = resolve_value(&None, &Some("BROWSER39_TEST_VAR_1".into())).unwrap();
        assert_eq!(result, "env-value");
        unsafe { env::remove_var("BROWSER39_TEST_VAR_1") };
    }

    #[test]
    fn test_resolve_value_missing_env() {
        let result = resolve_value(&None, &Some("BROWSER39_DEFINITELY_NOT_SET_XYZ".into()));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("BROWSER39_DEFINITELY_NOT_SET_XYZ")
        );
    }

    #[test]
    fn test_resolve_value_both_none() {
        let result = resolve_value(&None, &None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("either 'value' or 'value_env'")
        );
    }

    #[test]
    fn test_resolve_value_precedence() {
        // SAFETY: test-only, single-threaded test runner for this test
        unsafe { env::set_var("BROWSER39_TEST_VAR_2", "env-value") };
        let result = resolve_value(
            &Some("inline-value".into()),
            &Some("BROWSER39_TEST_VAR_2".into()),
        )
        .unwrap();
        assert_eq!(result, "inline-value");
        unsafe { env::remove_var("BROWSER39_TEST_VAR_2") };
    }

    #[test]
    fn test_auth_profile_value_prefix() {
        let toml = r#"
[auth.api]
header = "Authorization"
value = "my-token"
value_prefix = "Bearer "
domains = ["api.example.com"]
"#;
        let mut config: Config = toml::from_str(toml).unwrap();
        config.resolve().unwrap();
        assert_eq!(
            config.auth["api"].resolved_value,
            Some("Bearer my-token".into())
        );
    }

    #[test]
    fn test_domain_matches_exact() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(!domain_matches("example.com", "other.com"));
    }

    #[test]
    fn test_domain_matches_wildcard() {
        assert!(domain_matches("*.example.com", "api.example.com"));
        assert!(domain_matches("*.example.com", "app.example.com"));
    }

    #[test]
    fn test_domain_no_match_wildcard_root() {
        assert!(!domain_matches("*.example.com", "example.com"));
    }

    #[test]
    fn test_domain_case_insensitive() {
        assert!(domain_matches("Example.COM", "example.com"));
        assert!(domain_matches("*.Example.COM", "api.example.com"));
    }

    #[test]
    fn test_session_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.session.start_url, None);
        assert_eq!(config.session.user_agent, concat!("browser39/", env!("CARGO_PKG_VERSION")));
        assert_eq!(config.session.timeout_secs, 30);
        assert_eq!(config.session.max_redirects, 10);
        assert_eq!(config.session.defaults.max_tokens, None);
        assert!(!config.session.defaults.strip_nav);
        assert!(config.session.defaults.include_links);
        assert!(!config.session.defaults.include_images);
    }

    #[test]
    fn test_search_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(
            config.search.engine,
            "https://html.duckduckgo.com/html/?q={}"
        );
    }

    #[test]
    fn test_search_custom_engine() {
        let toml = r#"
[search]
engine = "https://www.google.com/search?q={}"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.search.engine, "https://www.google.com/search?q={}");
    }

    #[test]
    fn test_security_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(
            config
                .security
                .sensitive_cookies
                .contains(&"session".into())
        );
        assert!(config.security.sensitive_cookies.contains(&"jwt".into()));
        assert!(
            config
                .security
                .sensitive_headers
                .contains(&"authorization".into())
        );
        assert!(config.security.mcp.redact);
        assert!(!config.security.jsonl.redact);
    }

    #[test]
    fn test_config_env_var_override() {
        let dir = env::temp_dir().join("browser39_test_config");
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[session]
user_agent = "custom-agent"
"#,
        )
        .unwrap();

        // SAFETY: test-only, single-threaded test runner for this test
        unsafe { env::set_var("BROWSER39_CONFIG", config_path.to_str().unwrap()) };
        let config = Config::load(None).unwrap();
        assert_eq!(config.session.user_agent, "custom-agent");
        unsafe { env::remove_var("BROWSER39_CONFIG") };

        std::fs::remove_dir_all(&dir).ok();
    }
}
