//! File plumbing: atomic replace-with-retry and uniquely named backups.

use std::collections::hash_map::RandomState;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// How many times a `rename` is attempted when Windows reports a sharing
/// violation (antivirus / indexer holding the target open).
const RENAME_ATTEMPTS: u32 = 6;

fn is_transient(e: &io::Error) -> bool {
    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION.
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Temp file name in the same directory as `target`.
pub(crate) fn temp_path(target: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("settings.json");
    target.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()))
}

/// Write `bytes` to a temp file next to `target`, fsync it, then rename it
/// over `target` (retrying transient Windows sharing violations with a short
/// backoff). The temp file never survives a failure.
pub(crate) fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = target.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let tmp = temp_path(target);
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_with_retry(&tmp, target)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut delay = Duration::from_millis(15);
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_ATTEMPTS && is_transient(&e) => {
                std::thread::sleep(delay);
                delay *= 2;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// `settings.json.<tag>-<yyyyMMddTHHmmss>-<random6>.bak` next to `target`
/// (timestamp in UTC).
pub(crate) fn backup_path(target: &Path, tag: &str) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("settings.json");
    target.with_file_name(format!(
        "{name}.{tag}-{}-{}.bak",
        utc_stamp(SystemTime::now()),
        random6()
    ))
}

/// Create a NEW backup file (never overwrites) holding `bytes`, fsynced.
pub(crate) fn write_backup(target: &Path, tag: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let mut last = None;
    for _ in 0..3 {
        let path = backup_path(target, tag);
        let res = (|| {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            f.write_all(bytes)?;
            f.sync_all()
        })();
        match res {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => {
                let _ = fs::remove_file(&path);
                return Err(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("could not pick a backup name")))
}

/// Six lowercase base-36 characters.
pub(crate) fn random6() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let mut v = h.finish();
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..6)
        .map(|_| {
            let c = ALPHA[(v % 36) as usize] as char;
            v /= 36;
            c
        })
        .collect()
}

/// `yyyyMMddTHHmmss` in UTC.
pub(crate) fn utc_stamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-since-epoch -> (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_stamp_formats_known_instants() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000");
        // 2026-09-23T14:05:09Z
        let t = UNIX_EPOCH + Duration::from_secs(1_790_172_309);
        assert_eq!(utc_stamp(t), "20260923T140509");
        // Leap day 2024-02-29T23:59:59Z
        let t = UNIX_EPOCH + Duration::from_secs(1_709_251_199);
        assert_eq!(utc_stamp(t), "20240229T235959");
    }

    #[test]
    fn random6_is_six_base36_chars_and_varies() {
        let a = random6();
        assert_eq!(a.len(), 6);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()));
        let many: std::collections::HashSet<String> = (0..50).map(|_| random6()).collect();
        assert!(many.len() > 45);
    }
}
