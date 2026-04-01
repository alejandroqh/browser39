use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::core::error::ErrorCode;
use crate::core::page::{FetchMode, FetchOptions, HttpMethod};

// --- Command envelope ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandEnvelope {
    pub id: String,
    pub v: u32,
    pub seq: u64,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Config(ConfigAction),
    Fetch(FetchAction),
    Links,
    DomQuery(DomQueryAction),
    Fill(FillAction),
    Submit(SubmitAction),
    Cookies(CookiesAction),
    SetCookie(SetCookieAction),
    DeleteCookie(DeleteCookieAction),
    StorageGet(StorageGetAction),
    StorageSet(StorageSetAction),
    StorageDelete(StorageDeleteAction),
    StorageList(StorageListAction),
    StorageClear(StorageClearAction),
    History(HistoryAction),
    Back,
    Forward,
    Info,
    Quit,
}

// --- Run configuration (seq 0) ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_delay: Option<StepDelay>,
}

/// Either a fixed number of seconds or a [min, max] range for random delay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepDelay {
    Fixed(f64),
    Range(f64, f64),
}

impl Serialize for StepDelay {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            StepDelay::Fixed(v) => serializer.serialize_f64(*v),
            StepDelay::Range(min, max) => {
                use serde::ser::SerializeSeq;
                let mut seq = serializer.serialize_seq(Some(2))?;
                seq.serialize_element(min)?;
                seq.serialize_element(max)?;
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for StepDelay {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Number(n) => {
                let v = n.as_f64().ok_or_else(|| serde::de::Error::custom("invalid number"))?;
                if v < 0.0 {
                    return Err(serde::de::Error::custom("step_delay must be non-negative"));
                }
                Ok(StepDelay::Fixed(v))
            }
            serde_json::Value::Array(arr) if arr.len() == 2 => {
                let min = arr[0].as_f64().ok_or_else(|| serde::de::Error::custom("min must be a number"))?;
                let max = arr[1].as_f64().ok_or_else(|| serde::de::Error::custom("max must be a number"))?;
                if min < 0.0 || max < 0.0 {
                    return Err(serde::de::Error::custom("step_delay values must be non-negative"));
                }
                if min > max {
                    return Err(serde::de::Error::custom("min must be <= max"));
                }
                Ok(StepDelay::Range(min, max))
            }
            _ => Err(serde::de::Error::custom("step_delay must be a number or [min, max] array")),
        }
    }
}

// --- Action structs ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub method: HttpMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_profile: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub options: FetchOptions,
}

impl FetchAction {
    pub fn resolve_mode(&self) -> Option<FetchMode> {
        FetchMode::resolve(self.url.as_deref(), self.index, self.text.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomQueryAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FillField {
    pub selector: String,
    pub value: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FillAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FillField>>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubmitAction {
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookiesAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetCookieAction {
    pub name: String,
    pub value: String,
    pub domain: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteCookieAction {
    pub name: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageGetAction {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageSetAction {
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageDeleteAction {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageListAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageClearAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

// --- Result envelope ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResultEnvelope {
    pub id: String,
    pub ok: bool,
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    /// Hint: `true` for transient errors (timeout, HTTP) worth retrying,
    /// `false` for permanent errors (bad command, missing page).
    /// Only present on error responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(flatten)]
    pub data: serde_json::Map<String, Value>,
}

impl ResultEnvelope {
    pub fn success(
        id: String,
        seq: u64,
        data: impl Serialize,
    ) -> Result<Self, serde_json::Error> {
        let value = serde_json::to_value(data)?;
        let map = match value {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        Ok(Self {
            id,
            ok: true,
            seq,
            error: None,
            code: None,
            retryable: None,
            data: map,
        })
    }

    pub fn error(id: String, seq: u64, code: ErrorCode, message: String) -> Self {
        let retryable = code.retryable();
        Self {
            id,
            ok: false,
            seq,
            error: Some(message),
            retryable: Some(retryable),
            code: Some(code),
            data: serde_json::Map::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::page::*;

    #[test]
    fn test_command_fetch_url_roundtrip() {
        let cmd = CommandEnvelope {
            id: "a".into(),
            v: 1,
            seq: 1,
            action: Action::Fetch(FetchAction {
                url: Some("https://example.com".into()),
                index: None,
                text: None,
                method: HttpMethod::Get,
                body: None,
                auth_profile: None,
                headers: HashMap::new(),
                options: FetchOptions::default(),
            }),
        };
        let json = serde_json::to_string(&cmd).unwrap();

        // Verify flat structure — url is at the top level, not nested
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["action"], "fetch");
        assert_eq!(value["url"], "https://example.com");
        assert_eq!(value["id"], "a");

        let back: CommandEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cmd);
    }

    #[test]
    fn test_command_fetch_from_spec() {
        let json = r#"{"id":"a","action":"fetch","v":1,"seq":1,"url":"https://news.ycombinator.com","options":{"max_tokens":2000}}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.id, "a");
        assert_eq!(cmd.v, 1);
        assert_eq!(cmd.seq, 1);
        if let Action::Fetch(ref f) = cmd.action {
            assert_eq!(f.url, Some("https://news.ycombinator.com".into()));
            assert_eq!(f.options.max_tokens, Some(2000));
            assert!(!f.options.strip_nav); // default
        } else {
            panic!("expected Fetch action");
        }
    }

    #[test]
    fn test_command_links_minimal() {
        let json = r#"{"id":"b","action":"links","v":1,"seq":2}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.id, "b");
        assert_eq!(cmd.seq, 2);
        assert_eq!(cmd.action, Action::Links);
    }

    #[test]
    fn test_command_dom_query_selector() {
        let json = r#"{"id":"d","action":"dom_query","v":1,"seq":4,"selector":"h1","attr":"textContent"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::DomQuery(ref dq) = cmd.action {
            assert_eq!(dq.selector, Some("h1".into()));
            assert_eq!(dq.attr, Some("textContent".into()));
            assert_eq!(dq.script, None);
        } else {
            panic!("expected DomQuery action");
        }
    }

    #[test]
    fn test_command_fetch_with_headers() {
        let json = r#"{"id":"x","action":"fetch","v":1,"seq":1,"url":"https://example.com","headers":{"Accept-Language":"en-US"}}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Fetch(ref f) = cmd.action {
            assert_eq!(f.headers.get("Accept-Language").unwrap(), "en-US");
        } else {
            panic!("expected Fetch action");
        }
    }

    #[test]
    fn test_command_fetch_method_body() {
        let json = r#"{"id":"x","action":"fetch","v":1,"seq":1,"url":"https://api.example.com/login","method":"POST","headers":{"Content-Type":"application/json"},"body":"{\"username\":\"agent\",\"password\":\"secret\"}"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Fetch(ref f) = cmd.action {
            assert_eq!(f.method, HttpMethod::Post);
            assert!(f.body.as_ref().unwrap().contains("username"));
            assert_eq!(f.headers.get("Content-Type").unwrap(), "application/json");
        } else {
            panic!("expected Fetch action");
        }
    }

    #[test]
    fn test_command_back_forward_info_quit() {
        for (json, expected) in [
            (
                r#"{"id":"a","action":"back","v":1,"seq":1}"#,
                Action::Back,
            ),
            (
                r#"{"id":"a","action":"forward","v":1,"seq":1}"#,
                Action::Forward,
            ),
            (
                r#"{"id":"a","action":"info","v":1,"seq":1}"#,
                Action::Info,
            ),
            (
                r#"{"id":"a","action":"quit","v":1,"seq":1}"#,
                Action::Quit,
            ),
        ] {
            let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
            assert_eq!(cmd.action, expected);
        }
    }

    #[test]
    fn test_command_cookies_optional_domain() {
        let json = r#"{"id":"a","action":"cookies","v":1,"seq":1}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Cookies(ref c) = cmd.action {
            assert_eq!(c.domain, None);
        } else {
            panic!("expected Cookies action");
        }

        let json = r#"{"id":"a","action":"cookies","v":1,"seq":1,"domain":"example.com"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Cookies(ref c) = cmd.action {
            assert_eq!(c.domain, Some("example.com".into()));
        } else {
            panic!("expected Cookies action");
        }
    }

    #[test]
    fn test_command_set_cookie_full() {
        let json = r#"{"id":"a","action":"set_cookie","v":1,"seq":1,"name":"token","value":"xyz","domain":"example.com","path":"/","secure":true,"http_only":false,"max_age_secs":3600,"sensitive":true}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::SetCookie(ref sc) = cmd.action {
            assert_eq!(sc.name, "token");
            assert_eq!(sc.value, "xyz");
            assert_eq!(sc.domain, "example.com");
            assert_eq!(sc.path, Some("/".into()));
            assert!(sc.secure);
            assert!(!sc.http_only);
            assert_eq!(sc.max_age_secs, Some(3600));
            assert!(sc.sensitive);
        } else {
            panic!("expected SetCookie action");
        }
    }

    #[test]
    fn test_command_fill_single_and_multi() {
        // Single field
        let json = r##"{"id":"a","action":"fill","v":1,"seq":1,"selector":"#username","value":"agent@example.com"}"##;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Fill(ref f) = cmd.action {
            assert_eq!(f.selector, Some("#username".into()));
            assert_eq!(f.value, Some("agent@example.com".into()));
            assert_eq!(f.fields, None);
        } else {
            panic!("expected Fill action");
        }

        // Multiple fields
        let json = r##"{"id":"a","action":"fill","v":1,"seq":1,"fields":[{"selector":"#user","value":"agent"},{"selector":"#pass","value":"secret","sensitive":true}]}"##;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Fill(ref f) = cmd.action {
            assert_eq!(f.selector, None);
            let fields = f.fields.as_ref().unwrap();
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].selector, "#user");
            assert!(!fields[0].sensitive);
            assert!(fields[1].sensitive);
        } else {
            panic!("expected Fill action");
        }
    }

    #[test]
    fn test_command_storage_actions() {
        let json = r#"{"id":"a","action":"storage_get","v":1,"seq":1,"key":"user_pref"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::StorageGet(ref sg) = cmd.action {
            assert_eq!(sg.key, "user_pref");
            assert_eq!(sg.origin, None);
        } else {
            panic!("expected StorageGet");
        }

        let json = r#"{"id":"a","action":"storage_set","v":1,"seq":1,"key":"k","value":"v","sensitive":true}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::StorageSet(ref ss) = cmd.action {
            assert_eq!(ss.key, "k");
            assert_eq!(ss.value, "v");
            assert!(ss.sensitive);
        } else {
            panic!("expected StorageSet");
        }

        let json = r#"{"id":"a","action":"storage_delete","v":1,"seq":1,"key":"k"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd.action, Action::StorageDelete(_)));

        let json = r#"{"id":"a","action":"storage_list","v":1,"seq":1}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd.action, Action::StorageList(_)));

        let json = r#"{"id":"a","action":"storage_clear","v":1,"seq":1}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd.action, Action::StorageClear(_)));
    }

    #[test]
    fn test_command_unknown_action_fails() {
        let json = r#"{"id":"x","action":"foobar","v":1,"seq":1}"#;
        let result = serde_json::from_str::<CommandEnvelope>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_unknown_fields_ignored() {
        let json = r#"{"id":"a","action":"links","v":1,"seq":1,"unknown_field":"hi","another":42}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.action, Action::Links);
    }

    #[test]
    fn test_result_envelope_success() {
        let page = PageResult {
            url: "https://example.com".into(),
            title: Some("Example".into()),
            status: 200,
            markdown: "# Example".into(),
            links: Some(vec![]),
            meta: PageMetadata::default(),
            stats: PageStats {
                fetch_ms: 100,
                tokens_est: 10,
                content_bytes: 500,
            },
            truncated: false,
            next_offset: None,
            content_selectors: None,
        };
        let env = ResultEnvelope::success("a".into(), 1, &page).unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify flat structure
        assert_eq!(value["id"], "a");
        assert_eq!(value["ok"], true);
        assert_eq!(value["seq"], 1);
        assert_eq!(value["url"], "https://example.com");
        assert_eq!(value["status"], 200);
        assert_eq!(value["markdown"], "# Example");
        assert!(value["error"].is_null());
        assert!(value["code"].is_null());
        assert!(value["retryable"].is_null()); // Not present on success
    }

    #[test]
    fn test_result_envelope_error() {
        let env =
            ResultEnvelope::error("x".into(), 1, ErrorCode::HttpError, "connection refused".into());
        let json = serde_json::to_string(&env).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["id"], "x");
        assert_eq!(value["ok"], false);
        assert_eq!(value["seq"], 1);
        assert_eq!(value["error"], "connection refused");
        assert_eq!(value["code"], "HTTP_ERROR");
        assert_eq!(value["retryable"], true);

        // Non-retryable error
        let env = ResultEnvelope::error(
            "y".into(),
            2,
            ErrorCode::InvalidCommand,
            "bad input".into(),
        );
        let json = serde_json::to_string(&env).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["retryable"], false);
    }

    #[test]
    fn test_result_envelope_matches_spec() {
        let links = LinksResult {
            links: vec![
                Link {
                    i: 0,
                    text: "Show HN".into(),
                    href: "https://example.com".into(),
                },
                Link {
                    i: 1,
                    text: "IANA".into(),
                    href: "https://www.iana.org/".into(),
                },
            ],
            count: 2,
        };
        let env = ResultEnvelope::success("b".into(), 2, &links).unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["count"], 2);
        assert_eq!(value["links"][0]["i"], 0);
        assert_eq!(value["links"][1]["text"], "IANA");
    }

    #[test]
    fn test_fetch_action_resolve_mode() {
        let action = FetchAction {
            url: Some("https://example.com".into()),
            index: Some(3),
            text: None,
            method: HttpMethod::default(),
            body: None,
            auth_profile: None,
            headers: HashMap::new(),
            options: FetchOptions::default(),
        };
        assert_eq!(
            action.resolve_mode(),
            Some(FetchMode::Url("https://example.com".into()))
        );

        let action = FetchAction {
            url: None,
            index: Some(5),
            text: Some("click".into()),
            method: HttpMethod::default(),
            body: None,
            auth_profile: None,
            headers: HashMap::new(),
            options: FetchOptions::default(),
        };
        assert_eq!(action.resolve_mode(), Some(FetchMode::Index(5)));
    }

    #[test]
    fn test_command_submit() {
        let json = r#"{"id":"a","action":"submit","v":1,"seq":1,"selector":"form#login","max_tokens":4000}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::Submit(ref s) = cmd.action {
            assert_eq!(s.selector, "form#login");
            assert_eq!(s.max_tokens, Some(4000));
        } else {
            panic!("expected Submit action");
        }
    }

    #[test]
    fn test_command_delete_cookie() {
        let json = r#"{"id":"a","action":"delete_cookie","v":1,"seq":1,"name":"token","domain":"example.com"}"#;
        let cmd: CommandEnvelope = serde_json::from_str(json).unwrap();
        if let Action::DeleteCookie(ref dc) = cmd.action {
            assert_eq!(dc.name, "token");
            assert_eq!(dc.domain, "example.com");
        } else {
            panic!("expected DeleteCookie action");
        }
    }
}
