use std::collections::HashMap;

/// In-memory secret store that maps opaque handles to sensitive values.
///
/// Handles have the form `${browser39_secret_N}` where N is a monotonically
/// increasing counter.
pub struct SecretStore {
    counter: u64,
    secrets: HashMap<String, String>,
    reverse: HashMap<String, String>,
}

impl SecretStore {
    pub fn new() -> Self {
        Self {
            counter: 1,
            secrets: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Store a secret value and return its handle.
    /// If the same value was already stored, returns the existing handle.
    pub fn store(&mut self, value: &str) -> String {
        if let Some(handle) = self.reverse.get(value) {
            return handle.clone();
        }
        let n = self.counter;
        self.counter += 1;
        let handle = format!("${{browser39_secret_{n}}}");
        self.secrets.insert(handle.clone(), value.to_string());
        self.reverse.insert(value.to_string(), handle.clone());
        handle
    }

    /// Resolve all `${browser39_secret_N}` handles in the given text,
    /// replacing each with the real value.
    pub fn resolve(&self, text: &str) -> String {
        if !text.contains("${browser39_secret_") {
            return text.to_string();
        }
        let mut result = text.to_string();
        for (handle, value) in &self.secrets {
            if result.contains(handle.as_str()) {
                result = result.replace(handle.as_str(), value);
            }
        }
        result
    }

    #[allow(dead_code)]
    pub fn get(&self, handle: &str) -> Option<&str> {
        self.secrets.get(handle).map(|s| s.as_str())
    }

    #[allow(dead_code)]
    pub fn redact_known(&self, text: &str) -> String {
        if self.reverse.is_empty() {
            return text.to_string();
        }
        let mut pairs: Vec<(&str, &str)> = self
            .reverse
            .iter()
            .map(|(val, handle)| (val.as_str(), handle.as_str()))
            .collect();
        pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        let mut result = text.to_string();
        for (value, handle) in pairs {
            if result.contains(value) {
                result = result.replace(value, handle);
            }
        }
        result
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_get() {
        let mut store = SecretStore::new();
        let handle = store.store("my-secret-value");
        assert!(handle.starts_with("${browser39_secret_"));
        assert!(handle.ends_with('}'));
        assert_eq!(store.get(&handle).unwrap(), "my-secret-value");
    }

    #[test]
    fn test_store_dedup() {
        let mut store = SecretStore::new();
        let h1 = store.store("same-value");
        let h2 = store.store("same-value");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_store_distinct() {
        let mut store = SecretStore::new();
        let h1 = store.store("value-a");
        let h2 = store.store("value-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_resolve() {
        let mut store = SecretStore::new();
        let handle = store.store("token123");
        let text = format!("Authorization: Bearer {handle}");
        let resolved = store.resolve(&text);
        assert_eq!(resolved, "Authorization: Bearer token123");
    }

    #[test]
    fn test_resolve_no_handles() {
        let store = SecretStore::new();
        let text = "no handles here";
        assert_eq!(store.resolve(text), text);
    }

    #[test]
    fn test_resolve_multiple() {
        let mut store = SecretStore::new();
        let h1 = store.store("user");
        let h2 = store.store("pass");
        let text = format!("{h1}:{h2}");
        let resolved = store.resolve(&text);
        assert_eq!(resolved, "user:pass");
    }

    #[test]
    fn test_redact_known() {
        let mut store = SecretStore::new();
        let handle = store.store("my-api-key-12345");
        let text = "key is my-api-key-12345 in the response";
        let redacted = store.redact_known(text);
        assert_eq!(redacted, format!("key is {handle} in the response"));
    }

    #[test]
    fn test_redact_known_empty_store() {
        let store = SecretStore::new();
        let text = "nothing to redact";
        assert_eq!(store.redact_known(text), text);
    }

    #[test]
    fn test_redact_known_longer_first() {
        let mut store = SecretStore::new();
        let h_short = store.store("abc");
        let h_long = store.store("abcdef");
        let text = "prefix abcdef suffix";
        let redacted = store.redact_known(text);
        assert_eq!(redacted, format!("prefix {h_long} suffix"));
        assert!(!redacted.contains(&h_short));
    }

    #[test]
    fn test_get_nonexistent() {
        let store = SecretStore::new();
        assert_eq!(store.get("${browser39_secret_999}"), None);
    }

    #[test]
    fn test_is_empty() {
        let mut store = SecretStore::new();
        assert!(store.is_empty());
        store.store("x");
        assert!(!store.is_empty());
    }
}
