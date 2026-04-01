use regex::Regex;

use super::config::SecurityConfig;
use super::page::{CookieInfo, CookiesResult, PageResult, StorageGetResult, StorageListResult};
use super::secrets::SecretStore;

/// Built-in regex patterns for common secret formats.
fn builtin_patterns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("jwt", r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
        ("github_pat", r"gh[ps]_[A-Za-z0-9_]{36,}"),
        ("github_fine", r"github_pat_[A-Za-z0-9_]{22,}"),
        ("openai", r"sk-[A-Za-z0-9]{20,}"),
        ("slack_bot", r"xoxb-[0-9]{10,}-[A-Za-z0-9]{20,}"),
        ("slack_user", r"xoxp-[0-9]{10,}-[A-Za-z0-9]{20,}"),
        ("slack_app", r"xapp-[0-9]-[A-Za-z0-9]{20,}"),
    ]
}

/// Compiled redaction engine that applies pattern-based and name-based
/// redaction to outgoing responses.
pub struct RedactionEngine {
    patterns: Vec<(String, Regex)>,
    /// Pre-lowercased cookie name substrings considered sensitive.
    sensitive_cookies: Vec<String>,
    pub enabled: bool,
}

impl RedactionEngine {
    /// Build a redaction engine from SecurityConfig for a given transport.
    pub fn new(config: &SecurityConfig, transport: Transport) -> Self {
        let enabled = match transport {
            Transport::Mcp => config.mcp.redact,
            Transport::Jsonl => config.jsonl.redact,
        };

        let mut patterns = Vec::new();

        for (name, pat) in builtin_patterns() {
            if let Ok(re) = Regex::new(pat) {
                patterns.push((name.to_string(), re));
            }
        }

        for (name, pat) in &config.patterns {
            match Regex::new(pat) {
                Ok(re) => {
                    patterns.retain(|(n, _)| n != name);
                    patterns.push((name.clone(), re));
                }
                Err(e) => {
                    eprintln!("browser39: invalid redaction pattern '{name}': {e}");
                }
            }
        }

        Self {
            patterns,
            sensitive_cookies: config
                .sensitive_cookies
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect(),
            enabled,
        }
    }

    /// Scan text for secret patterns and store each match in the SecretStore,
    /// replacing occurrences with their handles.
    pub fn redact_text(&self, text: &str, secrets: &mut SecretStore) -> String {
        if !self.enabled || self.patterns.is_empty() {
            return text.to_string();
        }

        let mut result = text.to_string();
        for (_name, re) in &self.patterns {
            if !re.is_match(&result) {
                continue;
            }
            // Collect unique matches, store them to get handles, then replace.
            // We can't use replace_all with a mutable closure since secrets
            // needs &mut while regex borrows the result string.
            let mut match_to_handle: Vec<(String, String)> = Vec::new();
            for m in re.find_iter(&result) {
                let matched = m.as_str().to_string();
                let handle = secrets.store(&matched);
                match_to_handle.push((matched, handle));
            }
            match_to_handle.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
            match_to_handle.dedup_by(|a, b| a.0 == b.0);
            for (matched, handle) in &match_to_handle {
                result = result.replace(matched.as_str(), handle);
            }
        }
        result
    }

    /// Check if a cookie name is considered sensitive.
    pub fn is_sensitive_cookie(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.sensitive_cookies.iter().any(|s| lower.contains(s.as_str()))
    }

    pub fn redact_page_result(&self, page: &mut PageResult, secrets: &mut SecretStore) {
        if !self.enabled {
            return;
        }
        page.markdown = self.redact_text(&page.markdown, secrets);
    }

    pub fn redact_cookies_result(&self, result: &mut CookiesResult, secrets: &mut SecretStore) {
        if !self.enabled {
            return;
        }
        for cookie in &mut result.cookies {
            self.redact_cookie(cookie, secrets);
        }
    }

    pub fn redact_cookie(&self, cookie: &mut CookieInfo, secrets: &mut SecretStore) {
        if !self.enabled {
            return;
        }
        if self.is_sensitive_cookie(&cookie.name) {
            let handle = secrets.store(&cookie.value);
            cookie.handle = Some(handle);
            cookie.value = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".to_string();
        } else {
            // Only pattern-match non-sensitive cookies (sensitive ones are already masked)
            let redacted = self.redact_text(&cookie.value, secrets);
            if redacted != cookie.value {
                cookie.value = redacted;
            }
        }
    }

    pub fn redact_storage_get(&self, result: &mut StorageGetResult, secrets: &mut SecretStore) {
        if !self.enabled {
            return;
        }
        if let Some(ref value) = result.value {
            let redacted = self.redact_text(value, secrets);
            if redacted != *value {
                let handle = secrets.store(value);
                result.handle = Some(handle);
                result.value = Some(redacted);
            }
        }
    }

    pub fn redact_storage_list(&self, result: &mut StorageListResult, secrets: &mut SecretStore) {
        if !self.enabled {
            return;
        }
        for value in result.entries.values_mut() {
            let redacted = self.redact_text(value, secrets);
            if redacted != *value {
                *value = redacted;
            }
        }
    }
}

/// Transport type for determining redaction behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Mcp,
    Jsonl,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{SecurityConfig, TransportSecurity};
    use std::collections::HashMap;

    fn test_config() -> SecurityConfig {
        SecurityConfig {
            sensitive_cookies: vec![
                "session".into(),
                "sid".into(),
                "token".into(),
                "jwt".into(),
                "auth".into(),
                "csrf".into(),
            ],
            sensitive_headers: vec!["authorization".into(), "x-api-key".into()],
            patterns: HashMap::new(),
            mcp: TransportSecurity { redact: true },
            jsonl: TransportSecurity { redact: false },
        }
    }

    #[test]
    fn test_redact_jwt_in_markdown() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let text = format!("The token is {jwt} in the response");
        let redacted = engine.redact_text(&text, &mut secrets);

        assert!(!redacted.contains("eyJ"));
        assert!(redacted.contains("${browser39_secret_"));
        let resolved = secrets.resolve(&redacted);
        assert_eq!(resolved, text);
    }

    #[test]
    fn test_redact_github_pat() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let text = "token: ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";
        let redacted = engine.redact_text(text, &mut secrets);
        assert!(!redacted.contains("ghp_"));
        assert!(redacted.contains("${browser39_secret_"));
    }

    #[test]
    fn test_redact_openai_key() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let text = "key: sk-abcdefghijklmnopqrstuvwxyz";
        let redacted = engine.redact_text(text, &mut secrets);
        assert!(!redacted.contains("sk-"));
    }

    #[test]
    fn test_redact_disabled_for_jsonl() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Jsonl);
        let mut secrets = SecretStore::new();

        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let text = format!("The token is {jwt}");
        let redacted = engine.redact_text(&text, &mut secrets);
        assert_eq!(redacted, text);
    }

    #[test]
    fn test_sensitive_cookie_redacted() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let mut cookie = CookieInfo {
            name: "session_id".into(),
            value: "abc123secret".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires: None,
            handle: None,
        };

        engine.redact_cookie(&mut cookie, &mut secrets);
        assert_ne!(cookie.value, "abc123secret");
        assert!(cookie.handle.is_some());
        let resolved = secrets.resolve(cookie.handle.as_ref().unwrap());
        assert_eq!(resolved, "abc123secret");
    }

    #[test]
    fn test_non_sensitive_cookie_not_redacted() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let mut cookie = CookieInfo {
            name: "theme".into(),
            value: "dark".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: false,
            http_only: false,
            expires: None,
            handle: None,
        };

        engine.redact_cookie(&mut cookie, &mut secrets);
        assert_eq!(cookie.value, "dark");
        assert_eq!(cookie.handle, None);
    }

    #[test]
    fn test_is_sensitive_cookie() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);

        assert!(engine.is_sensitive_cookie("session"));
        assert!(engine.is_sensitive_cookie("SESSION_ID"));
        assert!(engine.is_sensitive_cookie("my_auth_token"));
        assert!(engine.is_sensitive_cookie("csrf_token"));
        assert!(!engine.is_sensitive_cookie("theme"));
        assert!(!engine.is_sensitive_cookie("language"));
    }

    #[test]
    fn test_user_pattern_override() {
        let mut config = test_config();
        config
            .patterns
            .insert("custom".into(), r"CUSTOM-[A-Z0-9]{10,}".into());
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let text = "secret: CUSTOM-ABCDEFGHIJ1234";
        let redacted = engine.redact_text(text, &mut secrets);
        assert!(!redacted.contains("CUSTOM-"));
    }

    #[test]
    fn test_no_patterns_no_redaction() {
        let config = SecurityConfig {
            sensitive_cookies: vec![],
            sensitive_headers: vec![],
            patterns: HashMap::new(),
            mcp: TransportSecurity { redact: true },
            jsonl: TransportSecurity { redact: false },
        };
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let text = "just plain text";
        assert_eq!(engine.redact_text(text, &mut secrets), text);
    }

    #[test]
    fn test_redact_storage_get() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let mut result = StorageGetResult {
            key: "api_token".into(),
            value: Some(jwt.to_string()),
            handle: None,
        };

        engine.redact_storage_get(&mut result, &mut secrets);
        assert!(!result.value.as_ref().unwrap().contains("eyJ"));
        assert!(result.handle.is_some());
    }

    #[test]
    fn test_redact_storage_list() {
        let config = test_config();
        let engine = RedactionEngine::new(&config, Transport::Mcp);
        let mut secrets = SecretStore::new();

        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let mut entries = HashMap::new();
        entries.insert("safe_key".into(), "safe_value".into());
        entries.insert("token".into(), jwt.to_string());

        let mut result = StorageListResult {
            origin: "https://example.com".into(),
            entries,
            count: 2,
        };

        engine.redact_storage_list(&mut result, &mut secrets);
        assert_eq!(result.entries.get("safe_key").unwrap(), "safe_value");
        assert!(!result.entries.get("token").unwrap().contains("eyJ"));
    }
}
