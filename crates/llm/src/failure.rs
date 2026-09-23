//! Provider failure construction (user copy) and the spec §10 mapping to the
//! closed `ErrorCode` set.
//!
//! Every message here is user-facing: status code quoted, at most a
//! 200-character provider snippet, never a Debug dump, never the key.

use callcore_contract::config::PROVIDER_SNIPPET_CHARS;
use callcore_contract::ports::{ProviderFailure, ProviderFailureKind};
use callcore_contract::{AppError, ErrorCode};

/// Spec §10: Auth→`llm_auth`, RateLimit→`llm_rate_limit`, Aborted→`aborted`,
/// everything else→`llm_http`. The message is carried over unchanged (it is
/// already kind-specific user copy).
pub fn failure_to_app_error(failure: &ProviderFailure) -> AppError {
    let code = match failure.kind {
        ProviderFailureKind::Auth { .. } => ErrorCode::LlmAuth,
        ProviderFailureKind::RateLimit { .. } => ErrorCode::LlmRateLimit,
        ProviderFailureKind::Aborted => ErrorCode::Aborted,
        ProviderFailureKind::Connect
        | ProviderFailureKind::Http { .. }
        | ProviderFailureKind::StreamDrop
        | ProviderFailureKind::ProviderError
        | ProviderFailureKind::Incomplete
        | ProviderFailureKind::EmptyAnswer
        | ProviderFailureKind::ModelUnavailable => ErrorCode::LlmHttp,
    };
    AppError::new(code, failure.message.clone())
}

pub(crate) fn fail(kind: ProviderFailureKind, message: impl Into<String>) -> ProviderFailure {
    ProviderFailure {
        kind,
        message: message.into(),
    }
}

/// Turn a provider error body (JSON or text) into a quotable snippet:
/// prefers `error.message` (then `message`, then `error` as a string), strips
/// control characters, collapses whitespace, redacts every secret, and cuts
/// to [`PROVIDER_SNIPPET_CHARS`] characters.
pub(crate) fn snippet(body: &str, secrets: &[String]) -> String {
    let extracted = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .or_else(|| v.get("error").and_then(|m| m.as_str()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.to_owned());
    clean(&extracted, secrets)
}

/// Strip control chars, collapse whitespace, redact secrets, truncate.
pub(crate) fn clean(text: &str, secrets: &[String]) -> String {
    let mut redacted = text.to_owned();
    for s in secrets {
        if !s.is_empty() {
            redacted = redacted.replace(s.as_str(), "***");
        }
    }
    let mut collapsed = String::with_capacity(redacted.len());
    let mut pending_space = false;
    for ch in redacted.chars() {
        if ch.is_control() || ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            pending_space = false;
            collapsed.push(ch);
        }
    }
    if collapsed.chars().count() > PROVIDER_SNIPPET_CHARS {
        let mut cut: String = collapsed.chars().take(PROVIDER_SNIPPET_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_error_mapping_matches_spec_section_10() {
        let cases = [
            (
                ProviderFailureKind::Auth { status: 401 },
                ErrorCode::LlmAuth,
            ),
            (
                ProviderFailureKind::RateLimit { status: 529 },
                ErrorCode::LlmRateLimit,
            ),
            (ProviderFailureKind::Aborted, ErrorCode::Aborted),
            (ProviderFailureKind::Connect, ErrorCode::LlmHttp),
            (
                ProviderFailureKind::Http { status: 500 },
                ErrorCode::LlmHttp,
            ),
            (ProviderFailureKind::StreamDrop, ErrorCode::LlmHttp),
            (ProviderFailureKind::ProviderError, ErrorCode::LlmHttp),
            (ProviderFailureKind::Incomplete, ErrorCode::LlmHttp),
            (ProviderFailureKind::EmptyAnswer, ErrorCode::LlmHttp),
            (ProviderFailureKind::ModelUnavailable, ErrorCode::LlmHttp),
        ];
        for (kind, code) in cases {
            let e = failure_to_app_error(&fail(kind, "msg"));
            assert_eq!(e.code, code, "{kind:?}");
            assert_eq!(e.message, "msg");
        }
    }

    #[test]
    fn snippet_prefers_json_error_message() {
        let body =
            r#"{"type":"error","error":{"type":"api_error","message":"Internal\n\tfailure"}}"#;
        assert_eq!(snippet(body, &[]), "Internal failure");
        assert_eq!(snippet(r#"{"message":"top"}"#, &[]), "top");
        assert_eq!(snippet("plain <b>text</b>", &[]), "plain <b>text</b>");
    }

    #[test]
    fn snippet_redacts_key_strips_control_chars_and_truncates() {
        let key = "sk-secret-123".to_owned();
        let body = format!("{{\"error\":{{\"message\":\"bad key {key}\\u0007 here\"}}}}");
        let s = snippet(&body, std::slice::from_ref(&key));
        assert_eq!(s, "bad key *** here");
        let long = "x".repeat(1000);
        let s = snippet(&long, &[]);
        assert_eq!(s.chars().count(), PROVIDER_SNIPPET_CHARS);
        assert!(s.ends_with('…'));
        // A key sitting across the truncation boundary is still redacted.
        let body = format!("{}{key}", "y".repeat(198));
        let s = snippet(&body, std::slice::from_ref(&key));
        assert!(!s.contains("sk-secret"), "{s}");
    }
}
