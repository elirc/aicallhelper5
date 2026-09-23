//! Byte-stable prompt construction (spec §8). PUBLIC API PINNED — the session
//! crate and the eval harness call exactly these.
//!
//! Everything here is pure and deterministic: no timestamps, no hash-map
//! iteration, no allocation-order dependence. The same [`PromptInput`] and
//! transcript always produce byte-identical [`PromptParts`], and flipping only
//! the answer style never changes `cached_prefix` (so provider prompt caches
//! survive style flips).

use callcore_contract::ports::PromptParts;
use callcore_contract::{AnswerStyle, CallType};

pub mod eval;

/// The profile fields that go into the prompt. Borrowed so the hot path does
/// not clone 200 kB resumes.
///
/// `Debug` is hand-written and redacted: profile text (resume, notes, …) must
/// never reach logs or panic messages. Only field lengths are shown.
#[derive(Clone, Copy)]
pub struct PromptInput<'a> {
    pub call_type: CallType,
    pub style: AnswerStyle,
    pub resume: &'a str,
    pub job_description: &'a str,
    pub focus: &'a str,
    pub notes: &'a str,
}

impl std::fmt::Debug for PromptInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptInput")
            .field("call_type", &self.call_type)
            .field("style", &self.style)
            .field("resume_len", &self.resume.len())
            .field("job_description_len", &self.job_description.len())
            .field("focus_len", &self.focus.len())
            .field("notes_len", &self.notes.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallTypeInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub jd_header: &'static str,
}

// ─────────────────────────────── spec §8 strings ───────────────────────────────

macro_rules! preamble {
    () => {
        "You are a real-time call assistant helping the user during a live call. You are given a transcript of what the other person just said. Reply with exactly what the user should say next, written in first person, in natural spoken English. Do not add meta commentary, greetings, or quotation marks — output only the words to say."
    };
}

/// The shared preamble that every call-type role except `behavioral` starts with.
pub const PREAMBLE: &str = preamble!();

/// Section header before the resume.
pub const RESUME_HEADER: &str = "\n\n--- THE USER'S RESUME ---\n";
/// Section header before the focus text.
pub const FOCUS_HEADER: &str = "\n\n--- FOCUS FOR THIS CALL ---\n";
/// Section header before the notes.
pub const NOTES_HEADER: &str = "\n\n--- THE USER'S NOTES ---\n";

const JD_INTERVIEW: &str = "THE JOB THEY ARE INTERVIEWING FOR";

const ROLE_BEHAVIORAL: &str = "You are a real-time call assistant helping the user answer questions asked of them during a live interview or call. You are given a transcript of what the other person just said. Reply with the answer the user should say, written in first person, in natural spoken English. Do not add meta commentary, greetings, or quotation marks — output only the answer itself. If the transcript contains no real question, briefly suggest what the user could say next. Answer with a specific example from the user's own experience whenever the question invites one, and make the outcome concrete when the resume gives one. Structure for longer answers: what the situation was, what you did, what the result was.";

const ROLE_TECHNICAL: &str = concat!(
    preamble!(),
    " This is a technical screening interview. Answer the technical question directly and correctly first, using the precise names of the APIs, data structures, or language features involved, then add the one tradeoff or edge case a senior engineer would mention. Prefer the tools and stack named in the focus section; if a question is about a technology you have not used, say how you would approach it rather than bluffing. If the transcript is a coding problem, state the approach and its time and space complexity, not a full code listing. If the transcript contains no real question, briefly suggest a clarifying question the user could ask. Structure for longer answers: the direct answer, how it works, when you would and would not use it."
);

const ROLE_SYSTEM_DESIGN: &str = concat!(
    preamble!(),
    " This is a system design interview. Treat the transcript as a design prompt or a follow-up on one. Start by naming the one or two requirements or constraints that drive the design, then propose the components and how data flows between them, then name the main tradeoff and what you would change at ten times the scale. Ask one clarifying question when the requirements are genuinely ambiguous rather than assuming. If the transcript contains no real question, briefly suggest the next part of the design the user could walk through. Structure for longer answers: requirements and constraints, core components and data flow, the key tradeoff, how it scales or fails."
);

const ROLE_RECRUITER: &str = concat!(
    preamble!(),
    " This is a recruiter or HR screening call. Keep answers short, warm, and positive: one or two sentences that confirm interest, summarize fit, or give a straight logistics answer (availability, location, work authorization, notice period). For compensation questions give a range or defer to the full process — never a single number unless the notes say otherwise. If the transcript contains no real question, briefly suggest a question the user could ask the recruiter about the process or the team. Structure for longer answers: the direct answer, one sentence of relevant background, one sentence of enthusiasm for the role."
);

const ROLE_SALES: &str = concat!(
    preamble!(),
    " This is a sales, customer, or client call where the user represents their company or product. Work out what the other person is really asking for — a feature, a price, reassurance, a next step — and reply with the answer that moves the conversation forward: address an objection with a specific benefit, answer a factual question plainly, or propose the next concrete step. Never invent pricing, capabilities, or commitments; when the notes do not cover a detail, say you will confirm it and move on. If the transcript contains no real question, briefly suggest a discovery question the user could ask. Structure for longer answers: acknowledge their point, the specific answer or benefit, the next step."
);

const ROLE_MEETING: &str = concat!(
    preamble!(),
    " This is a general work meeting or discussion, not an interview. Reply with the most useful contribution the user could make right now: answer the question if one was asked, otherwise offer the one clarifying question, decision, or next step the discussion needs. Keep it collegial and concrete. If the transcript contains no real question, briefly suggest what the user could say next. Structure for longer answers: the point, the reason, the proposed next step."
);

const GROUNDING_BEHAVIORAL: &str = "\n\nGround every answer in the resume and target role above. Never invent experience the resume does not support.";
const GROUNDING_TECHNICAL: &str = "\n\nUse the resume, role, and focus above to choose which technologies and examples to lead with, but answer the technical question on its merits — technical facts do not need to come from the resume. Never claim hands-on experience the resume does not support.";
const GROUNDING_SYSTEM_DESIGN: &str = "\n\nDraw on the systems and scale described in the resume and focus above for concrete examples, and never claim to have built something the resume does not support.";
const GROUNDING_RECRUITER: &str = "\n\nGround every answer in the resume and target role above. Never invent experience or credentials the resume does not support.";
const GROUNDING_SALES: &str = "\n\nGround every claim in the notes and background above. Never invent pricing, features, customers, or commitments they do not support.";
const GROUNDING_MEETING: &str = "\n\nUse the background and notes above for context. Never invent facts, decisions, or commitments they do not support.";

const STYLE_BRIEF: &str = "Answer in one or two spoken sentences — the shortest reply that fully answers the question. No lists, no headings, no lead-in.";
const STYLE_BALANCED: &str = "Be concise and confident: a few sentences for simple questions, short structured points for complex ones.";
const STYLE_DETAILED: &str = "Give a structured answer: one sentence that answers directly, then three to five short supporting points following the structure for longer answers given in the call guidance above. Keep every point short enough to say in one breath — this is spoken aloud, not read.";

/// Every answer style, in UI order.
pub const ANSWER_STYLES: [AnswerStyle; 3] = [
    AnswerStyle::Brief,
    AnswerStyle::Balanced,
    AnswerStyle::Detailed,
];

/// `(call type, label)` pairs in spec order, for UI pickers.
pub const CALL_TYPE_LABELS: [(CallType, &str); 6] = [
    (CallType::Behavioral, "Behavioral interview"),
    (CallType::Technical, "Technical screen"),
    (CallType::SystemDesign, "System design"),
    (CallType::Recruiter, "Recruiter screen"),
    (CallType::Sales, "Sales or customer call"),
    (CallType::Meeting, "General meeting"),
];

// ─────────────────────────────── public helpers ───────────────────────────────

/// Id, UI label and job-description section header for a call type.
pub fn call_type_info(call_type: CallType) -> CallTypeInfo {
    let (label, jd_header) = match call_type {
        CallType::Behavioral => ("Behavioral interview", JD_INTERVIEW),
        CallType::Technical => ("Technical screen", JD_INTERVIEW),
        CallType::SystemDesign => ("System design", JD_INTERVIEW),
        CallType::Recruiter => ("Recruiter screen", JD_INTERVIEW),
        CallType::Sales => ("Sales or customer call", "ABOUT THIS CALL"),
        CallType::Meeting => ("General meeting", "ABOUT THIS MEETING"),
    };
    CallTypeInfo {
        id: call_type.id(),
        label,
        jd_header,
    }
}

/// The shared preamble (spec §8 `PREAMBLE`).
pub fn preamble() -> &'static str {
    PREAMBLE
}

/// The call-type role text that opens `cached_prefix`.
pub fn role(call_type: CallType) -> &'static str {
    match call_type {
        CallType::Behavioral => ROLE_BEHAVIORAL,
        CallType::Technical => ROLE_TECHNICAL,
        CallType::SystemDesign => ROLE_SYSTEM_DESIGN,
        CallType::Recruiter => ROLE_RECRUITER,
        CallType::Sales => ROLE_SALES,
        CallType::Meeting => ROLE_MEETING,
    }
}

/// The grounding sentence appended when any profile field is non-empty.
/// Starts with `"\n\n"`.
pub fn grounding(call_type: CallType) -> &'static str {
    match call_type {
        CallType::Behavioral => GROUNDING_BEHAVIORAL,
        CallType::Technical => GROUNDING_TECHNICAL,
        CallType::SystemDesign => GROUNDING_SYSTEM_DESIGN,
        CallType::Recruiter => GROUNDING_RECRUITER,
        CallType::Sales => GROUNDING_SALES,
        CallType::Meeting => GROUNDING_MEETING,
    }
}

/// The answer-style instruction (`PromptParts::style_suffix`).
pub fn style_suffix(style: AnswerStyle) -> &'static str {
    match style {
        AnswerStyle::Brief => STYLE_BRIEF,
        AnswerStyle::Balanced => STYLE_BALANCED,
        AnswerStyle::Detailed => STYLE_DETAILED,
    }
}

/// Stable string id for a style (matches the serde / `from_id_lossy` ids).
pub fn style_id(style: AnswerStyle) -> &'static str {
    match style {
        AnswerStyle::Brief => "brief",
        AnswerStyle::Balanced => "balanced",
        AnswerStyle::Detailed => "detailed",
    }
}

/// U+2060 WORD JOINER, inserted between adjacent `"` inside runs of three or
/// more quotes. See [`neutralize_triple_quotes`].
pub const QUOTE_BREAKER: char = '\u{2060}';

/// Make it impossible for the transcript to close the `"""` quoted section
/// early.
///
/// Rule: every maximal run of **three or more** consecutive `"` gets a
/// U+2060 WORD JOINER inserted between each adjacent pair of quotes, so
/// `"""` becomes `"⁠"⁠"`. Runs of one or two quotes are left alone (they cannot
/// form a delimiter on their own, and a maximal run cannot merge with a
/// neighbouring run). The delimiters in `user_message` sit on their own lines
/// (`"""\n` before, `\n"""` after), so a transcript that starts or ends with
/// `"`/`""` cannot merge with them across the newline either.
///
/// Why the word joiner instead of escaping or swapping to typographic quotes:
/// - it is invisible and zero-width, so the model reads the quote characters
///   the other person actually produced (faithful transcript);
/// - it is not a quote, a backslash or whitespace, so no `"""` substring can
///   survive and there is no escape sequence that itself needs escaping;
/// - it only fires on the pathological input, so a transcript without `"""`
///   is passed through byte-for-byte and `user_message` is exactly spec §8.
///
/// Returns the input unchanged (borrowed) when it contains no `"""`.
pub fn neutralize_triple_quotes(transcript: &str) -> std::borrow::Cow<'_, str> {
    if !transcript.contains("\"\"\"") {
        return std::borrow::Cow::Borrowed(transcript);
    }
    let mut out = String::with_capacity(transcript.len() + 16);
    let bytes = transcript.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            let run = i - start;
            if run >= 3 {
                for k in 0..run {
                    if k > 0 {
                        out.push(QUOTE_BREAKER);
                    }
                    out.push('"');
                }
            } else {
                out.push_str(&transcript[start..i]);
            }
        } else {
            // Copy up to the next quote in one go; `"` is ASCII so the slice
            // boundaries are always char boundaries.
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            out.push_str(&transcript[start..i]);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Build `user_message` for a transcript (spec §8 format, with
/// [`neutralize_triple_quotes`] applied to the transcript).
pub fn user_message(transcript: &str) -> String {
    let t = neutralize_triple_quotes(transcript);
    let mut s = String::with_capacity(t.len() + 80);
    s.push_str("The other person on the call just said:\n\"\"\"\n");
    s.push_str(&t);
    s.push_str("\n\"\"\"\n\nWhat should I say?");
    s
}

/// Build `cached_prefix` (role + non-empty sections + grounding). Independent
/// of the answer style by construction.
pub fn cached_prefix(
    call_type: CallType,
    resume: &str,
    job_description: &str,
    focus: &str,
    notes: &str,
) -> String {
    let resume = resume.trim();
    let jd = job_description.trim();
    let focus = focus.trim();
    let notes = notes.trim();
    let info = call_type_info(call_type);
    let role = role(call_type);

    let mut s = String::with_capacity(
        role.len() + resume.len() + jd.len() + focus.len() + notes.len() + 512,
    );
    s.push_str(role);
    if !resume.is_empty() {
        s.push_str(RESUME_HEADER);
        s.push_str(resume);
    }
    if !jd.is_empty() {
        s.push_str("\n\n--- ");
        s.push_str(info.jd_header);
        s.push_str(" ---\n");
        s.push_str(jd);
    }
    if !focus.is_empty() {
        s.push_str(FOCUS_HEADER);
        s.push_str(focus);
    }
    if !notes.is_empty() {
        s.push_str(NOTES_HEADER);
        s.push_str(notes);
    }
    if !(resume.is_empty() && jd.is_empty() && focus.is_empty() && notes.is_empty()) {
        s.push_str(grounding(call_type));
    }
    s
}

/// Build the three prompt parts. Pure and deterministic.
pub fn build_prompt(input: &PromptInput<'_>, transcript: &str) -> PromptParts {
    PromptParts {
        cached_prefix: cached_prefix(
            input.call_type,
            input.resume,
            input.job_description,
            input.focus,
            input.notes,
        ),
        style_suffix: style_suffix(input.style).to_owned(),
        user_message: user_message(transcript),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutralize_leaves_short_runs_alone() {
        for s in [
            "",
            "a",
            "\"",
            "\"\"",
            "say \"hi\"",
            "\"\"x\"\"",
            "a\"\"\nb\"\"",
        ] {
            assert!(matches!(
                neutralize_triple_quotes(s),
                std::borrow::Cow::Borrowed(_)
            ));
            assert_eq!(neutralize_triple_quotes(s), s);
        }
    }

    #[test]
    fn neutralize_breaks_every_long_run() {
        let wj = QUOTE_BREAKER;
        assert_eq!(
            neutralize_triple_quotes("\"\"\""),
            format!("\"{wj}\"{wj}\"")
        );
        assert_eq!(
            neutralize_triple_quotes("a\"\"\"\"b"),
            format!("a\"{wj}\"{wj}\"{wj}\"b")
        );
        // Multi-byte text around the run survives intact.
        assert_eq!(
            neutralize_triple_quotes("é\"\"\"—"),
            format!("é\"{wj}\"{wj}\"—")
        );
    }

    #[test]
    fn prompt_input_debug_is_redacted() {
        let input = PromptInput {
            call_type: CallType::Sales,
            style: AnswerStyle::Brief,
            resume: "SECRET-RESUME-TEXT",
            job_description: "SECRET-JD",
            focus: "SECRET-FOCUS",
            notes: "SECRET-NOTES",
        };
        let dbg = format!("{input:?}");
        assert!(!dbg.contains("SECRET"), "{dbg}");
        assert!(dbg.contains("resume_len: 18"), "{dbg}");
    }
}
