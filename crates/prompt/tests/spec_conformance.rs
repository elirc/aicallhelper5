//! Parses `docs/SPEC.md` §8 at test time and checks every prose string in the
//! crate is byte-identical to the spec (guards against transcription typos in
//! the long role texts). The spec is read-only for build agents, so this test
//! only fails if the crate drifts, or if the spec is deliberately changed.

use callcore_contract::{AnswerStyle, CallType};
use callcore_prompt::*;

fn spec_section_8() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SPEC.md");
    let spec = std::fs::read_to_string(&path)
        .expect("docs/SPEC.md readable")
        .replace("\r\n", "\n");
    let start = spec.find("## 8. Prompt construction").expect("§8 heading");
    let end = spec[start..].find("\n## 9.").expect("§9 heading") + start;
    spec[start..end].to_owned()
}

/// All `"..."` segments on a line (spec strings contain no inner `"`), with
/// the spec's `\n` escapes turned into real newlines.
fn quoted(line: &str) -> Vec<String> {
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.replace("\\n", "\n"))
        .collect()
}

#[test]
fn preamble_matches_spec() {
    let sec = spec_section_8();
    let line = sec
        .lines()
        .find(|l| l.starts_with("PREAMBLE = "))
        .expect("PREAMBLE line");
    assert_eq!(quoted(line), vec![preamble().to_owned()]);
}

#[test]
fn call_types_match_spec() {
    let sec = spec_section_8();
    let mut seen = 0;
    let mut current: Option<CallType> = None;
    for line in sec.lines() {
        if let Some(rest) = line
            .split_once(". `")
            .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
            .map(|(_, r)| r)
        {
            let id = rest.split('`').next().unwrap();
            let ct = CallType::ALL
                .into_iter()
                .find(|c| c.id() == id)
                .unwrap_or_else(|| panic!("unknown id {id}"));
            let q = quoted(line);
            let info = call_type_info(ct);
            assert_eq!(
                q,
                vec![info.label.to_owned(), info.jd_header.to_owned()],
                "{id} header line"
            );
            if line.contains("(DEFAULT)") {
                assert_eq!(ct, CallType::default());
            }
            current = Some(ct);
            seen += 1;
            continue;
        }
        let Some(ct) = current else { continue };
        let t = line.trim_start();
        if let Some(r) = t.strip_prefix("role: ") {
            let q = quoted(r);
            assert_eq!(q.len(), 1, "{}", ct.id());
            let expected = if r.starts_with("PREAMBLE + ") {
                format!("{}{}", preamble(), q[0])
            } else {
                q[0].clone()
            };
            assert_eq!(role(ct), expected, "role for {}", ct.id());
        } else if let Some(g) = t.strip_prefix("grounding: ") {
            assert_eq!(
                quoted(g),
                vec![grounding(ct).to_owned()],
                "grounding for {}",
                ct.id()
            );
        }
    }
    assert_eq!(seen, 6);
}

#[test]
fn style_suffixes_match_spec() {
    let sec = spec_section_8();
    let find = |prefix: &str| {
        let line = sec
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix}"));
        quoted(line)
    };
    assert_eq!(
        find("- brief: "),
        vec![style_suffix(AnswerStyle::Brief).to_owned()]
    );
    assert_eq!(
        find("- balanced (default): "),
        vec![style_suffix(AnswerStyle::Balanced).to_owned()]
    );
    assert_eq!(
        find("- detailed: "),
        vec![style_suffix(AnswerStyle::Detailed).to_owned()]
    );
    assert_eq!(AnswerStyle::default(), AnswerStyle::Balanced);
}

#[test]
fn section_headers_and_user_message_match_spec() {
    let sec = spec_section_8();
    assert!(sec.contains(r#"`"\n\n--- THE USER'S RESUME ---\n" + resume`"#));
    assert!(sec.contains(r#"`"\n\n--- " + jd_header + " ---\n" + jd`"#));
    assert!(sec.contains(r#"`"\n\n--- FOCUS FOR THIS CALL ---\n" + focus`"#));
    assert!(sec.contains(r#"`"\n\n--- THE USER'S NOTES ---\n" + notes`"#));
    assert_eq!(RESUME_HEADER, "\n\n--- THE USER'S RESUME ---\n");
    assert_eq!(FOCUS_HEADER, "\n\n--- FOCUS FOR THIS CALL ---\n");
    assert_eq!(NOTES_HEADER, "\n\n--- THE USER'S NOTES ---\n");
    assert!(sec.contains(
        r#"`"The other person on the call just said:\n\"\"\"\n" + transcript + "\n\"\"\"\n\nWhat should I say?"`"#
    ));
    assert_eq!(
        user_message("{transcript}"),
        "The other person on the call just said:\n\"\"\"\n{transcript}\n\"\"\"\n\nWhat should I say?"
    );
}
