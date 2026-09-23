//! Build info (spec §17): version, git revision, dirty flag (INCLUDING
//! untracked files), build time. `src-tauri/build.rs` sets `AICA_GIT_REV`,
//! `AICA_GIT_DIRTY`, `AICA_BUILD_TIME`; use the [`build_info!`] macro in that
//! crate so `option_env!` expands where those variables exist.

use callcore_contract::BuildInfo;

pub const UNKNOWN: &str = "unknown";

/// Assemble from raw env values; missing/blank values become "unknown" /
/// not-dirty.
pub fn from_parts(
    version: &str,
    git_rev: Option<&str>,
    dirty: Option<&str>,
    build_time: Option<&str>,
) -> BuildInfo {
    let clean = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(UNKNOWN)
            .to_string()
    };
    BuildInfo {
        version: version.to_string(),
        git_revision: clean(git_rev),
        dirty: matches!(dirty.map(str::trim), Some("true") | Some("1")),
        build_time: clean(build_time),
    }
}

/// Expands `option_env!` in the CALLING crate (whose build.rs set the vars).
#[macro_export]
macro_rules! build_info {
    () => {
        $crate::build_info::from_parts(
            env!("CARGO_PKG_VERSION"),
            option_env!("AICA_GIT_REV"),
            option_env!("AICA_GIT_DIRTY"),
            option_env!("AICA_BUILD_TIME"),
        )
    };
}

/// Format seconds since the Unix epoch as UTC ISO-8601 (`YYYY-MM-DDTHH:MM:SSZ`).
/// Shared with build.rs logic (which re-implements it; build scripts cannot
/// depend on this crate cheaply) and used for log timestamps.
pub fn iso8601_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_values_are_unknown() {
        let b = from_parts("4.0.0", None, None, Some("  "));
        assert_eq!(b.version, "4.0.0");
        assert_eq!(b.git_revision, "unknown");
        assert!(!b.dirty);
        assert_eq!(b.build_time, "unknown");
    }

    #[test]
    fn present_values_pass_through() {
        let b = from_parts(
            "4.0.0",
            Some("abc1234"),
            Some("true"),
            Some("2026-09-23T10:00:00Z"),
        );
        assert_eq!(b.git_revision, "abc1234");
        assert!(b.dirty);
        assert_eq!(b.build_time, "2026-09-23T10:00:00Z");
        assert!(!from_parts("4", None, Some("false"), None).dirty);
    }

    #[test]
    fn macro_uses_this_crates_version() {
        let b = crate::build_info!();
        assert_eq!(b.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn iso8601_known_values() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601_utc(1_790_000_000), "2026-09-21T14:13:20Z");
        assert_eq!(iso8601_utc(4_107_542_399), "2100-02-28T23:59:59Z");
    }
}
