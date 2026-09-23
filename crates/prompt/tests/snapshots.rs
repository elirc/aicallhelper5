//! Byte-exact snapshot tests: every call type x style x field case.
//!
//! Snapshots live in `tests/snapshots/<call_type>.<style>.<case>.txt`.
//! Regenerate (only after reviewing the diff!) with
//! `UPDATE_SNAPSHOTS=1 cargo test -p callcore-prompt --test snapshots`.
//! `tests/snapshots/.gitattributes` marks them `-text` so git never rewrites
//! their line endings on Windows checkouts.

use callcore_contract::CallType;
use callcore_prompt::{build_prompt, style_id, PromptInput, ANSWER_STYLES};
use std::path::PathBuf;

const TRANSCRIPT: &str =
    "Can you walk me through \"the\" project?\n  Second line, with trailing space. ";

struct Case {
    name: &'static str,
    resume: &'static str,
    jd: &'static str,
    focus: &'static str,
    notes: &'static str,
}

const CASES: [Case; 3] = [
    Case {
        name: "empty",
        resume: "",
        jd: "",
        focus: "",
        notes: "",
    },
    Case {
        name: "full",
        resume: "  \n Jane Doe — Senior Engineer\n\n  - Led the ledger migration\t(Go)  \n\n",
        jd: "\tStaff Engineer, Payments.  Own reliability.\n",
        focus: " Lead with Go ",
        notes: "Notice: 4 weeks.\n\nComp: 210–240k.\n",
    },
    Case {
        name: "mixed",
        resume: "Resume only line.",
        jd: "  \n\t ",
        focus: "",
        notes: "  Notes only.  ",
    },
];

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

fn render(call_type: CallType, style: callcore_contract::AnswerStyle, case: &Case) -> String {
    let parts = build_prompt(
        &PromptInput {
            call_type,
            style,
            resume: case.resume,
            job_description: case.jd,
            focus: case.focus,
            notes: case.notes,
        },
        TRANSCRIPT,
    );
    format!(
        "=== cached_prefix ===\n{}\n=== style_suffix ===\n{}\n=== user_message ===\n{}\n=== end ===\n",
        parts.cached_prefix, parts.style_suffix, parts.user_message
    )
}

#[test]
fn snapshots_every_call_type_style_and_field_case() {
    let dir = snapshot_dir();
    let update = std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1");
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut failures = Vec::new();
    let mut expected_files = Vec::new();
    for ct in CallType::ALL {
        for style in ANSWER_STYLES {
            for case in &CASES {
                let name = format!("{}.{}.{}.txt", ct.id(), style_id(style), case.name);
                let path = dir.join(&name);
                let actual = render(ct, style, case);
                expected_files.push(name.clone());
                if update {
                    std::fs::write(&path, actual.as_bytes()).unwrap();
                    continue;
                }
                match std::fs::read(&path) {
                    Ok(bytes) if bytes == actual.as_bytes() => {}
                    Ok(bytes) => {
                        let hint = if bytes.contains(&b'\r') {
                            " (file contains CR — line endings were rewritten; see .gitattributes)"
                        } else {
                            ""
                        };
                        failures.push(format!("{name}: bytes differ{hint}"));
                    }
                    Err(e) => failures.push(format!("{name}: missing ({e})")),
                }
            }
        }
    }
    assert_eq!(expected_files.len(), 54);
    // No stale snapshot files left behind by renamed cases.
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".txt"))
        .collect();
    on_disk.sort();
    expected_files.sort();
    assert_eq!(on_disk, expected_files, "stale or missing snapshot files");
    assert!(
        failures.is_empty(),
        "{} snapshot mismatch(es) — review, then UPDATE_SNAPSHOTS=1 to accept:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn snapshot_attributes_file_keeps_bytes_exact() {
    let attrs = std::fs::read_to_string(snapshot_dir().join(".gitattributes")).unwrap();
    assert!(attrs.lines().any(|l| l.trim() == "* -text"), "{attrs}");
}
