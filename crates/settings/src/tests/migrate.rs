use callcore_contract::ports::SettingsReader;
use callcore_contract::{CallType, KeyStorage, LayoutMode, SettingsLoadStatus};
use serde_json::json;

use super::*;

fn key_status(s: &SettingsStore, id: &str) -> (bool, KeyStorage) {
    let k = s.view().keys.into_iter().find(|k| k.id == id).unwrap();
    (k.has_key, k.storage)
}

fn secret(s: &SettingsStore, id: &str) -> Option<String> {
    s.get_secret(id).unwrap().map(|k| k.expose().to_string())
}

#[test]
fn v3_legacy_top_level_resume_becomes_default_profile() {
    let env = Env::new();
    env.write_json(&json!({"resume": "Ten years of Rust", "jobDescription": "Staff engineer", "hotkey": "Alt+X"}));
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Ok);
    let v = s.view();
    assert_eq!(v.profiles.len(), 1);
    let p = &v.profiles[0];
    assert_eq!((p.id.as_str(), p.name.as_str()), ("default", "Default"));
    assert_eq!(p.resume, "Ten years of Rust");
    assert_eq!(p.job_description, "Staff engineer");
    assert_eq!(p.call_type, CallType::Behavioral);
    assert_eq!(v.hotkey, "Alt+X");
}

#[test]
fn v3_legacy_snake_case_job_description() {
    let env = Env::new();
    env.write_json(&json!({"resume": "R", "job_description": "JD snake"}));
    assert_eq!(env.open().view().profiles[0].job_description, "JD snake");
}

#[test]
fn v3_profile_snake_case_aliases_camel_wins() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [
        {"id": "a", "name": "A", "call_type": "recruiter", "job_description": "snake jd"},
        {"id": "b", "name": "B", "callType": "meeting", "call_type": "sales",
         "jobDescription": "camel", "job_description": "snake"}
    ]}));
    let v = env.open().view();
    assert_eq!(v.profiles[0].call_type, CallType::Recruiter);
    assert_eq!(v.profiles[0].job_description, "snake jd");
    assert_eq!(v.profiles[1].call_type, CallType::Meeting);
    assert_eq!(v.profiles[1].job_description, "camel");
}

#[test]
fn v3_top_level_mirrors_ignored_when_profiles_exist() {
    let env = Env::new();
    env.write_json(&json!({
        "profiles": [{"id": "job1", "name": "Job 1", "resume": "profile resume"}],
        "activeProfileId": "job1",
        "resume": "mirror resume", "jobDescription": "mirror jd"
    }));
    let v = env.open().view();
    assert_eq!(v.profiles.len(), 1);
    assert_eq!(v.profiles[0].resume, "profile resume");
}

#[test]
fn full_v3_file_migrates_and_is_rewritten_as_v4() {
    let env = Env::new();
    env.write_json(&json!({
        "profiles": [{"id": "default", "name": "Default", "callType": "technical", "focus": "",
                      "resume": "R", "jobDescription": "J", "notes": ""}],
        "activeProfileId": "default", "resume": "R", "jobDescription": "J",
        "alwaysOnTop": true, "llmProvider": "groq", "answerStyle": "brief",
        "hotkey": "Ctrl+Shift+Space", "layoutMode": "prompter", "prompterFontPx": 20,
        "answerFontPx": 16,
        "secrets": {"deepgram": format!("enc:{}", B64.encode(fake_encrypt(b"dg-key")))},
        "windowBounds": {"x": 10, "y": 20, "width": 900, "height": 700}, "prompterBounds": null
    }));
    let s = env.open();
    assert_eq!(secret(&s, "deepgram").as_deref(), Some("dg-key"));
    s.save_bounds(
        LayoutMode::Prompter,
        callcore_contract::Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        },
    )
    .unwrap();
    let j = env.json();
    assert_eq!(j["version"], 4);
    assert_eq!(j["settingsRevision"], 0);
    assert!(
        j.get("resume").is_none() && j.get("jobDescription").is_none(),
        "legacy mirrors not written back"
    );
    assert_eq!(j["profiles"][0]["callType"], "technical");
    assert_eq!(j["llmProvider"], "groq");
    assert_eq!(j["windowBounds"]["width"], 900);
    let dg = j["secrets"]["deepgram"].as_str().unwrap();
    assert!(dg.starts_with("dpapi:"), "enc: normalized to dpapi: ({dg})");
    let reopened = env.open();
    assert_eq!(secret(&reopened, "deepgram").as_deref(), Some("dg-key"));
}

#[test]
fn legacy_top_level_api_key_fields() {
    let env = Env::new();
    env.write_json(&json!({
        "deepgramApiKey": "dg-raw-unprefixed",
        "anthropicApiKey": format!("plain:{}", B64.encode("sk-ant-legacy")),
        "groqApiKey": format!("enc:{}", B64.encode(fake_encrypt(b"gsk-legacy")))
    }));
    let s = env.open();
    assert_eq!(secret(&s, "deepgram").as_deref(), Some("dg-raw-unprefixed"));
    assert_eq!(secret(&s, "anthropic").as_deref(), Some("sk-ant-legacy"));
    assert_eq!(secret(&s, "groq").as_deref(), Some("gsk-legacy"));
    for id in ["deepgram", "anthropic", "groq"] {
        assert_eq!(key_status(&s, id), (true, KeyStorage::Encrypted), "{id}");
    }
}

#[test]
fn legacy_api_keys_object() {
    let env = Env::new();
    env.write_json(&json!({"apiKeys": {
        "deepgram": "plain:dg-verbatim-key",
        "anthropic": fake_dpapi("sk-ant-obj"),
        "groq": "unprefixed-is-invalid-here"
    }}));
    let s = env.open();
    assert_eq!(secret(&s, "deepgram").as_deref(), Some("dg-verbatim-key"));
    assert_eq!(secret(&s, "anthropic").as_deref(), Some("sk-ant-obj"));
    assert_eq!(secret(&s, "groq"), None);
    assert_eq!(key_status(&s, "groq"), (false, KeyStorage::Unreadable));
}

#[test]
fn secret_sources_precedence_secrets_over_apikeys_over_top_level() {
    let env = Env::new();
    env.write_json(&json!({
        "deepgramApiKey": "top",
        "apiKeys": {"deepgram": "plain:apikeys", "anthropic": "plain:apikeys-anthropic"},
        "secrets": {"deepgram": "plain:secrets"},
        "groqApiKey": "top-groq"
    }));
    let s = env.open();
    assert_eq!(secret(&s, "deepgram").as_deref(), Some("secrets"));
    assert_eq!(
        secret(&s, "anthropic").as_deref(),
        Some("apikeys-anthropic")
    );
    assert_eq!(secret(&s, "groq").as_deref(), Some("top-groq"));
}

#[test]
fn plain_secret_reencrypted_on_first_write_and_plaintext_gone() {
    let env = Env::new();
    env.write_json(&json!({
        "secrets": {"deepgram": format!("plain:{}", B64.encode("dg-PLAINTEXT-123"))},
        "anthropicApiKey": "sk-ant-PLAINTEXT-456"
    }));
    let s = env.open();
    // Not written on open.
    assert!(env.text().contains("sk-ant-PLAINTEXT-456"));
    s.apply_patch(patch(0)).unwrap();
    let text = env.text();
    assert!(!text.contains("PLAINTEXT"), "{text}");
    assert!(
        !text.contains(&B64.encode("dg-PLAINTEXT-123")),
        "base64 plaintext also gone"
    );
    assert!(!text.contains("plain:"));
    assert!(!text.contains("anthropicApiKey"));
    let j = env.json();
    assert!(j["secrets"]["deepgram"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
    assert!(j["secrets"]["anthropic"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
    let s2 = env.open();
    assert_eq!(secret(&s2, "deepgram").as_deref(), Some("dg-PLAINTEXT-123"));
    assert_eq!(
        secret(&s2, "anthropic").as_deref(),
        Some("sk-ant-PLAINTEXT-456")
    );
}

#[test]
fn plain_secret_reencrypted_by_geometry_autosave_too() {
    let env = Env::new();
    env.write_json(&json!({"secrets": {"groq": "plain:gsk-PLAINTEXT"}}));
    let s = env.open();
    s.save_bounds(
        LayoutMode::Full,
        callcore_contract::Bounds {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
    )
    .unwrap();
    assert!(!env.text().contains("PLAINTEXT"));
}

#[test]
fn plain_secret_when_dpapi_fails_reads_unset_and_is_preserved() {
    let env = Env::new();
    env.ks
        .fail_protect
        .store(true, std::sync::atomic::Ordering::SeqCst);
    env.write_json(&json!({"secrets": {"deepgram": "plain:dg-legacy"}}));
    let s = env.open();
    assert_eq!(
        secret(&s, "deepgram"),
        None,
        "fail closed: never use a key we can't encrypt"
    );
    assert_eq!(key_status(&s, "deepgram"), (false, KeyStorage::Unreadable));
    s.apply_patch(patch(0)).unwrap();
    // Not destroyed: it was already on disk; it is re-tried on the next write.
    assert_eq!(
        env.json()["secrets"]["deepgram"],
        format!("plain:{}", B64.encode("dg-legacy"))
    );
    env.ks
        .fail_protect
        .store(false, std::sync::atomic::Ordering::SeqCst);
    s.apply_patch(patch(1)).unwrap();
    assert!(env.json()["secrets"]["deepgram"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
}

#[test]
fn undecryptable_blob_reads_unset_and_is_preserved() {
    let env = Env::new();
    let foreign = format!("dpapi:{}", B64.encode(b"blob from another user"));
    env.write_json(&json!({"secrets": {"anthropic": foreign.clone()}}));
    let s = env.open();
    assert_eq!(secret(&s, "anthropic"), None);
    assert_eq!(key_status(&s, "anthropic"), (false, KeyStorage::Unreadable));
    s.apply_patch(patch(0)).unwrap();
    assert_eq!(env.json()["secrets"]["anthropic"], foreign.as_str());
}

#[test]
fn invalid_secret_values_read_unreadable_and_are_dropped_on_write() {
    let env = Env::new();
    env.write_json(
        &json!({"secrets": {"deepgram": "dpapi:%%%notbase64", "groq": 12, "anthropic": ""}}),
    );
    let s = env.open();
    assert_eq!(key_status(&s, "deepgram"), (false, KeyStorage::Unreadable));
    assert_eq!(key_status(&s, "groq"), (false, KeyStorage::Unreadable));
    assert_eq!(
        key_status(&s, "anthropic"),
        (false, KeyStorage::Unset),
        "empty string = unset"
    );
    s.apply_patch(patch(0)).unwrap();
    assert_eq!(env.json()["secrets"], json!({}));
}

#[test]
fn unknown_secret_ids_are_preserved_but_not_listed() {
    let env = Env::new();
    env.write_json(&json!({"secrets": {"openai": fake_dpapi("sk-openai")}}));
    let s = env.open();
    assert!(s.view().keys.iter().all(|k| k.id != "openai"));
    s.apply_patch(patch(0)).unwrap();
    assert!(env.json()["secrets"]["openai"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
}
