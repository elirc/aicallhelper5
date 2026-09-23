use callcore_contract::{Bounds, LayoutMode, SettingsLoadStatus};

use super::*;

const CORRUPT: &[u8] = b"{ \"resume\": \"half-written file with my resume";

fn bounds() -> Bounds {
    Bounds {
        x: 1,
        y: 2,
        width: 300,
        height: 400,
    }
}

/// `settings.json.<tag>-yyyyMMddTHHmmss-xxxxxx.bak`
fn assert_backup_name(name: &str, tag: &str) {
    let rest = name
        .strip_prefix(&format!("settings.json.{tag}-"))
        .and_then(|r| r.strip_suffix(".bak"))
        .unwrap_or_else(|| panic!("bad backup name {name}"));
    let (stamp, id) = rest
        .split_once('-')
        .unwrap_or_else(|| panic!("bad backup name {name}"));
    assert_eq!(stamp.len(), 15, "{name}");
    assert_eq!(&stamp[8..9], "T", "{name}");
    assert!(
        stamp[..8]
            .bytes()
            .chain(stamp[9..].bytes())
            .all(|b| b.is_ascii_digit()),
        "{name}"
    );
    assert_eq!(id.len(), 6, "{name}");
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
        "{name}"
    );
}

#[test]
fn corrupt_file_untouched_when_closed_without_writing() {
    let env = Env::new();
    env.write_raw(CORRUPT);
    let s = env.open();
    let _ = s.view();
    let _ = s.bounds(LayoutMode::Full);
    drop(s);
    assert_eq!(env.bytes(), CORRUPT);
    assert!(
        env.others().is_empty(),
        "no backup, no temp files: {:?}",
        env.others()
    );
}

#[test]
fn corrupt_file_backed_up_before_first_patch() {
    let env = Env::new();
    env.write_raw(CORRUPT);
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Corrupt);
    let v = s
        .apply_patch(SettingsPatch {
            hotkey: Some("Alt+K".into()),
            ..patch(0)
        })
        .unwrap();
    let others = env.others();
    assert_eq!(others.len(), 1, "{others:?}");
    assert_backup_name(&others[0], "corrupt");
    let bak = env.dir.path().join(&others[0]);
    assert_eq!(
        std::fs::read(&bak).unwrap(),
        CORRUPT,
        "backup holds the original bytes"
    );
    let issue = v.load_issue.expect("issue still surfaced");
    assert_eq!(
        issue.backup_path.as_deref(),
        Some(bak.display().to_string().as_str())
    );
    assert!(!issue.writes_blocked);
    assert!(
        issue.message.contains(&others[0]),
        "message tells the user where: {}",
        issue.message
    );
    assert_eq!(env.json()["hotkey"], "Alt+K");
}

#[test]
fn corrupt_file_backed_up_before_first_geometry_save() {
    let env = Env::new();
    env.write_raw(CORRUPT);
    let s = env.open();
    s.save_bounds(LayoutMode::Full, bounds()).unwrap();
    let others = env.others();
    assert_eq!(others.len(), 1);
    assert_backup_name(&others[0], "corrupt");
    assert_eq!(
        std::fs::read(env.dir.path().join(&others[0])).unwrap(),
        CORRUPT
    );
    assert!(s.load_issue().unwrap().backup_path.is_some());
}

#[test]
fn backup_happens_only_once() {
    let env = Env::new();
    env.write_raw(CORRUPT);
    let s = env.open();
    s.save_bounds(LayoutMode::Full, bounds()).unwrap();
    s.apply_patch(patch(0)).unwrap();
    s.save_bounds(LayoutMode::Prompter, bounds()).unwrap();
    s.apply_patch(patch(1)).unwrap();
    assert_eq!(env.others().len(), 1, "{:?}", env.others());
}

#[test]
fn rejected_patch_does_not_back_up_or_write() {
    let env = Env::new();
    env.write_raw(CORRUPT);
    let s = env.open();
    assert!(s.apply_patch(patch(5)).is_err());
    assert!(s
        .apply_patch(SettingsPatch {
            prompter_font_px: Some(15),
            ..patch(0)
        })
        .is_err());
    assert_eq!(env.bytes(), CORRUPT);
    assert!(env.others().is_empty());
    assert!(s.load_issue().unwrap().backup_path.is_none());
}

#[test]
fn first_run_write_makes_no_backup() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(patch(0)).unwrap();
    s.save_bounds(LayoutMode::Full, bounds()).unwrap();
    assert!(env.others().is_empty(), "{:?}", env.others());
    assert!(s.view().load_issue.is_none());
}

#[test]
fn unreadable_file_loads_defaults_and_blocks_writes_when_backup_fails() {
    // A directory named settings.json: reading it is an IO error (not
    // NotFound), and copying it for the backup fails too.
    let env = Env::new();
    std::fs::create_dir(env.path()).unwrap();
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Unreadable);
    let issue = s.view().load_issue.expect("issue surfaced");
    assert_eq!(issue.status, SettingsLoadStatus::Unreadable);
    assert!(
        !issue.writes_blocked,
        "not blocked until a write is attempted"
    );
    assert_eq!(s.view().profiles[0].id, "default");

    let err = s.save_bounds(LayoutMode::Full, bounds()).unwrap_err();
    assert!(err.message.contains("Nothing was saved"), "{}", err.message);
    let issue = s.view().load_issue.unwrap();
    assert!(issue.writes_blocked);
    assert!(issue.backup_path.is_none());
    assert!(s.apply_patch(patch(0)).is_err(), "all later writes refused");
    assert!(env.path().is_dir(), "original left in place");
    assert!(env.others().is_empty(), "{:?}", env.others());
}

#[test]
fn unreadable_file_is_backed_up_by_reread_when_it_becomes_readable() {
    let env = Env::new();
    std::fs::create_dir(env.path()).unwrap();
    let s = env.open();
    // The transient cause goes away (e.g. AV released it) before the first write.
    std::fs::remove_dir(env.path()).unwrap();
    env.write_raw(b"original bytes");
    s.apply_patch(patch(0)).unwrap();
    let others = env.others();
    assert_eq!(others.len(), 1, "{others:?}");
    assert_backup_name(&others[0], "unreadable");
    assert_eq!(
        std::fs::read(env.dir.path().join(&others[0])).unwrap(),
        b"original bytes"
    );
}

#[test]
fn corrupt_backup_failure_blocks_all_writes_even_after_recovery() {
    let env = Env::new();
    let sub = env.dir.path().join("cfg");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("settings.json"), CORRUPT).unwrap();
    let s = SettingsStore::open(&sub, env.ks.clone(), providers());
    assert_eq!(s.load_status(), SettingsLoadStatus::Corrupt);
    // Make the backup impossible: the settings directory becomes a file.
    std::fs::remove_dir_all(&sub).unwrap();
    std::fs::write(&sub, b"i am a file now").unwrap();
    let err = s.apply_patch(patch(0)).unwrap_err();
    assert_eq!(err.code, callcore_contract::ErrorCode::Internal);
    assert!(err.message.contains("Nothing was saved"), "{}", err.message);
    assert!(
        err.message.contains("settings.json"),
        "actionable: names the file: {}",
        err.message
    );
    let issue = s.load_issue().unwrap();
    assert!(issue.writes_blocked);
    assert_eq!(issue.message, err.message);
    // Environment heals; the store still refuses (the original was never preserved).
    std::fs::remove_file(&sub).unwrap();
    std::fs::create_dir(&sub).unwrap();
    assert!(s.apply_patch(patch(0)).is_err());
    assert!(s.save_bounds(LayoutMode::Full, bounds()).is_err());
    assert!(list_others(&sub).is_empty());
    assert!(!sub.join("settings.json").exists());
    assert_eq!(s.revision(), 0, "nothing applied in memory either");
}
