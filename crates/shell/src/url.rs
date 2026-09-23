//! `open_external` URL validation (spec §9): https only, host required, no
//! whitespace/control characters, ≤ 2048 chars, no userinfo.

use callcore_contract::AppError;

pub const MAX_URL_LEN: usize = 2048;

/// Characters that are never allowed anywhere in the URL: they could split a
/// process argument, smuggle a different host, or visually spoof one.
fn forbidden(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || matches!(c, '"' | '<' | '>' | '\\' | '^' | '{' | '}' | '|')
        // Unicode format characters: bidi overrides/isolates, zero-width.
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// Validate and return the URL unchanged. Errors carry user copy (code
/// `internal`, which the page shows as a plain message).
pub fn validate_external(url: &str) -> Result<String, AppError> {
    let bad = |why: &str| AppError::internal(format!("That link can't be opened: {why}."));
    if url.is_empty() {
        return Err(bad("it is empty"));
    }
    if url.len() > MAX_URL_LEN {
        return Err(bad("it is longer than 2048 characters"));
    }
    if url.chars().any(forbidden) {
        return Err(bad("it contains spaces or control characters"));
    }
    let Some(prefix) = url.get(..8) else {
        return Err(bad("only https links are allowed"));
    };
    if !prefix.eq_ignore_ascii_case("https://") {
        return Err(bad("only https links are allowed"));
    }
    let rest = &url[8..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(bad("it contains a user name or password"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        // IPv6 literal without port: "[::1]"
        Some((h, p)) if !p.contains(']') => (h, Some(p)),
        _ => (authority, None),
    };
    if host.is_empty() || host.starts_with('.') || host.contains("..") {
        return Err(bad("it has no host"));
    }
    let host_ok = if host.starts_with('[') {
        host.ends_with(']') && host.len() > 2
    } else {
        host.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '_')
    };
    if !host_ok {
        return Err(bad("the host name is not valid"));
    }
    if let Some(p) = port {
        if p.is_empty()
            || p.len() > 5
            || !p.chars().all(|c| c.is_ascii_digit())
            || p.parse::<u32>().map_or(true, |n| n > 65_535)
        {
            return Err(bad("the port is not valid"));
        }
    }
    Ok(url.to_string())
}

/// Navigation policy: the only origins the webview may load are the bundled
/// app (`tauri://localhost`, `http(s)://tauri.localhost` on Windows) and the
/// Vite dev server (`http://localhost:5173`). Everything else is blocked.
pub fn is_app_origin(scheme: &str, host: Option<&str>, port: Option<u16>) -> bool {
    match scheme.to_ascii_lowercase().as_str() {
        "tauri" => host == Some("localhost"),
        "http" | "https" => match host {
            Some(h) if h.eq_ignore_ascii_case("tauri.localhost") => port.is_none(),
            Some(h) if h.eq_ignore_ascii_case("localhost") => port == Some(5173),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_origins_are_allowed() {
        assert!(is_app_origin("tauri", Some("localhost"), None));
        assert!(is_app_origin("http", Some("tauri.localhost"), None));
        assert!(is_app_origin("https", Some("tauri.localhost"), None));
        assert!(is_app_origin("http", Some("localhost"), Some(5173)));
    }

    #[test]
    fn everything_else_is_blocked() {
        assert!(!is_app_origin("https", Some("example.com"), None));
        assert!(!is_app_origin("http", Some("localhost"), Some(8080)));
        assert!(!is_app_origin("http", Some("localhost"), None));
        assert!(!is_app_origin("http", Some("tauri.localhost"), Some(81)));
        assert!(!is_app_origin("tauri", Some("evil"), None));
        assert!(!is_app_origin("file", None, None));
        assert!(!is_app_origin("javascript", None, None));
        assert!(!is_app_origin("data", None, None));
    }

    fn ok(u: &str) {
        assert_eq!(
            validate_external(u).as_deref(),
            Ok(u),
            "{u:?} should be accepted"
        );
    }
    fn rejected(u: &str) {
        assert!(validate_external(u).is_err(), "{u:?} should be rejected");
    }

    #[test]
    fn accepts_plain_https() {
        ok("https://console.deepgram.com/signup");
        ok("https://console.anthropic.com/settings/keys?x=1#top");
        ok("HTTPS://Example.COM");
        ok("https://example.com:8443/path");
        ok("https://[::1]/");
        ok("https://example.com");
    }

    #[test]
    fn rejects_other_schemes() {
        rejected("javascript:alert(1)");
        rejected("http://example.com");
        rejected("file:///C:/Windows/System32/calc.exe");
        rejected("ms-settings:privacy");
        rejected("mailto:a@b.c");
        rejected("//example.com");
        rejected("");
    }

    #[test]
    fn rejects_missing_host() {
        rejected("https://");
        rejected("https:///path");
        rejected("https://?q");
        rejected("https://:443/");
        rejected("https://..");
    }

    #[test]
    fn rejects_whitespace_and_controls() {
        rejected("https://a b");
        rejected("https://example.com/a b");
        rejected("https://example.com/\n");
        rejected("https://example.com/\t");
        rejected("https://example.com/\u{0007}");
        rejected("https://example.com/\u{0085}");
        rejected("https://example.com/\u{00A0}");
        rejected("https://example.com/\u{202E}gpj.exe");
        rejected("https://exa\u{200B}mple.com");
        rejected("https://example.com/\"x");
        rejected("https://example.com\\@evil.com");
    }

    #[test]
    fn rejects_userinfo() {
        rejected("https://user:pass@example.com/");
        rejected("https://google.com@evil.com/");
        // '@' after the path is fine (not userinfo).
        ok("https://example.com/users/@me");
    }

    #[test]
    fn length_limit() {
        let base = "https://example.com/";
        let at_limit = format!("{base}{}", "a".repeat(MAX_URL_LEN - base.len()));
        ok(&at_limit);
        rejected(&format!("{at_limit}a"));
    }

    #[test]
    fn bad_ports() {
        rejected("https://example.com:99999/");
        rejected("https://example.com:abc/");
        rejected("https://example.com:/");
    }

    #[test]
    fn multibyte_prefix_does_not_panic() {
        rejected("httpsé//x");
        rejected("né");
    }
}
