//! Failure -> closed `ErrorCode` mapping (spec §10).

use callcore_contract::ports::{
    AudioError, ProviderFailure, ProviderFailureKind, SttFailure, SttFailureKind,
};
use callcore_contract::{copy, AppError, ErrorCode};

/// Copy used when a provider failure arrives without a message.
const PROVIDER_FALLBACK: &str = "The answer provider failed. Try again.";
/// Copy for a whitespace-only answer that a provider reported as success.
pub(crate) const EMPTY_ANSWER: &str =
    "The answer provider returned an empty answer. Try again, or switch the answer provider in Settings.";
/// Copy for a Flushed/close that arrived before the recording finished.
pub(crate) const STT_CLOSED_EARLY: &str =
    "Deepgram closed the connection before the transcript was final. Try again.";
/// Copy for a prompt/request build that panicked.
pub(crate) const BUILD_FAILED: &str =
    "Could not build the answer request. Copy diagnostics from Settings and report it.";
/// Copy for a key read whose blocking task died.
pub(crate) const KEY_READ_FAILED: &str = "Could not read the saved API keys. Try again.";

/// Provider failure -> `AppError` (spec §10): Auth -> `llm_auth`,
/// RateLimit -> `llm_rate_limit`, Aborted -> `aborted`, every other kind ->
/// `llm_http`. The message is the provider's own actionable copy.
pub fn provider_failure_to_app_error(failure: &ProviderFailure) -> AppError {
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
    let message = if failure.message.trim().is_empty() {
        match failure.kind {
            ProviderFailureKind::Aborted => "Cancelled.",
            ProviderFailureKind::EmptyAnswer => EMPTY_ANSWER,
            _ => PROVIDER_FALLBACK,
        }
        .to_string()
    } else {
        failure.message.clone()
    };
    AppError::new(code, message)
}

/// STT failure before the final transcript -> `AppError` (spec §10):
/// BadKey keeps its "code 1008" message; Connect -> canonical connect copy;
/// Server/Closed -> `stt_error`.
pub fn stt_failure_to_app_error(failure: &SttFailure) -> AppError {
    match failure.kind {
        SttFailureKind::BadKey => AppError::new(
            ErrorCode::SttConnect,
            non_empty_or(&failure.message, copy::STT_CONNECT),
        ),
        SttFailureKind::Connect => AppError::new(ErrorCode::SttConnect, copy::STT_CONNECT),
        SttFailureKind::Server | SttFailureKind::Closed => AppError::new(
            ErrorCode::SttError,
            non_empty_or(&failure.message, STT_CLOSED_EARLY),
        ),
    }
}

/// Audio start failure -> `internal` with the device copy (spec §10).
pub(crate) fn audio_error_to_app_error(err: &AudioError) -> AppError {
    match err {
        AudioError::DeviceOpen(m) => AppError::internal(non_empty_or(m, copy::DEVICE_OPEN)),
        AudioError::WorkerGone | AudioError::Other(_) => AppError::internal(copy::DEVICE_OPEN),
    }
}

fn non_empty_or(message: &str, fallback: &str) -> String {
    if message.trim().is_empty() {
        fallback.to_string()
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf(kind: ProviderFailureKind) -> ProviderFailure {
        ProviderFailure {
            kind,
            message: "boom (500)".into(),
        }
    }

    #[test]
    fn provider_mapping_is_the_closed_set() {
        use ProviderFailureKind::*;
        assert_eq!(
            provider_failure_to_app_error(&pf(Auth { status: 401 })).code,
            ErrorCode::LlmAuth
        );
        assert_eq!(
            provider_failure_to_app_error(&pf(RateLimit { status: 429 })).code,
            ErrorCode::LlmRateLimit
        );
        assert_eq!(
            provider_failure_to_app_error(&pf(Aborted)).code,
            ErrorCode::Aborted
        );
        for k in [
            Connect,
            Http { status: 500 },
            StreamDrop,
            ProviderError,
            Incomplete,
            EmptyAnswer,
            ModelUnavailable,
        ] {
            let e = provider_failure_to_app_error(&pf(k));
            assert_eq!(e.code, ErrorCode::LlmHttp, "{k:?}");
            assert_eq!(e.message, "boom (500)");
        }
    }

    #[test]
    fn provider_mapping_fills_empty_messages() {
        let e = provider_failure_to_app_error(&ProviderFailure {
            kind: ProviderFailureKind::EmptyAnswer,
            message: " ".into(),
        });
        assert_eq!(e.message, EMPTY_ANSWER);
    }

    #[test]
    fn stt_mapping() {
        let bad = SttFailure {
            kind: SttFailureKind::BadKey,
            message: "Deepgram closed the connection (code 1008)".into(),
        };
        let e = stt_failure_to_app_error(&bad);
        assert_eq!(e.code, ErrorCode::SttConnect);
        assert!(e.message.contains("1008"));
        let c = SttFailure {
            kind: SttFailureKind::Connect,
            message: "dns".into(),
        };
        assert_eq!(stt_failure_to_app_error(&c).message, copy::STT_CONNECT);
        for k in [SttFailureKind::Server, SttFailureKind::Closed] {
            let e = stt_failure_to_app_error(&SttFailure {
                kind: k,
                message: "x".into(),
            });
            assert_eq!(e.code, ErrorCode::SttError);
        }
    }

    #[test]
    fn audio_mapping() {
        let e = audio_error_to_app_error(&AudioError::DeviceOpen(copy::DEVICE_OPEN.into()));
        assert_eq!(e, AppError::internal(copy::DEVICE_OPEN));
        let e = audio_error_to_app_error(&AudioError::WorkerGone);
        assert_eq!(e, AppError::internal(copy::DEVICE_OPEN));
    }
}
