use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnknownAction,
    InvalidCommand,
    HttpError,
    Timeout,
    InvalidUrl,
    NoPage,
    NoHistory,
    LinkNotFound,
    DomQueryError,
    SelectorNotFound,
    FormNotFound,
    CookieError,
    StorageError,
    AuthProfileNotFound,
    AuthProfileDomainMismatch,
    SessionError,
}

impl ErrorCode {
    /// Whether an error with this code is worth retrying.
    ///
    /// Retryable: transient network/server failures (timeout, HTTP errors).
    /// Not retryable: client mistakes (bad command, missing page, invalid URL).
    pub fn retryable(&self) -> bool {
        matches!(self, ErrorCode::Timeout | ErrorCode::HttpError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_serialization() {
        assert_eq!(
            serde_json::to_string(&ErrorCode::HttpError).unwrap(),
            "\"HTTP_ERROR\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorCode::UnknownAction).unwrap(),
            "\"UNKNOWN_ACTION\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorCode::AuthProfileDomainMismatch).unwrap(),
            "\"AUTH_PROFILE_DOMAIN_MISMATCH\""
        );
    }

    #[test]
    fn test_error_code_roundtrip() {
        let codes = [
            ErrorCode::UnknownAction,
            ErrorCode::InvalidCommand,
            ErrorCode::HttpError,
            ErrorCode::Timeout,
            ErrorCode::InvalidUrl,
            ErrorCode::NoPage,
            ErrorCode::NoHistory,
            ErrorCode::LinkNotFound,
            ErrorCode::DomQueryError,
            ErrorCode::SelectorNotFound,
            ErrorCode::FormNotFound,
            ErrorCode::CookieError,
            ErrorCode::StorageError,
            ErrorCode::AuthProfileNotFound,
            ErrorCode::AuthProfileDomainMismatch,
            ErrorCode::SessionError,
        ];
        for code in &codes {
            let json = serde_json::to_string(code).unwrap();
            let back: ErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, code);
        }
    }

    #[test]
    fn test_error_code_from_string() {
        let code: ErrorCode = serde_json::from_str("\"UNKNOWN_ACTION\"").unwrap();
        assert_eq!(code, ErrorCode::UnknownAction);

        let code: ErrorCode = serde_json::from_str("\"DOM_QUERY_ERROR\"").unwrap();
        assert_eq!(code, ErrorCode::DomQueryError);
    }

    #[test]
    fn test_retryable_codes() {
        assert!(ErrorCode::Timeout.retryable());
        assert!(ErrorCode::HttpError.retryable());
        assert!(!ErrorCode::InvalidCommand.retryable());
        assert!(!ErrorCode::InvalidUrl.retryable());
        assert!(!ErrorCode::NoPage.retryable());
        assert!(!ErrorCode::NoHistory.retryable());
        assert!(!ErrorCode::LinkNotFound.retryable());
        assert!(!ErrorCode::DomQueryError.retryable());
        assert!(!ErrorCode::SessionError.retryable());
        assert!(!ErrorCode::UnknownAction.retryable());
    }

}
