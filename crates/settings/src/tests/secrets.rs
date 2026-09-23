use std::sync::atomic::Ordering;

use callcore_contract::ports::SettingsReader;
use callcore_contract::{KeyStorage, SecretAction, SecretChange};
use serde_json::json;

use super::*;

fn set(id: &str, v: &str) -> SecretChange {
    SecretChange {
        key_id: id.into(),
        action: SecretAction::Set { value: v.into() },
    }
}

fn remove(id: &str) -> SecretChange {
    SecretChange {
        key_id: id.into(),
        action: SecretAction::Remove,
    }
}

fn with_secrets(base: u64, changes: Vec<SecretChange>) -> SettingsPatch {
    SettingsPatch {
        secrets: Some(changes),
        ..patch(base)
    }
}

fn secret(s: &SettingsStore, id: &str) -> Option<String> {
    s.get_secret(id).unwrap().map(|k| k.expose().to_string())
}

fn status(s: &SettingsStore, id: &str) -> (bool, KeyStorage) {
    let k = s.view().keys.into_iter().find(|k| k.id == id).unwrap();
    (k.has_key, k.storage)
}

#[test]
fn set_secret_encrypts_on_disk_and_view_shows_status_only() {
    let env = Env::new();
    let s = env.open();
    let v = s
        .apply_patch(with_secrets(0, vec![set("deepgram", "dg-SUPERSECRET-1")]))
        .unwrap();
    let k = v.keys.iter().find(|k| k.id == "deepgram").unwrap();
    assert!(k.has_key);
    assert_eq!(k.storage, KeyStorage::Encrypted);
    assert!(!env.text().contains("SUPERSECRET"));
    assert!(env.json()["secrets"]["deepgram"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
    assert_eq!(secret(&s, "deepgram").as_deref(), Some("dg-SUPERSECRET-1"));
    assert_eq!(
        secret(&env.open(), "deepgram").as_deref(),
        Some("dg-SUPERSECRET-1")
    );
}

#[test]
fn set_secret_value_is_trimmed() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(with_secrets(0, vec![set("groq", "  gsk-abc \n")]))
        .unwrap();
    assert_eq!(secret(&s, "groq").as_deref(), Some("gsk-abc"));
}

#[test]
fn empty_set_is_a_noop_and_keeps_the_key() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(with_secrets(0, vec![set("anthropic", "sk-ant-1")]))
        .unwrap();
    let calls = env.ks.protect_calls.load(Ordering::SeqCst);
    let v = s
        .apply_patch(with_secrets(1, vec![set("anthropic", "   ")]))
        .unwrap();
    assert_eq!(
        v.settings_revision, 2,
        "a successful save still bumps the revision"
    );
    assert_eq!(secret(&s, "anthropic").as_deref(), Some("sk-ant-1"));
    assert_eq!(
        env.ks.protect_calls.load(Ordering::SeqCst),
        calls,
        "nothing encrypted"
    );
}

#[test]
fn remove_deletes_the_key() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(with_secrets(
        0,
        vec![set("anthropic", "sk-ant-1"), set("groq", "gsk-1")],
    ))
    .unwrap();
    s.apply_patch(with_secrets(1, vec![remove("anthropic")]))
        .unwrap();
    assert_eq!(secret(&s, "anthropic"), None);
    assert_eq!(status(&s, "anthropic"), (false, KeyStorage::Unset));
    assert_eq!(
        secret(&s, "groq").as_deref(),
        Some("gsk-1"),
        "other keys untouched"
    );
    assert!(env.json()["secrets"].get("anthropic").is_none());
    // Removing an absent key is fine.
    s.apply_patch(with_secrets(2, vec![remove("deepgram")]))
        .unwrap();
}

#[test]
fn secret_changes_apply_in_order() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(with_secrets(0, vec![set("groq", "gsk-1"), remove("groq")]))
        .unwrap();
    assert_eq!(secret(&s, "groq"), None);
    s.apply_patch(with_secrets(1, vec![remove("groq"), set("groq", "gsk-2")]))
        .unwrap();
    assert_eq!(secret(&s, "groq").as_deref(), Some("gsk-2"));
}

#[test]
fn encryption_failure_is_fail_closed() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(with_secrets(0, vec![set("deepgram", "dg-OLD")]))
        .unwrap();
    let before = env.bytes();
    env.ks.fail_protect.store(true, Ordering::SeqCst);
    let err = s
        .apply_patch(SettingsPatch {
            hotkey: Some("Alt+N".into()),
            secrets: Some(vec![remove("groq"), set("deepgram", "dg-NEW-SECRET")]),
            ..patch(1)
        })
        .unwrap_err();
    assert!(
        err.message.to_lowercase().contains("nothing was saved"),
        "{}",
        err.message
    );
    assert!(!err.message.contains("dg-NEW-SECRET"));
    assert_eq!(env.bytes(), before, "nothing written");
    assert!(!env.text().contains("dg-NEW-SECRET"), "never plaintext");
    assert_eq!(s.revision(), 1);
    assert_ne!(s.view().hotkey, "Alt+N", "whole patch refused");
    env.ks.fail_protect.store(false, Ordering::SeqCst);
    assert_eq!(
        secret(&s, "deepgram").as_deref(),
        Some("dg-OLD"),
        "previous key untouched"
    );
    assert!(env.others().is_empty());
}

#[test]
fn unknown_key_id_rejected() {
    let env = Env::new();
    let s = env.open();
    assert!(s
        .apply_patch(with_secrets(0, vec![set("openai", "sk-1")]))
        .is_err());
    assert!(s
        .apply_patch(with_secrets(0, vec![remove("nope")]))
        .is_err());
    assert!(!env.path().exists());
}

#[test]
fn malformed_key_rejected_without_echoing_it() {
    let env = Env::new();
    let s = env.open();
    for bad in [
        "sk\u{201C}smart\u{201D}",
        "sk ant inner space",
        "sk\u{0007}bell",
    ] {
        let err = s
            .apply_patch(with_secrets(0, vec![set("anthropic", bad)]))
            .unwrap_err();
        assert!(!err.message.contains(bad), "{}", err.message);
    }
    let too_long = "k".repeat(4097);
    assert!(s
        .apply_patch(with_secrets(0, vec![set("anthropic", &too_long)]))
        .is_err());
    assert_eq!(env.ks.protect_calls.load(Ordering::SeqCst), 0);
    assert!(!env.path().exists());
}

#[test]
fn get_secret_unset_unknown_and_undecryptable_return_none() {
    let env = Env::new();
    let s = env.open();
    assert_eq!(secret(&s, "deepgram"), None);
    assert_eq!(secret(&s, "whatever"), None);
    s.apply_patch(with_secrets(0, vec![set("deepgram", "dg-1")]))
        .unwrap();
    env.ks.fail_unprotect.store(true, Ordering::SeqCst);
    assert_eq!(
        s.get_secret("deepgram").unwrap(),
        None,
        "decrypt failure reads as unset"
    );
}

#[test]
fn keys_list_ids_labels_and_urls() {
    let env = Env::new();
    let mut provs = providers();
    provs.push(callcore_contract::ProviderInfo {
        id: "mystery".into(),
        display_name: "Mystery".into(),
        key_id: "mystery_key".into(),
        model: "m".into(),
    });
    provs.push(callcore_contract::ProviderInfo {
        id: "anthropic-alt".into(),
        display_name: "Alt".into(),
        key_id: "anthropic".into(),
        model: "m".into(),
    });
    let s = SettingsStore::open(env.dir.path(), env.ks.clone(), provs);
    let keys = s.view().keys;
    let summary: Vec<(&str, &str, &str)> = keys
        .iter()
        .map(|k| (k.id.as_str(), k.label.as_str(), k.get_key_url.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("deepgram", "Deepgram", "https://console.deepgram.com/"),
            (
                "anthropic",
                "Anthropic",
                "https://console.anthropic.com/settings/keys"
            ),
            ("groq", "Groq", "https://console.groq.com/keys"),
            ("mystery_key", "mystery_key", ""),
        ],
        "deepgram first, providers deduped by key id"
    );
    assert!(keys
        .iter()
        .all(|k| !k.has_key && k.storage == KeyStorage::Unset));
}

#[test]
fn view_never_contains_key_material() {
    let env = Env::new();
    env.write_json(&json!({"secrets": {"groq": "plain:gsk-LEGACY-MATERIAL"}}));
    let s = env.open();
    s.apply_patch(with_secrets(
        0,
        vec![
            set("deepgram", "dg-VIEW-MATERIAL"),
            set("anthropic", "sk-ant-VIEW-MATERIAL"),
        ],
    ))
    .unwrap();
    let json = serde_json::to_string(&s.view()).unwrap();
    for needle in ["VIEW-MATERIAL", "LEGACY-MATERIAL", "dpapi:", "FK1"] {
        assert!(!json.contains(needle), "{needle} leaked into view: {json}");
    }
}

#[test]
fn debug_never_prints_keys_or_profile_text() {
    let env = Env::new();
    env.write_json(&json!({"secrets": {"groq": "plain:gsk-DEBUG-LEGACY"}}));
    env.ks.fail_protect.store(true, Ordering::SeqCst); // keeps the legacy value as Plain in memory
    let s = env.open();
    env.ks.fail_protect.store(false, Ordering::SeqCst);
    let mut p = profile("a", "A");
    p.resume = "RESUME-TEXT-DO-NOT-LOG".into();
    p.notes = "NOTES-TEXT-DO-NOT-LOG".into();
    s.apply_patch(SettingsPatch {
        profiles: Some(vec![p]),
        secrets: Some(vec![set("deepgram", "dg-DEBUG-KEY")]),
        ..patch(0)
    })
    .unwrap();
    let env2 = Env::new();
    env2.write_json(&json!({"secrets": {"groq": "plain:gsk-DEBUG-PLAIN"}}));
    env2.ks.fail_protect.store(true, Ordering::SeqCst);
    let s2 = env2.open();
    let dbg = format!("{s:?} {s2:?} {:#?}", s);
    for needle in ["dg-DEBUG-KEY", "gsk-DEBUG", "RESUME-TEXT", "NOTES-TEXT"] {
        assert!(!dbg.contains(needle), "{needle} in {dbg}");
    }
    assert!(dbg.contains("SettingsStore"));
}

#[test]
fn errors_never_contain_key_material() {
    let env = Env::new();
    let s = env.open();
    env.ks.fail_protect.store(true, Ordering::SeqCst);
    let e1 = s
        .apply_patch(with_secrets(0, vec![set("groq", "gsk-ERR-KEY")]))
        .unwrap_err();
    env.ks.fail_protect.store(false, Ordering::SeqCst);
    let e2 = s
        .apply_patch(with_secrets(9, vec![set("groq", "gsk-ERR-KEY")]))
        .unwrap_err();
    let e3 = s
        .apply_patch(with_secrets(0, vec![set("nope", "gsk-ERR-KEY")]))
        .unwrap_err();
    for e in [e1, e2, e3] {
        assert!(!format!("{e:?} {e}").contains("gsk-ERR-KEY"));
    }
}

#[test]
fn default_dir_is_appdata_aicallassistant() {
    let d = crate::default_dir();
    assert!(d.ends_with("AICallAssistant"), "{}", d.display());
    if let Some(appdata) = std::env::var_os("APPDATA") {
        assert_eq!(d, std::path::PathBuf::from(appdata).join("AICallAssistant"));
    }
}

#[cfg(windows)]
#[test]
fn real_dpapi_store_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ks: Arc<dyn callcore_contract::ports::Keystore> = Arc::new(crate::DpapiKeystore);
    let s = SettingsStore::open(dir.path(), ks.clone(), providers());
    s.apply_patch(with_secrets(0, vec![set("deepgram", "dg-REAL-DPAPI-KEY")]))
        .unwrap();
    let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(!text.contains("dg-REAL-DPAPI-KEY"));
    let s2 = SettingsStore::open(dir.path(), ks, providers());
    assert_eq!(status(&s2, "deepgram"), (true, KeyStorage::Encrypted));
    assert_eq!(
        secret(&s2, "deepgram").as_deref(),
        Some("dg-REAL-DPAPI-KEY")
    );
}

#[cfg(not(windows))]
#[test]
fn dpapi_fails_closed_off_windows() {
    use callcore_contract::ports::Keystore;
    assert!(crate::DpapiKeystore.protect(b"x").is_err());
    assert!(crate::DpapiKeystore.unprotect(b"x").is_err());
}
