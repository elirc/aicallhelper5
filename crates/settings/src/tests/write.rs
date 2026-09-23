use std::sync::Barrier;

use callcore_contract::{AnswerStyle, Bounds, CallType, LayoutMode, Profile};
use serde_json::json;

use super::*;

#[test]
fn stale_revision_rejected_nothing_applied_file_identical() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(SettingsPatch {
        hotkey: Some("Alt+1".into()),
        ..patch(0)
    })
    .unwrap();
    let before = env.bytes();
    let err = s
        .apply_patch(SettingsPatch {
            hotkey: Some("Alt+2".into()),
            secrets: Some(vec![callcore_contract::SecretChange {
                key_id: "groq".into(),
                action: callcore_contract::SecretAction::Set {
                    value: "gsk-x".into(),
                },
            }]),
            ..patch(0)
        })
        .unwrap_err();
    assert_eq!(err.code, callcore_contract::ErrorCode::Internal);
    assert!(err.message.contains("nothing was saved"), "{}", err.message);
    assert_eq!(env.bytes(), before);
    assert_eq!(s.view().hotkey, "Alt+1");
    assert_eq!(s.revision(), 1);
    assert_eq!(
        env.ks
            .protect_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no encryption attempted"
    );
}

#[test]
fn future_revision_is_also_rejected() {
    let env = Env::new();
    let s = env.open();
    assert!(s.apply_patch(patch(7)).is_err());
    assert!(!env.path().exists());
}

#[test]
fn revision_bumps_exactly_once_per_save() {
    let env = Env::new();
    let s = env.open();
    for i in 0..5u64 {
        let v = s
            .apply_patch(SettingsPatch {
                answer_style: Some(AnswerStyle::Brief),
                hotkey: Some(format!("Alt+{i}")),
                always_on_top: Some(false),
                ..patch(i)
            })
            .unwrap();
        assert_eq!(v.settings_revision, i + 1);
        assert_eq!(env.json()["settingsRevision"], i + 1);
    }
    assert_eq!(
        env.open().revision(),
        5,
        "revision persists across restarts"
    );
}

#[test]
fn save_bounds_persists_without_bumping_revision() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(patch(0)).unwrap();
    let full = Bounds {
        x: -1920,
        y: 10,
        width: 1000,
        height: 700,
    };
    let prompter = Bounds {
        x: 5,
        y: 6,
        width: 500,
        height: 120,
    };
    s.save_bounds(LayoutMode::Full, full).unwrap();
    s.save_bounds(LayoutMode::Prompter, prompter).unwrap();
    assert_eq!(s.revision(), 1);
    assert_eq!(s.bounds(LayoutMode::Full), Some(full));
    let j = env.json();
    assert_eq!(j["settingsRevision"], 1);
    assert_eq!(
        j["windowBounds"],
        json!({"x": -1920, "y": 10, "width": 1000, "height": 700})
    );
    let re = env.open();
    assert_eq!(re.bounds(LayoutMode::Prompter), Some(prompter));
    // A patch based on revision 1 still applies after geometry saves.
    s.apply_patch(patch(1)).unwrap();
}

#[test]
fn save_bounds_rejects_insane_sizes() {
    let env = Env::new();
    let s = env.open();
    for b in [
        Bounds {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
        },
        Bounds {
            x: 0,
            y: 0,
            width: 10,
            height: 0,
        },
        Bounds {
            x: 0,
            y: 0,
            width: 100_000,
            height: 10,
        },
    ] {
        assert!(s.save_bounds(LayoutMode::Full, b).is_err());
    }
    assert!(!env.path().exists());
}

fn profiles_n(n: usize) -> Vec<Profile> {
    (0..n)
        .map(|i| profile(&format!("p{i}"), &format!("P{i}")))
        .collect()
}

#[test]
fn invalid_patch_values_rejected_nothing_applied() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(patch(0)).unwrap();
    let before = env.bytes();
    let before_view = s.view();
    let mut long_name = profile("a", "A");
    long_name.name = "n".repeat(61);
    let mut long_focus = profile("a", "A");
    long_focus.focus = "f".repeat(2001);
    let mut long_resume = profile("a", "A");
    long_resume.resume = "r".repeat(200_001);
    let mut long_jd = profile("a", "A");
    long_jd.job_description = "j".repeat(200_001);
    let mut long_notes = profile("a", "A");
    long_notes.notes = "n".repeat(200_001);
    let cases: Vec<(&str, SettingsPatch)> = vec![
        (
            "odd prompter font",
            SettingsPatch {
                prompter_font_px: Some(15),
                ..patch(1)
            },
        ),
        (
            "prompter font too big",
            SettingsPatch {
                prompter_font_px: Some(30),
                ..patch(1)
            },
        ),
        (
            "prompter font too small",
            SettingsPatch {
                prompter_font_px: Some(12),
                ..patch(1)
            },
        ),
        (
            "odd answer font",
            SettingsPatch {
                answer_font_px: Some(13),
                ..patch(1)
            },
        ),
        (
            "answer font too big",
            SettingsPatch {
                answer_font_px: Some(24),
                ..patch(1)
            },
        ),
        (
            "unknown provider",
            SettingsPatch {
                llm_provider: Some("openai".into()),
                ..patch(1)
            },
        ),
        (
            "hotkey too long",
            SettingsPatch {
                hotkey: Some("k".repeat(101)),
                ..patch(1)
            },
        ),
        (
            "no profiles",
            SettingsPatch {
                profiles: Some(vec![]),
                ..patch(1)
            },
        ),
        (
            "21 profiles",
            SettingsPatch {
                profiles: Some(profiles_n(21)),
                ..patch(1)
            },
        ),
        (
            "bad id",
            SettingsPatch {
                profiles: Some(vec![profile("bad id", "X")]),
                ..patch(1)
            },
        ),
        (
            "long id",
            SettingsPatch {
                profiles: Some(vec![profile(&"x".repeat(65), "X")]),
                ..patch(1)
            },
        ),
        (
            "dup ids",
            SettingsPatch {
                profiles: Some(vec![profile("a", "A"), profile("a", "B")]),
                ..patch(1)
            },
        ),
        (
            "blank name",
            SettingsPatch {
                profiles: Some(vec![profile("a", "   ")]),
                ..patch(1)
            },
        ),
        (
            "long name",
            SettingsPatch {
                profiles: Some(vec![long_name]),
                ..patch(1)
            },
        ),
        (
            "long focus",
            SettingsPatch {
                profiles: Some(vec![long_focus]),
                ..patch(1)
            },
        ),
        (
            "long resume",
            SettingsPatch {
                profiles: Some(vec![long_resume]),
                ..patch(1)
            },
        ),
        (
            "long jd",
            SettingsPatch {
                profiles: Some(vec![long_jd]),
                ..patch(1)
            },
        ),
        (
            "long notes",
            SettingsPatch {
                profiles: Some(vec![long_notes]),
                ..patch(1)
            },
        ),
        (
            "unknown active",
            SettingsPatch {
                active_profile_id: Some("ghost".into()),
                ..patch(1)
            },
        ),
        (
            "valid fields + one invalid",
            SettingsPatch {
                hotkey: Some("Alt+Z".into()),
                always_on_top: Some(false),
                answer_font_px: Some(99),
                ..patch(1)
            },
        ),
    ];
    for (label, p) in cases {
        let err = s.apply_patch(p).expect_err(label);
        assert!(
            err.message.contains("Nothing was saved"),
            "{label}: {}",
            err.message
        );
        assert_eq!(env.bytes(), before, "{label}");
        assert_eq!(s.view(), before_view, "{label}");
    }
}

#[test]
fn profile_text_error_does_not_echo_profile_text() {
    let env = Env::new();
    let s = env.open();
    let mut p = profile("a", "A");
    p.resume = format!("SECRET-RESUME-{}", "r".repeat(200_000));
    let err = s
        .apply_patch(SettingsPatch {
            profiles: Some(vec![p]),
            ..patch(0)
        })
        .unwrap_err();
    assert!(!err.message.contains("SECRET-RESUME"), "{}", err.message);
}

#[test]
fn valid_patch_applies_every_field() {
    let env = Env::new();
    let s = env.open();
    let mut a = profile("a", "  Alpha  ");
    a.call_type = CallType::Sales;
    a.resume = "é".repeat(200_000);
    let v = s
        .apply_patch(SettingsPatch {
            profiles: Some(vec![a, profile("b", "Beta")]),
            active_profile_id: Some("b".into()),
            llm_provider: Some("groq".into()),
            answer_style: Some(AnswerStyle::Detailed),
            hotkey: Some(String::new()),
            always_on_top: Some(false),
            layout_mode: Some(LayoutMode::Prompter),
            prompter_font_px: Some(28),
            answer_font_px: Some(12),
            ..patch(0)
        })
        .unwrap();
    assert_eq!(v.profiles[0].name, "Alpha", "names are trimmed on save");
    assert_eq!(v.active_profile_id, "b");
    assert_eq!(v.llm_provider, "groq");
    assert_eq!(v.hotkey, "", "empty hotkey = disabled, kept");
    assert_eq!((v.prompter_font_px, v.answer_font_px), (28, 12));
    let re = env.open().view();
    assert_eq!(re.profiles, v.profiles);
    assert_eq!(re.active_profile_id, "b");
    assert_eq!(re.layout_mode, LayoutMode::Prompter);
    assert!(!re.always_on_top);
    assert_eq!(re.answer_style, AnswerStyle::Detailed);
}

#[test]
fn replacing_profiles_without_active_falls_back_to_first() {
    let env = Env::new();
    let s = env.open();
    let v = s
        .apply_patch(SettingsPatch {
            profiles: Some(vec![profile("x", "X"), profile("y", "Y")]),
            ..patch(0)
        })
        .unwrap();
    assert_eq!(v.active_profile_id, "x");
    let v = s
        .apply_patch(SettingsPatch {
            profiles: Some(vec![profile("y", "Y"), profile("z", "Z")]),
            active_profile_id: Some("z".into()),
            ..patch(1)
        })
        .unwrap();
    assert_eq!(v.active_profile_id, "z");
}

#[test]
fn twenty_profiles_accepted() {
    let env = Env::new();
    let s = env.open();
    assert_eq!(
        s.apply_patch(SettingsPatch {
            profiles: Some(profiles_n(20)),
            ..patch(0)
        })
        .unwrap()
        .profiles
        .len(),
        20
    );
}

#[test]
fn concurrent_same_base_patches_exactly_one_wins() {
    for _ in 0..10 {
        let env = Env::new();
        let s = Arc::new(env.open());
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = ["Alt+A", "Alt+B"]
            .into_iter()
            .map(|hk| {
                let (s, b) = (s.clone(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    s.apply_patch(SettingsPatch {
                        hotkey: Some(hk.into()),
                        ..patch(0)
                    })
                    .map(|v| v.hotkey)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners: Vec<&String> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(winners.len(), 1, "{results:?}");
        assert_eq!(s.revision(), 1);
        assert_eq!(&s.view().hotkey, winners[0]);
        assert_eq!(
            env.json()["hotkey"],
            winners[0].as_str(),
            "disk matches the winner"
        );
    }
}

#[test]
fn concurrent_writers_are_serialized_without_lost_updates() {
    let env = Env::new();
    let s = Arc::new(env.open());
    let threads = 6;
    let per = 5;
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let s = s.clone();
            std::thread::spawn(move || {
                let mut done = 0;
                while done < per {
                    let base = s.revision();
                    if s.apply_patch(SettingsPatch {
                        hotkey: Some(format!("T{t}-{done}")),
                        ..patch(base)
                    })
                    .is_ok()
                    {
                        done += 1;
                    }
                    // Geometry autosaves interleave with patches.
                    s.save_bounds(
                        LayoutMode::Full,
                        Bounds {
                            x: t,
                            y: done,
                            width: 100,
                            height: 100,
                        },
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(s.revision(), (threads * per) as u64);
    assert_eq!(env.json()["settingsRevision"], threads * per);
    assert!(
        env.others().is_empty(),
        "no temp files left: {:?}",
        env.others()
    );
}

#[test]
fn atomic_write_leaves_no_temp_files() {
    let env = Env::new();
    let s = env.open();
    for i in 0..10 {
        s.apply_patch(patch(i)).unwrap();
        s.save_bounds(
            LayoutMode::Full,
            Bounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        )
        .unwrap();
    }
    assert!(env.others().is_empty(), "{:?}", env.others());
}

#[test]
fn failed_write_leaves_memory_and_no_temp_file() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(patch(0)).unwrap();
    // Replace the target with a non-empty directory: rename over it fails.
    std::fs::remove_file(env.path()).unwrap();
    std::fs::create_dir(env.path()).unwrap();
    std::fs::write(env.path().join("x"), b"x").unwrap();
    let err = s
        .apply_patch(SettingsPatch {
            hotkey: Some("Alt+F".into()),
            ..patch(1)
        })
        .unwrap_err();
    assert!(err.message.contains("Nothing was saved"), "{}", err.message);
    assert_eq!(s.revision(), 1, "memory unchanged when disk write fails");
    assert_ne!(s.view().hotkey, "Alt+F");
    assert!(env.others().is_empty(), "temp cleaned: {:?}", env.others());
}

#[test]
fn written_file_is_pretty_camel_case_v4() {
    let env = Env::new();
    let s = env.open();
    s.apply_patch(patch(0)).unwrap();
    let text = env.text();
    assert!(text.contains("\n  \"version\": 4"), "{text}");
    let j = env.json();
    let obj = j.as_object().unwrap();
    for key in [
        "version",
        "settingsRevision",
        "profiles",
        "activeProfileId",
        "llmProvider",
        "answerStyle",
        "hotkey",
        "alwaysOnTop",
        "layoutMode",
        "prompterFontPx",
        "answerFontPx",
        "windowBounds",
        "prompterBounds",
        "secrets",
    ] {
        assert!(obj.contains_key(key), "missing {key}");
    }
    assert_eq!(obj.len(), 14, "{obj:?}");
    assert_eq!(j["profiles"][0]["jobDescription"], "");
    assert_eq!(j["profiles"][0]["callType"], "behavioral");
    assert_eq!(j["answerStyle"], "balanced");
    assert_eq!(j["layoutMode"], "full");
}

#[test]
fn answer_config_reflects_active_profile_style_provider() {
    use callcore_contract::ports::SettingsReader;
    let env = Env::new();
    let s = env.open();
    let mut b = profile("b", "B");
    b.call_type = CallType::SystemDesign;
    b.resume = "resume B".into();
    s.apply_patch(SettingsPatch {
        profiles: Some(vec![profile("a", "A"), b.clone()]),
        active_profile_id: Some("b".into()),
        answer_style: Some(AnswerStyle::Brief),
        llm_provider: Some("groq".into()),
        ..patch(0)
    })
    .unwrap();
    let cfg = s.answer_config();
    assert_eq!(cfg.profile, b);
    assert_eq!(cfg.call_type, CallType::SystemDesign);
    assert_eq!(cfg.style, AnswerStyle::Brief);
    assert_eq!(cfg.provider_id, "groq");
}
