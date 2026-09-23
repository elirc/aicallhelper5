use callcore_contract::{
    AnswerStyle, Bounds, CallType, LayoutMode, SettingsLoadStatus, DEFAULT_HOTKEY, MAX_PROFILES,
};
use serde_json::json;

use super::*;

#[test]
fn missing_file_is_first_run_defaults_without_issue() {
    let env = Env::new();
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::FirstRun);
    let v = s.view();
    assert!(v.load_issue.is_none());
    assert_eq!(v.settings_revision, 0);
    assert_eq!(v.profiles.len(), 1);
    assert_eq!(v.profiles[0].id, "default");
    assert_eq!(v.profiles[0].name, "Default");
    assert_eq!(v.active_profile_id, "default");
    assert_eq!(v.llm_provider, "anthropic");
    assert_eq!(v.answer_style, AnswerStyle::Balanced);
    assert_eq!(v.hotkey, DEFAULT_HOTKEY);
    assert!(v.always_on_top);
    assert_eq!(v.layout_mode, LayoutMode::Full);
    assert_eq!((v.prompter_font_px, v.answer_font_px), (18, 14));
    assert_eq!(v.settings_path, env.path().display().to_string());
    assert_eq!(v.build.version, env!("CARGO_PKG_VERSION"));
    assert!(!v.hotkey_registered);
    assert!(s.bounds(LayoutMode::Full).is_none());
}

#[test]
fn first_run_open_does_not_create_file() {
    let env = Env::new();
    drop(env.open());
    assert!(!env.path().exists());
    assert!(env.others().is_empty());
}

#[test]
fn missing_directory_is_first_run_and_first_write_creates_it() {
    let env = Env::new();
    let sub = env.dir.path().join("nested").join("AICallAssistant");
    let s = SettingsStore::open(&sub, env.ks.clone(), providers());
    assert_eq!(s.load_status(), SettingsLoadStatus::FirstRun);
    s.apply_patch(patch(0)).unwrap();
    assert!(sub.join("settings.json").exists());
}

#[test]
fn valid_file_loads_all_fields() {
    let env = Env::new();
    env.write_json(&json!({
        "version": 4, "settingsRevision": 9,
        "profiles": [
            {"id":"a","name":"Alpha","callType":"technical","focus":"f","resume":"r","jobDescription":"j","notes":"n"},
            {"id":"b","name":"Beta","callType":"sales","focus":"","resume":"","jobDescription":"","notes":""}
        ],
        "activeProfileId": "b", "llmProvider": "groq", "answerStyle": "detailed",
        "hotkey": "Alt+Q", "alwaysOnTop": false, "layoutMode": "prompter",
        "prompterFontPx": 24, "answerFontPx": 20,
        "windowBounds": {"x": -10, "y": 5, "width": 800, "height": 600},
        "prompterBounds": {"x": 1, "y": 2, "width": 300, "height": 200},
        "secrets": {}
    }));
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Ok);
    let v = s.view();
    assert!(v.load_issue.is_none());
    assert_eq!(v.settings_revision, 9);
    assert_eq!(v.profiles.len(), 2);
    assert_eq!(v.profiles[0].call_type, CallType::Technical);
    assert_eq!(v.profiles[0].job_description, "j");
    assert_eq!(v.active_profile_id, "b");
    assert_eq!(v.llm_provider, "groq");
    assert_eq!(v.answer_style, AnswerStyle::Detailed);
    assert_eq!(v.hotkey, "Alt+Q");
    assert!(!v.always_on_top);
    assert_eq!(v.layout_mode, LayoutMode::Prompter);
    assert_eq!((v.prompter_font_px, v.answer_font_px), (24, 20));
    assert_eq!(
        s.bounds(LayoutMode::Full),
        Some(Bounds {
            x: -10,
            y: 5,
            width: 800,
            height: 600
        })
    );
    assert_eq!(
        s.bounds(LayoutMode::Prompter),
        Some(Bounds {
            x: 1,
            y: 2,
            width: 300,
            height: 200
        })
    );
}

#[test]
fn invalid_scalar_fields_fall_back_individually() {
    let env = Env::new();
    env.write_json(&json!({
        "settingsRevision": -3,
        "profiles": [{"id":"keep","name":"Keep me","resume":"my resume"}],
        "activeProfileId": 42,
        "llmProvider": "openai",
        "answerStyle": "verbose",
        "hotkey": "x".repeat(101),
        "alwaysOnTop": "yes",
        "layoutMode": "tiny",
        "prompterFontPx": "big",
        "answerFontPx": null,
        "windowBounds": "nope",
        "prompterBounds": {"x": 0, "y": 0, "width": 400, "height": 300}
    }));
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Ok);
    let v = s.view();
    assert_eq!(v.settings_revision, 0);
    assert_eq!(
        v.profiles[0].resume, "my resume",
        "valid field survives its neighbours"
    );
    assert_eq!(v.active_profile_id, "keep");
    assert_eq!(v.llm_provider, "anthropic");
    assert_eq!(v.answer_style, AnswerStyle::Balanced);
    assert_eq!(v.hotkey, DEFAULT_HOTKEY);
    assert!(v.always_on_top);
    assert_eq!(v.layout_mode, LayoutMode::Full);
    assert_eq!((v.prompter_font_px, v.answer_font_px), (18, 14));
    assert!(s.bounds(LayoutMode::Full).is_none());
    assert!(
        s.bounds(LayoutMode::Prompter).is_some(),
        "valid bounds survive invalid sibling"
    );
}

#[test]
fn unknown_llm_provider_falls_back_to_anthropic() {
    let env = Env::new();
    env.write_json(&json!({"llmProvider": "mystery"}));
    assert_eq!(env.open().view().llm_provider, "anthropic");
    env.write_json(&json!({"llmProvider": "groq"}));
    assert_eq!(env.open().view().llm_provider, "groq");
}

#[test]
fn fonts_clamp_and_snap_to_step_grid() {
    let env = Env::new();
    for (p, a, want_p, want_a) in [
        (json!(15), json!(13), 14, 12),
        (json!(100), json!(100), 28, 22),
        (json!(-4), json!(0), 14, 12),
        (json!(27), json!(21), 26, 20),
        (json!(19.7), json!(16.0), 18, 16),
        (json!(20), json!(22), 20, 22),
    ] {
        env.write_json(&json!({"prompterFontPx": p, "answerFontPx": a}));
        let v = env.open().view();
        assert_eq!(
            (v.prompter_font_px, v.answer_font_px),
            (want_p, want_a),
            "{p} {a}"
        );
    }
}

#[test]
fn hotkey_empty_kept_as_disabled_and_verbatim() {
    let env = Env::new();
    env.write_json(&json!({"hotkey": ""}));
    assert_eq!(env.open().view().hotkey, "");
    env.write_json(&json!({"hotkey": "not a real combo!!"}));
    assert_eq!(
        env.open().view().hotkey,
        "not a real combo!!",
        "validity is the shell's job"
    );
    env.write_json(&json!({"hotkey": "é".repeat(100)}));
    assert_eq!(
        env.open().view().hotkey.chars().count(),
        100,
        "limit counts chars"
    );
}

#[test]
fn bounds_sanity_checked_per_layout() {
    let env = Env::new();
    for bad in [
        json!({"x":0,"y":0,"width":0,"height":10}),
        json!({"x":0,"y":0,"width":10,"height":100000}),
        json!({"x":0,"y":0,"width":-5,"height":10}),
        json!({"x":0.5,"y":0,"width":10,"height":10}),
        json!({"x":3000000000i64,"y":0,"width":10,"height":10}),
        json!({"y":0,"width":10,"height":10}),
        json!([1, 2, 3, 4]),
    ] {
        env.write_json(&json!({"windowBounds": bad}));
        assert!(env.open().bounds(LayoutMode::Full).is_none(), "{bad}");
    }
    env.write_json(&json!({"windowBounds": {"x":-2000,"y":-50,"width":99999,"height":1}}));
    assert!(env.open().bounds(LayoutMode::Full).is_some());
}

#[test]
fn profiles_invalid_entries_dropped_individually() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [
        {"id": "ok1", "name": "One"},
        {"id": "bad id", "name": "Space"},
        {"id": "", "name": "Empty"},
        {"id": "x".repeat(65), "name": "Long"},
        {"name": "No id"},
        {"id": 7, "name": "Numeric"},
        "not an object",
        {"id": "ok2", "name": "Two"}
    ]}));
    let v = env.open().view();
    let ids: Vec<&str> = v.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["ok1", "ok2"]);
}

#[test]
fn profiles_duplicate_ids_keep_first() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [
        {"id": "a", "name": "First"}, {"id": "a", "name": "Second"}, {"id": "b", "name": "B"}
    ]}));
    let v = env.open().view();
    assert_eq!(v.profiles.len(), 2);
    assert_eq!(v.profiles[0].name, "First");
}

#[test]
fn profiles_truncate_by_chars_not_bytes() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [{
        "id": "a",
        "name": format!("  {}  ", "é".repeat(70)),
        "focus": "ü".repeat(2500),
        "resume": "字".repeat(200_010),
        "jobDescription": "j".repeat(200_001),
        "notes": "n".repeat(10)
    }]}));
    let p = &env.open().view().profiles[0];
    assert_eq!(p.name, "é".repeat(60));
    assert_eq!(p.focus, "ü".repeat(2000));
    assert_eq!(p.resume.chars().count(), 200_000);
    assert_eq!(p.job_description.len(), 200_000);
    assert_eq!(p.notes, "n".repeat(10));
}

#[test]
fn profile_missing_or_blank_name_becomes_untitled_and_non_string_text_is_empty() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [
        {"id": "a"}, {"id": "b", "name": "   ", "resume": 5, "notes": ["x"]}
    ]}));
    let v = env.open().view();
    assert_eq!(v.profiles[0].name, "Untitled");
    assert_eq!(v.profiles[1].name, "Untitled");
    assert_eq!(v.profiles[1].resume, "");
    assert_eq!(v.profiles[1].notes, "");
}

#[test]
fn profiles_capped_at_twenty() {
    let env = Env::new();
    let list: Vec<_> = (0..25)
        .map(|i| json!({"id": format!("p{i}"), "name": format!("P{i}")}))
        .collect();
    env.write_json(&json!({"profiles": list, "activeProfileId": "p22"}));
    let v = env.open().view();
    assert_eq!(v.profiles.len(), MAX_PROFILES);
    assert_eq!(v.profiles[19].id, "p19");
    assert_eq!(
        v.active_profile_id, "p0",
        "active beyond the cap falls back to the first"
    );
}

#[test]
fn zero_valid_profiles_creates_default() {
    let env = Env::new();
    for profiles in [
        json!([]),
        json!([{"id": "bad id"}]),
        json!("nope"),
        json!({"id": "a"}),
    ] {
        env.write_json(&json!({"profiles": profiles, "activeProfileId": "a"}));
        let v = env.open().view();
        assert_eq!(v.profiles.len(), 1);
        assert_eq!(v.profiles[0].id, "default");
        assert_eq!(v.profiles[0].name, "Default");
        assert_eq!(v.active_profile_id, "default");
    }
}

#[test]
fn unknown_call_type_becomes_behavioral() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [
        {"id": "a", "name": "A", "callType": "karaoke"},
        {"id": "b", "name": "B", "callType": 3},
        {"id": "c", "name": "C", "callType": "system_design"}
    ]}));
    let v = env.open().view();
    assert_eq!(v.profiles[0].call_type, CallType::Behavioral);
    assert_eq!(v.profiles[1].call_type, CallType::Behavioral);
    assert_eq!(v.profiles[2].call_type, CallType::SystemDesign);
}

#[test]
fn active_profile_falls_back_to_first() {
    let env = Env::new();
    env.write_json(&json!({"profiles": [{"id":"a","name":"A"},{"id":"b","name":"B"}], "activeProfileId": "zzz"}));
    assert_eq!(env.open().view().active_profile_id, "a");
}

#[test]
fn utf8_bom_prefixed_file_loads() {
    let env = Env::new();
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(br#"{"hotkey":"Alt+B"}"#);
    env.write_raw(&bytes);
    let s = env.open();
    assert_eq!(s.load_status(), SettingsLoadStatus::Ok);
    assert_eq!(s.view().hotkey, "Alt+B");
}

#[test]
fn non_object_or_invalid_json_is_corrupt() {
    let env = Env::new();
    for raw in [
        &b"[1,2,3]"[..],
        b"\"text\"",
        b"null",
        b"{\"a\":",
        b"",
        b"\xff\xfe\x00garbage",
    ] {
        env.write_raw(raw);
        let s = env.open();
        assert_eq!(s.load_status(), SettingsLoadStatus::Corrupt, "{raw:?}");
        let issue = s.view().load_issue.expect("issue surfaced");
        assert_eq!(issue.status, SettingsLoadStatus::Corrupt);
        assert!(issue.backup_path.is_none());
        assert!(!issue.writes_blocked);
        assert_eq!(s.view().profiles[0].id, "default");
    }
}
