//! Build script: Tauri codegen + embedded build info (spec §17).
//!
//! Sets, via `cargo:rustc-env`:
//! * `AICA_GIT_REV`   — short HEAD revision, or "unknown" (no git / no commits)
//! * `AICA_GIT_DIRTY` — "true" if `git status --porcelain` prints anything,
//!   INCLUDING untracked files; "false" otherwise or when git is missing
//! * `AICA_BUILD_TIME` — UTC ISO-8601 build time

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Days-from-civil inverse (Howard Hinnant), no dependencies.
fn iso8601_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
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

fn main() {
    let rev = git(&["rev-parse", "--short=12", "HEAD"])
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let dirty = match git(&["status", "--porcelain", "--untracked-files=normal"]) {
        Some(s) => !s.is_empty(),
        None => false,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=AICA_GIT_REV={rev}");
    println!("cargo:rustc-env=AICA_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=AICA_BUILD_TIME={}", iso8601_utc(now));
    // Re-run when HEAD moves or the index changes (commits, staging). Edits
    // to tracked/untracked files are picked up on the next rebuild of this
    // crate; a release build is always a fresh build.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");

    // Embed the app manifest with the LINKER for every target — including
    // test binaries. tauri-build's default embeds it as a resource only into
    // the app binary, so `tauri::test` integration tests die at load with
    // STATUS_ENTRYPOINT_NOT_FOUND (comctl32 v6 imports). The manifest also
    // declares per-monitor DPI v2.
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let windows_attrs = if msvc {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        tauri_build::WindowsAttributes::new_without_app_manifest()
    } else {
        tauri_build::WindowsAttributes::new()
    };
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows_attrs))
        .expect("tauri-build failed");
}
