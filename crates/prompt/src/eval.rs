//! Offline prompt-eval model (spec §8 "offline prompt-eval harness").
//!
//! Pure data + rule checking only: no network, no provider. The
//! `prompt_eval` example builds every fixture's prompt; a live-run tool (in the
//! llm area) can load the same [`EvalSuite`], send the prompts under its own
//! cost budget and score each real answer with [`check_answer`].
//!
//! Fixture file: `crates/prompt/eval/fixtures.json` (also available compiled
//! in as [`BUNDLED_FIXTURES_JSON`]).

use callcore_contract::{AnswerStyle, CallType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{build_prompt, PromptInput};
use callcore_contract::ports::PromptParts;

/// The bundled fixture file, compiled in so tools need no path handling.
pub const BUNDLED_FIXTURES_JSON: &str = include_str!("../eval/fixtures.json");

/// A whole fixture file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalSuite {
    /// Schema version of the fixture file.
    pub version: u32,
    /// Named sample profiles, referenced by [`Fixture::profile`]. `BTreeMap`
    /// so iteration (and therefore any output) is deterministic.
    pub profiles: BTreeMap<String, Profile>,
    pub fixtures: Vec<Fixture>,
}

/// A sample profile (the four free-text prompt fields).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub resume: String,
    #[serde(default)]
    pub job_description: String,
    #[serde(default)]
    pub focus: String,
    #[serde(default)]
    pub notes: String,
}

impl std::fmt::Debug for Profile {
    // Profile text stays out of Debug output (logs / panics), even for samples,
    // so the habit is uniform with real profiles.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("resume_len", &self.resume.len())
            .field("job_description_len", &self.job_description.len())
            .field("focus_len", &self.focus.len())
            .field("notes_len", &self.notes.len())
            .finish()
    }
}

/// One eval case: a question asked on a call of a given type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Fixture {
    /// Unique, filename-safe id (e.g. `behavioral-conflict`).
    pub id: String,
    pub call_type: CallType,
    /// Key into [`EvalSuite::profiles`].
    pub profile: String,
    /// What the other person said (the transcript).
    pub question: String,
    #[serde(default)]
    pub expect: Expect,
}

/// Rules a real answer must satisfy. All rules are optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Expect {
    /// Case-insensitive substrings that must NOT appear.
    #[serde(default)]
    pub must_not_contain: Vec<String>,
    /// Case-insensitive substrings of which at least ONE must appear
    /// (ignored when empty).
    #[serde(default)]
    pub must_contain_any: Vec<String>,
    /// Per-style upper bound on whitespace-separated words.
    #[serde(default)]
    pub max_words: MaxWords,
    /// Lower bound on words, any style.
    #[serde(default)]
    pub min_words: Option<usize>,
    /// Answer must be spoken as the user: contains a first-person word and
    /// never refers to "the user".
    #[serde(default)]
    pub first_person: bool,
    /// Answer must not be wrapped in quotation marks.
    #[serde(default)]
    pub no_wrapping_quotes: bool,
}

/// Word limits per answer style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaxWords {
    #[serde(default)]
    pub brief: Option<usize>,
    #[serde(default)]
    pub balanced: Option<usize>,
    #[serde(default)]
    pub detailed: Option<usize>,
}

impl MaxWords {
    pub fn for_style(&self, style: AnswerStyle) -> Option<usize> {
        match style {
            AnswerStyle::Brief => self.brief,
            AnswerStyle::Balanced => self.balanced,
            AnswerStyle::Detailed => self.detailed,
        }
    }
}

/// Errors from loading / validating a fixture file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteError {
    Parse(String),
    DuplicateId(String),
    UnknownProfile { fixture: String, profile: String },
    BadId(String),
}

impl std::fmt::Display for SuiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SuiteError::Parse(e) => write!(f, "fixture file does not parse: {e}"),
            SuiteError::DuplicateId(id) => write!(f, "duplicate fixture id `{id}`"),
            SuiteError::UnknownProfile { fixture, profile } => {
                write!(
                    f,
                    "fixture `{fixture}` references unknown profile `{profile}`"
                )
            }
            SuiteError::BadId(id) => {
                write!(
                    f,
                    "fixture id `{id}` must be non-empty [a-z0-9_-] (used as a filename)"
                )
            }
        }
    }
}

impl std::error::Error for SuiteError {}

impl EvalSuite {
    /// Parse and validate a fixture file.
    pub fn from_json(json: &str) -> Result<EvalSuite, SuiteError> {
        let suite: EvalSuite =
            serde_json::from_str(json).map_err(|e| SuiteError::Parse(e.to_string()))?;
        suite.validate()?;
        Ok(suite)
    }

    /// The fixtures bundled with this crate.
    pub fn bundled() -> Result<EvalSuite, SuiteError> {
        EvalSuite::from_json(BUNDLED_FIXTURES_JSON)
    }

    pub fn validate(&self) -> Result<(), SuiteError> {
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.fixtures {
            if f.id.is_empty()
                || !f
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            {
                return Err(SuiteError::BadId(f.id.clone()));
            }
            if !seen.insert(f.id.as_str()) {
                return Err(SuiteError::DuplicateId(f.id.clone()));
            }
            if !self.profiles.contains_key(&f.profile) {
                return Err(SuiteError::UnknownProfile {
                    fixture: f.id.clone(),
                    profile: f.profile.clone(),
                });
            }
        }
        Ok(())
    }

    /// The profile a fixture uses (validated to exist by [`Self::validate`]).
    pub fn profile_for(&self, fixture: &Fixture) -> Option<&Profile> {
        self.profiles.get(&fixture.profile)
    }

    /// Build the exact prompt the app would send for `fixture` in `style`.
    pub fn build(&self, fixture: &Fixture, style: AnswerStyle) -> Option<PromptParts> {
        let p = self.profile_for(fixture)?;
        let input = PromptInput {
            call_type: fixture.call_type,
            style,
            resume: &p.resume,
            job_description: &p.job_description,
            focus: &p.focus,
            notes: &p.notes,
        };
        Some(build_prompt(&input, &fixture.question))
    }
}

/// Rough token estimate used for budgeting: `chars / 4`, rounded up.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Whitespace-separated word count.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

const FIRST_PERSON: [&str; 12] = [
    "i", "i'm", "i've", "i'd", "i'll", "me", "my", "mine", "we", "we're", "our", "us",
];

fn has_first_person(answer: &str) -> bool {
    answer
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}'))
        .filter(|w| !w.is_empty())
        .map(|w| w.replace('\u{2019}', "'").to_lowercase())
        .any(|w| FIRST_PERSON.contains(&w.as_str()))
}

fn is_wrapped_in_quotes(answer: &str) -> bool {
    let t = answer.trim();
    let mut chars = t.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return false;
    };
    matches!(
        (first, last),
        ('"', '"') | ('\u{201C}', '\u{201D}') | ('\'', '\'')
    )
}

/// Score a real answer against a fixture's rules. Returns one human-readable
/// line per violated rule; an empty vector means the answer passes. Pure and
/// deterministic (violations come out in a fixed rule order).
///
/// An empty / whitespace-only answer is always a violation. Violation text
/// quotes the rule, never the answer, so it is safe to log.
pub fn check_answer(fixture: &Fixture, style: AnswerStyle, answer: &str) -> Vec<String> {
    let mut v = Vec::new();
    let e = &fixture.expect;
    if answer.trim().is_empty() {
        v.push("answer is empty".to_owned());
        return v;
    }
    let lower = answer.to_lowercase();
    for bad in &e.must_not_contain {
        if lower.contains(&bad.to_lowercase()) {
            v.push(format!("must_not_contain: found {bad:?}"));
        }
    }
    if !e.must_contain_any.is_empty()
        && !e
            .must_contain_any
            .iter()
            .any(|w| lower.contains(&w.to_lowercase()))
    {
        v.push(format!(
            "must_contain_any: none of {:?} found",
            e.must_contain_any
        ));
    }
    let words = word_count(answer);
    if let Some(max) = e.max_words.for_style(style) {
        if words > max {
            v.push(format!(
                "max_words ({}): {words} words > {max}",
                crate::style_id(style)
            ));
        }
    }
    if let Some(min) = e.min_words {
        if words < min {
            v.push(format!("min_words: {words} words < {min}"));
        }
    }
    if e.first_person {
        if !has_first_person(answer) {
            v.push("first_person: no first-person word (I/me/my/we/our...)".to_owned());
        }
        if lower.contains("the user") {
            v.push("first_person: refers to \"the user\" instead of speaking as them".to_owned());
        }
    }
    if e.no_wrapping_quotes && is_wrapped_in_quotes(answer) {
        v.push("no_wrapping_quotes: answer is wrapped in quotation marks".to_owned());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(expect: Expect) -> Fixture {
        Fixture {
            id: "t".into(),
            call_type: CallType::Behavioral,
            profile: "p".into(),
            question: "Tell me about yourself.".into(),
            expect,
        }
    }

    fn strict() -> Expect {
        Expect {
            must_not_contain: vec!["As an AI".into(), "\"\"\"".into()],
            must_contain_any: vec![],
            max_words: MaxWords {
                brief: Some(10),
                balanced: Some(20),
                detailed: Some(40),
            },
            min_words: Some(3),
            first_person: true,
            no_wrapping_quotes: true,
        }
    }

    #[test]
    fn check_answer_passes_good_answer() {
        let f = fixture(strict());
        assert!(check_answer(&f, AnswerStyle::Brief, "I led the billing migration.").is_empty());
    }

    #[test]
    fn check_answer_flags_empty() {
        let f = fixture(strict());
        assert_eq!(
            check_answer(&f, AnswerStyle::Balanced, "  \n\t"),
            vec!["answer is empty"]
        );
        // Even with no rules at all.
        assert_eq!(
            check_answer(&fixture(Expect::default()), AnswerStyle::Brief, ""),
            vec!["answer is empty"]
        );
    }

    #[test]
    fn check_answer_must_not_contain_is_case_insensitive() {
        let f = fixture(strict());
        let v = check_answer(&f, AnswerStyle::Detailed, "as an ai, I cannot say.");
        assert_eq!(v, vec!["must_not_contain: found \"As an AI\""]);
        let v = check_answer(&f, AnswerStyle::Detailed, "I said \"\"\" there");
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].starts_with("must_not_contain"));
    }

    #[test]
    fn check_answer_max_words_is_per_style() {
        let f = fixture(strict());
        let ans = "I did one two three four five six seven eight nine ten eleven";
        assert_eq!(word_count(ans), 13);
        let v = check_answer(&f, AnswerStyle::Brief, ans);
        assert_eq!(v, vec!["max_words (brief): 13 words > 10"]);
        assert!(check_answer(&f, AnswerStyle::Balanced, ans).is_empty());
        assert!(check_answer(&f, AnswerStyle::Detailed, ans).is_empty());
    }

    #[test]
    fn check_answer_min_words() {
        let f = fixture(strict());
        assert_eq!(
            check_answer(&f, AnswerStyle::Brief, "I agree"),
            vec!["min_words: 2 words < 3"]
        );
    }

    #[test]
    fn check_answer_first_person() {
        let f = fixture(strict());
        let v = check_answer(
            &f,
            AnswerStyle::Balanced,
            "The user should mention the migration.",
        );
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].contains("no first-person"));
        assert!(v[1].contains("the user"));
        // Curly apostrophes count ("I’ve").
        assert!(check_answer(
            &f,
            AnswerStyle::Balanced,
            "Honestly, I\u{2019}ve shipped that twice."
        )
        .is_empty());
        // "I" inside another word does not count.
        let v = check_answer(
            &f,
            AnswerStyle::Balanced,
            "Kubernetes is interesting indeed.",
        );
        assert!(v.iter().any(|s| s.contains("no first-person")), "{v:?}");
    }

    #[test]
    fn check_answer_wrapping_quotes() {
        let f = fixture(strict());
        let v = check_answer(&f, AnswerStyle::Balanced, "  \"I led the migration.\"  ");
        assert_eq!(
            v,
            vec!["no_wrapping_quotes: answer is wrapped in quotation marks"]
        );
        let v = check_answer(
            &f,
            AnswerStyle::Balanced,
            "\u{201C}I led the migration.\u{201D}",
        );
        assert_eq!(v.len(), 1);
        // Interior quotes are fine.
        assert!(check_answer(
            &f,
            AnswerStyle::Balanced,
            "I called it \"project blue\" then."
        )
        .is_empty());
    }

    #[test]
    fn check_answer_must_contain_any() {
        let e = Expect {
            must_contain_any: vec!["range".into(), "flexible".into()],
            ..Expect::default()
        };
        let f = fixture(e);
        assert!(check_answer(&f, AnswerStyle::Brief, "I'm FLEXIBLE on that.").is_empty());
        let v = check_answer(&f, AnswerStyle::Brief, "One hundred thousand.");
        assert_eq!(v.len(), 1);
        assert!(v[0].starts_with("must_contain_any"));
    }

    #[test]
    fn check_answer_violations_do_not_echo_answer() {
        let f = fixture(strict());
        let secret = "The user PRIVATE-DETAIL-XYZ";
        for line in check_answer(&f, AnswerStyle::Brief, secret) {
            assert!(!line.contains("PRIVATE-DETAIL-XYZ"), "{line}");
        }
    }

    #[test]
    fn bundled_fixtures_parse_and_cover_every_call_type() {
        let suite = EvalSuite::bundled().expect("bundled fixtures valid");
        for ct in CallType::ALL {
            let n = suite.fixtures.iter().filter(|f| f.call_type == ct).count();
            assert!(n >= 4, "{} has only {n} fixtures", ct.id());
        }
        assert!(suite.fixtures.len() >= 24);
        for f in &suite.fixtures {
            assert!(!f.question.trim().is_empty(), "{}", f.id);
            assert!(
                f.expect.must_not_contain.iter().any(|s| s == "\"\"\""),
                "{}",
                f.id
            );
            assert!(
                f.expect.must_not_contain.iter().any(|s| s == "As an AI"),
                "{}",
                f.id
            );
            for style in crate::ANSWER_STYLES {
                assert!(
                    f.expect.max_words.for_style(style).is_some(),
                    "{} {style:?}",
                    f.id
                );
                assert!(suite.build(f, style).is_some());
            }
        }
    }

    #[test]
    fn suite_validation_rejects_bad_files() {
        let base = r#"{"version":1,"profiles":{"p":{}},"fixtures":[FIX]}"#;
        let one = |id: &str, prof: &str| {
            format!(r#"{{"id":"{id}","callType":"sales","profile":"{prof}","question":"q"}}"#)
        };
        assert!(EvalSuite::from_json(&base.replace("FIX", &one("a", "p"))).is_ok());
        assert_eq!(
            EvalSuite::from_json(
                &base.replace("FIX", &format!("{},{}", one("a", "p"), one("a", "p")))
            ),
            Err(SuiteError::DuplicateId("a".into()))
        );
        assert!(matches!(
            EvalSuite::from_json(&base.replace("FIX", &one("a", "zz"))),
            Err(SuiteError::UnknownProfile { .. })
        ));
        assert_eq!(
            EvalSuite::from_json(&base.replace("FIX", &one("A/B", "p"))),
            Err(SuiteError::BadId("A/B".into()))
        );
        assert!(matches!(
            EvalSuite::from_json(r#"{"version":1,"profiles":{},"fixtures":[],"extra":1}"#),
            Err(SuiteError::Parse(_))
        ));
    }

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("——"), 1);
    }

    #[test]
    fn profile_debug_is_redacted() {
        let p = Profile {
            resume: "PRIVATE".into(),
            ..Profile::default()
        };
        assert!(!format!("{p:?}").contains("PRIVATE"));
    }
}
