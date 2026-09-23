//! "Copy diagnostics" text (spec §9 get_diagnostics, §17). Never contains
//! secrets or prompt/profile text: the inputs are status values only, and the
//! whole output goes through [`redact`] as a last line of defence (log lines
//! are the one free-text input).

use callcore_contract::{
    AppError, BuildInfo, CoreState, HotkeyStatus, Protection, SettingsLoadStatus,
};

use crate::events::PumpStats;

pub const REDACTED: &str = "[redacted]";
/// Runs of hex/base64-ish characters at least this long are treated as keys.
pub const LONG_RUN: usize = 32;
/// At most this many log lines are included.
pub const MAX_LOG_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct DiagnosticsInput<'a> {
    pub build: &'a BuildInfo,
    pub os: &'a str,
    pub protection: Protection,
    pub core: CoreState,
    pub core_error: Option<&'a AppError>,
    pub settings_load: SettingsLoadStatus,
    pub settings_writes_blocked: bool,
    pub hotkey_status: HotkeyStatus,
    pub hotkey_message: Option<&'a str>,
    pub events: Option<PumpStats>,
    pub log_lines: &'a [String],
}

fn is_key_char(c: char) -> bool {
    // '=' is NOT part of a token ("key=sk-…" must split); base64 padding
    // after a redacted run is swallowed by `redact`.
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/')
}

fn looks_secret(tok: &str) -> bool {
    let lower = tok.to_ascii_lowercase();
    // Provider key prefixes: Anthropic/OpenAI "sk-…", Groq "gsk_…".
    if (lower.starts_with("sk-") || lower.starts_with("sk_") || lower.starts_with("gsk_"))
        && tok.len() >= 8
    {
        return true;
    }
    // Only the alphanumeric core counts for the long-run rule (paths like
    // "a/b/c" split by '/' are not keys, but base64 blobs contain '/').
    let core_len = tok.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    if core_len < LONG_RUN {
        return false;
    }
    let has_digit = tok.chars().any(|c| c.is_ascii_digit());
    let has_alpha = tok.chars().any(|c| c.is_ascii_alphabetic());
    let all_hex = tok.chars().all(|c| c.is_ascii_hexdigit());
    all_hex || (has_digit && has_alpha)
}

/// Replace anything resembling an API key with `[redacted]`.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut tok = String::new();
    // True right after a token was redacted: trailing '=' padding is dropped.
    let mut after_secret = false;
    let flush = |tok: &mut String, out: &mut String| -> bool {
        let mut secret = false;
        if !tok.is_empty() {
            secret = looks_secret(tok);
            out.push_str(if secret { REDACTED } else { tok.as_str() });
            tok.clear();
        }
        secret
    };
    for c in text.chars() {
        if is_key_char(c) {
            after_secret = false;
            tok.push(c);
        } else {
            if !tok.is_empty() {
                after_secret = flush(&mut tok, &mut out);
            }
            if c == '=' && after_secret {
                continue;
            }
            after_secret = false;
            out.push(c);
        }
    }
    flush(&mut tok, &mut out);
    out
}

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "?".into())
}

pub fn render(input: &DiagnosticsInput<'_>) -> String {
    let b = input.build;
    let mut s = String::new();
    s.push_str("AI Call Assistant diagnostics\n");
    s.push_str(&format!("version: {}\n", b.version));
    s.push_str(&format!(
        "build: {}{} @ {}\n",
        b.git_revision,
        if b.dirty { " (dirty)" } else { "" },
        b.build_time
    ));
    s.push_str(&format!("os: {}\n", input.os));
    s.push_str(&format!("protection: {}\n", enum_str(&input.protection)));
    s.push_str(&format!("core: {}\n", enum_str(&input.core)));
    if let Some(e) = input.core_error {
        s.push_str(&format!(
            "core error: {} — {}\n",
            enum_str(&e.code),
            e.message
        ));
    }
    s.push_str(&format!(
        "settings: {}{}\n",
        enum_str(&input.settings_load),
        if input.settings_writes_blocked {
            " (writes blocked)"
        } else {
            ""
        }
    ));
    s.push_str(&format!("hotkey: {}", enum_str(&input.hotkey_status)));
    if let Some(m) = input.hotkey_message {
        s.push_str(&format!(" — {m}"));
    }
    s.push('\n');
    if let Some(e) = input.events {
        s.push_str(&format!(
            "events: attached={} queued={} delivered={} failures={} attaches={} levels_coalesced={} levels_dropped={} overflow_dropped={}\n",
            e.attached, e.queued, e.delivered, e.delivery_failures, e.attaches, e.coalesced_levels, e.dropped_levels, e.dropped_overflow
        ));
    }
    let lines = input.log_lines;
    let start = lines.len().saturating_sub(MAX_LOG_LINES);
    s.push_str(&format!("\nrecent log ({} lines):\n", lines.len() - start));
    for l in &lines[start..] {
        s.push_str(l.trim_end());
        s.push('\n');
    }
    redact(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callcore_contract::ErrorCode;

    fn build() -> BuildInfo {
        BuildInfo {
            version: "4.0.0".into(),
            git_revision: "abc1234".into(),
            dirty: true,
            build_time: "2026-09-23T10:00:00Z".into(),
        }
    }

    #[test]
    fn redacts_provider_keys() {
        let s = redact("key=sk-ant-api03-AbCdEf123 and gsk_ZZZZyyyy1234 ok");
        assert!(!s.contains("sk-ant"), "{s}");
        assert!(!s.contains("gsk_"), "{s}");
        assert!(s.contains("ok"));
        assert_eq!(s.matches(REDACTED).count(), 2);
    }

    #[test]
    fn redacts_long_hex_and_base64_runs() {
        let dg = "0123456789abcdef0123456789abcdef01234567"; // Deepgram-style
        let b64 = "QWxhZGRpbjpvcGVuIHNlc2FtZQ9876543210+/abcdXYZ==";
        let s = redact(&format!("token {dg} blob {b64} end"));
        assert!(!s.contains(dg) && !s.contains(b64), "{s}");
        assert_eq!(s, format!("token {REDACTED} blob {REDACTED} end"));
    }

    #[test]
    fn keeps_ordinary_text() {
        let t = "2026-09-23T10:00:00Z INFO session s3 phase=recording C:\\Users\\Owner\\AppData\\Roaming\\AICallAssistant\\settings.json";
        assert_eq!(redact(t), t);
        // Long word without digits is not a key.
        let w = "Supercalifragilisticexpialidociousandthensome";
        assert_eq!(redact(w), w);
    }

    #[test]
    fn render_includes_status_and_redacts_logs() {
        let logs = vec![
            "line one".to_string(),
            "leaked Bearer sk-proj-SECRETSECRET123".to_string(),
        ];
        let err = AppError::new(ErrorCode::Internal, "boom");
        let out = render(&DiagnosticsInput {
            build: &build(),
            os: "Windows 10.0.26200",
            protection: Protection::Protected,
            core: CoreState::Failed,
            core_error: Some(&err),
            settings_load: SettingsLoadStatus::Corrupt,
            settings_writes_blocked: true,
            hotkey_status: HotkeyStatus::Unavailable,
            hotkey_message: Some("Ctrl+Shift+Space is already taken by another app."),
            events: Some(PumpStats::default()),
            log_lines: &logs,
        });
        for want in [
            "version: 4.0.0",
            "abc1234 (dirty)",
            "os: Windows 10.0.26200",
            "protection: protected",
            "core: failed",
            "core error: internal — boom",
            "settings: corrupt (writes blocked)",
            "hotkey: unavailable — Ctrl+Shift+Space",
            "events: attached=false",
            "line one",
        ] {
            assert!(out.contains(want), "missing {want:?} in\n{out}");
        }
        assert!(!out.contains("SECRETSECRET"), "{out}");
    }

    #[test]
    fn render_caps_log_lines() {
        let logs: Vec<String> = (0..500).map(|i| format!("l{i}")).collect();
        let out = render(&DiagnosticsInput {
            build: &build(),
            os: "x",
            protection: Protection::Unknown,
            core: CoreState::Ready,
            core_error: None,
            settings_load: SettingsLoadStatus::Ok,
            settings_writes_blocked: false,
            hotkey_status: HotkeyStatus::Registered,
            hotkey_message: None,
            events: None,
            log_lines: &logs,
        });
        assert!(out.contains("recent log (200 lines)"));
        assert!(out.contains("l499") && !out.contains("l299\n"));
    }
}
