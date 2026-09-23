//! Strict, hostile-input parsing of Deepgram server frames and close frames.
//!
//! Everything here is total: bad JSON, wrong types, missing fields and
//! oversized payloads produce `None` / a classified failure, never a panic.

use callcore_contract::ports::{SttFailure, SttFailureKind};
use serde::Deserialize;

/// Text frames larger than this are ignored without being parsed. Real
/// Deepgram Results frames are a few KiB.
pub const MAX_RESULT_FRAME_BYTES: usize = 1 << 20;

/// Close / error reasons quoted to the user are cut at this many chars.
pub const MAX_REASON_CHARS: usize = 200;

/// One recognized segment from a `Results` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSegment {
    pub transcript: String,
    pub is_final: bool,
}

#[derive(Deserialize)]
struct RawFrame {
    #[serde(rename = "type")]
    kind: Option<String>,
    is_final: Option<bool>,
    channel: Option<RawChannel>,
}

#[derive(Deserialize)]
struct RawChannel {
    alternatives: Option<Vec<RawAlternative>>,
}

#[derive(Deserialize)]
struct RawAlternative {
    transcript: Option<String>,
}

/// Parse a server text frame. Returns a segment only for a well-formed
/// `{"type":"Results","is_final":bool,"channel":{"alternatives":[{"transcript":str}]}}`.
/// Metadata / SpeechStarted / UtteranceEnd / unknown types, bad JSON, wrong
/// types, missing fields and frames over [`MAX_RESULT_FRAME_BYTES`] all yield
/// `None`.
pub fn parse_results(text: &str) -> Option<ResultSegment> {
    if text.len() > MAX_RESULT_FRAME_BYTES {
        return None;
    }
    let frame: RawFrame = serde_json::from_str(text).ok()?;
    if frame.kind.as_deref() != Some("Results") {
        return None;
    }
    let is_final = frame.is_final?;
    let transcript = frame
        .channel?
        .alternatives?
        .into_iter()
        .next()?
        .transcript?;
    Some(ResultSegment {
        transcript,
        is_final,
    })
}

/// Make server-controlled text safe to show: control characters become
/// spaces, runs of whitespace collapse, cut at [`MAX_REASON_CHARS`] chars.
pub fn sanitize_reason(raw: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut last_space = true;
    for c in raw.chars() {
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        if count == MAX_REASON_CHARS {
            break;
        }
        out.push(c);
        count += 1;
    }
    out.trim_end().to_string()
}

/// Close codes Deepgram (and proxies in front of it) use for auth problems.
pub fn is_auth_close_code(code: u16) -> bool {
    matches!(code, 1008 | 4001 | 4003 | 4008)
}

/// What a close frame means for the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Normal close after CloseStream: the transcript is complete.
    Flushed,
    Failed(SttFailure),
}

pub const UNEXPECTED_END: &str = "Deepgram ended the stream unexpectedly";

/// Classify a close frame. `code == None` (no status) counts as a normal close.
pub fn classify_close(code: Option<u16>, reason: &str, close_requested: bool) -> CloseOutcome {
    let code = match code {
        None | Some(1000) => {
            return if close_requested {
                CloseOutcome::Flushed
            } else {
                CloseOutcome::Failed(SttFailure {
                    kind: SttFailureKind::Server,
                    message: UNEXPECTED_END.to_string(),
                })
            };
        }
        Some(c) => c,
    };
    let reason = sanitize_reason(reason);
    let message = if reason.is_empty() {
        format!("Deepgram closed the connection (code {code})")
    } else {
        format!("Deepgram closed the connection (code {code}: {reason})")
    };
    let kind = if is_auth_close_code(code) {
        SttFailureKind::BadKey
    } else {
        SttFailureKind::Server
    };
    CloseOutcome::Failed(SttFailure { kind, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"type":"Results","channel_index":[0,1],"duration":1.2,"start":0.0,"is_final":true,"speech_final":true,"channel":{"alternatives":[{"transcript":"hello there","confidence":0.98,"words":[]}]},"metadata":{"request_id":"x"}}"#;

    #[test]
    fn parses_a_real_results_frame() {
        assert_eq!(
            parse_results(GOOD),
            Some(ResultSegment {
                transcript: "hello there".into(),
                is_final: true
            })
        );
        let interim = GOOD.replace(r#""is_final":true"#, r#""is_final":false"#);
        assert!(!parse_results(&interim).unwrap().is_final);
    }

    #[test]
    fn ignores_non_results_types() {
        for t in [
            r#"{"type":"Metadata","request_id":"abc","channels":1}"#,
            r#"{"type":"SpeechStarted","timestamp":1.0}"#,
            r#"{"type":"UtteranceEnd","last_word_end":2.0}"#,
            r#"{"type":"SomethingNew","channel":{"alternatives":[{"transcript":"x"}]},"is_final":true}"#,
            r#"{"channel":{"alternatives":[{"transcript":"x"}]},"is_final":true}"#,
        ] {
            assert_eq!(parse_results(t), None, "{t}");
        }
    }

    #[test]
    fn ignores_bad_json_wrong_types_and_missing_fields() {
        for t in [
            "",
            "not json",
            "{",
            "null",
            "[]",
            "5",
            r#"{"channel":5}"#,
            r#"{"type":5}"#,
            r#"{"type":"Results","is_final":"yes","channel":{"alternatives":[{"transcript":"x"}]}}"#,
            r#"{"type":"Results","channel":{"alternatives":[{"transcript":"x"}]}}"#,
            r#"{"type":"Results","is_final":true}"#,
            r#"{"type":"Results","is_final":true,"channel":5}"#,
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":5}}"#,
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[]}}"#,
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[5]}}"#,
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":7}]}}"#,
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{}]}}"#,
            r#"{"type":"Results","is_final":null,"channel":{"alternatives":[{"transcript":"x"}]}}"#,
        ] {
            assert_eq!(parse_results(t), None, "{t}");
        }
    }

    #[test]
    fn ignores_oversized_frames_even_if_valid() {
        let pad = "a".repeat(MAX_RESULT_FRAME_BYTES);
        let big = GOOD.replace("hello there", &pad);
        assert!(big.len() > MAX_RESULT_FRAME_BYTES);
        assert_eq!(parse_results(&big), None);
    }

    #[test]
    fn deeply_nested_json_does_not_overflow_the_stack() {
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert_eq!(parse_results(&deep), None);
        let deep_obj = format!(
            r#"{{"type":"Results","is_final":true,"channel":{}{}}}"#,
            r#"{"a":"#.repeat(50_000),
            "}".repeat(50_000)
        );
        assert_eq!(parse_results(&deep_obj), None);
    }

    #[test]
    fn sanitize_reason_strips_controls_and_caps_length() {
        assert_eq!(sanitize_reason("bad\r\nkey\t\u{0}here  "), "bad key here");
        let long = "é".repeat(500);
        let s = sanitize_reason(&long);
        assert_eq!(s.chars().count(), MAX_REASON_CHARS);
        assert_eq!(sanitize_reason(""), "");
    }

    #[test]
    fn classify_close_codes() {
        assert_eq!(classify_close(Some(1000), "", true), CloseOutcome::Flushed);
        assert_eq!(classify_close(None, "", true), CloseOutcome::Flushed);
        match classify_close(Some(1000), "", false) {
            CloseOutcome::Failed(f) => {
                assert_eq!(f.kind, SttFailureKind::Server);
                assert_eq!(f.message, UNEXPECTED_END);
            }
            other => panic!("{other:?}"),
        }
        for code in [1008u16, 4001, 4003, 4008] {
            match classify_close(Some(code), "Invalid credentials", true) {
                CloseOutcome::Failed(f) => {
                    assert_eq!(f.kind, SttFailureKind::BadKey);
                    assert_eq!(
                        f.message,
                        format!(
                            "Deepgram closed the connection (code {code}: Invalid credentials)"
                        )
                    );
                }
                other => panic!("{other:?}"),
            }
        }
        match classify_close(Some(1011), "", true) {
            CloseOutcome::Failed(f) => {
                assert_eq!(f.kind, SttFailureKind::Server);
                assert_eq!(f.message, "Deepgram closed the connection (code 1011)");
            }
            other => panic!("{other:?}"),
        }
    }
}
