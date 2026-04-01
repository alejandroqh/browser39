use std::collections::HashMap;

use anyhow::{Context, Result};

use super::config::{domain_matches, AuthProfileConfig};
use super::error::ErrorCode;
use crate::service::service::ServiceError;

/// Resolve an auth profile and inject its header into the given headers map.
///
/// Steps:
/// 1. Look up the profile by name.
/// 2. Verify the request URL's domain is in the profile's allowed domains.
/// 3. Insert the resolved header value.
///
/// Returns error codes:
/// - `AUTH_PROFILE_NOT_FOUND` if the profile name doesn't exist.
/// - `AUTH_PROFILE_DOMAIN_MISMATCH` if the URL's domain is not in the allowlist.
pub fn apply_auth_profile(
    profiles: &HashMap<String, AuthProfileConfig>,
    profile_name: &str,
    url: &str,
    headers: &mut HashMap<String, String>,
) -> Result<()> {
    let profile = profiles.get(profile_name).ok_or_else(|| {
        ServiceError::new(
            ErrorCode::AuthProfileNotFound,
            format!("auth profile '{profile_name}' not found"),
        )
    })?;

    let parsed: reqwest::Url = url.parse().context("invalid URL")?;
    let domain = parsed.host_str().unwrap_or_default();
    if !profile.domains.iter().any(|p| domain_matches(p, domain)) {
        return Err(ServiceError::new(
            ErrorCode::AuthProfileDomainMismatch,
            format!("auth profile '{profile_name}' not allowed for domain '{domain}'"),
        )
        .into());
    }

    if let Some(ref val) = profile.resolved_value {
        headers.insert(profile.header.clone(), val.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_profiles() -> HashMap<String, AuthProfileConfig> {
        let mut profiles = HashMap::new();
        profiles.insert(
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
        profiles.insert(
            "internal".into(),
            AuthProfileConfig {
                header: "X-API-Key".into(),
                value: Some("key-123".into()),
                value_env: None,
                value_prefix: None,
                domains: vec!["internal.company.com".into()],
                resolved_value: Some("key-123".into()),
            },
        );
        profiles
    }

    #[test]
    fn test_apply_auth_profile_success() {
        let profiles = test_profiles();
        let mut headers = HashMap::new();
        apply_auth_profile(
            &profiles,
            "github",
            "https://api.github.com/repos",
            &mut headers,
        )
        .unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer ghp_test");
    }

    #[test]
    fn test_apply_auth_profile_wildcard() {
        let profiles = test_profiles();
        let mut headers = HashMap::new();
        apply_auth_profile(
            &profiles,
            "github",
            "https://raw.github.com/file",
            &mut headers,
        )
        .unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer ghp_test");
    }

    #[test]
    fn test_apply_auth_profile_not_found() {
        let profiles = test_profiles();
        let mut headers = HashMap::new();
        let err = apply_auth_profile(
            &profiles,
            "nonexistent",
            "https://example.com",
            &mut headers,
        )
        .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::AuthProfileNotFound);
    }

    #[test]
    fn test_apply_auth_profile_domain_mismatch() {
        let profiles = test_profiles();
        let mut headers = HashMap::new();
        let err = apply_auth_profile(
            &profiles,
            "github",
            "https://evil.com/steal",
            &mut headers,
        )
        .unwrap_err();
        let se = err.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(se.code, ErrorCode::AuthProfileDomainMismatch);
    }

    #[test]
    fn test_apply_auth_profile_no_resolved_value() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "empty".into(),
            AuthProfileConfig {
                header: "Authorization".into(),
                value: None,
                value_env: None,
                value_prefix: None,
                domains: vec!["example.com".into()],
                resolved_value: None,
            },
        );
        let mut headers = HashMap::new();
        // Should succeed but not insert any header
        apply_auth_profile(&profiles, "empty", "https://example.com", &mut headers).unwrap();
        assert!(headers.is_empty());
    }
}
