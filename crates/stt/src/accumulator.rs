//! Transcript accumulator: finalized segments + the current interim segment
//! -> the FULL transcript text the UI shows (spec §4.5, §6).
//!
//! Pure and synchronous so every rule is unit-testable without a socket.

/// Accumulates Deepgram `Results` segments into one transcript.
///
/// * A final segment is trimmed and appended to the committed text (joined by
///   a single space, empty finals skipped) and clears the interim.
/// * An interim segment REPLACES the previous interim (Deepgram re-sends the
///   whole in-progress segment each time).
/// * The full text is `finals + " " + interim` (either part may be empty).
#[derive(Debug, Default, Clone)]
pub struct TranscriptAccumulator {
    finals: String,
    interim: String,
    last_emitted: Option<(String, bool)>,
}

/// What the accumulator wants the reader to emit after a segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptUpdate {
    /// The full transcript so far.
    pub text: String,
    /// True when the full text contains no interim part (everything shown is
    /// finalized).
    pub is_final: bool,
}

impl TranscriptAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one segment. Returns an update only when `(text, is_final)`
    /// differs from the last update returned, so identical re-sends (and empty
    /// interims after a final) don't spam the page.
    pub fn apply(&mut self, transcript: &str, is_final: bool) -> Option<TranscriptUpdate> {
        let segment = transcript.trim();
        if is_final {
            if !segment.is_empty() {
                if !self.finals.is_empty() {
                    self.finals.push(' ');
                }
                self.finals.push_str(segment);
            }
            self.interim.clear();
        } else {
            self.interim.clear();
            self.interim.push_str(segment);
        }

        let current = (self.full_text(), self.interim.is_empty());
        if self.last_emitted.as_ref() == Some(&current) {
            return None;
        }
        self.last_emitted = Some(current.clone());
        Some(TranscriptUpdate {
            text: current.0,
            is_final: current.1,
        })
    }

    /// Committed (final) text only.
    pub fn final_text(&self) -> &str {
        &self.finals
    }

    /// Current interim segment (trimmed; empty when none).
    pub fn interim_text(&self) -> &str {
        &self.interim
    }

    /// Finals + trailing interim, single-space joined. This is also what
    /// `SttEvent::Flushed` carries: Deepgram finalizes everything on
    /// CloseStream, but if an interim was never replaced by a final we keep it
    /// rather than silently dropping words the user already saw.
    pub fn full_text(&self) -> String {
        match (self.finals.is_empty(), self.interim.is_empty()) {
            (_, true) => self.finals.clone(),
            (true, false) => self.interim.clone(),
            (false, false) => format!("{} {}", self.finals, self.interim),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(text: &str, is_final: bool) -> Option<TranscriptUpdate> {
        Some(TranscriptUpdate {
            text: text.to_string(),
            is_final,
        })
    }

    #[test]
    fn interim_is_replaced_by_next_interim() {
        let mut a = TranscriptAccumulator::new();
        assert_eq!(a.apply("hel", false), upd("hel", false));
        assert_eq!(a.apply("hello wor", false), upd("hello wor", false));
        assert_eq!(a.full_text(), "hello wor");
        assert_eq!(a.final_text(), "");
    }

    #[test]
    fn final_commits_and_clears_interim() {
        let mut a = TranscriptAccumulator::new();
        a.apply("hello wor", false);
        assert_eq!(a.apply("hello world", true), upd("hello world", true));
        assert_eq!(a.interim_text(), "");
        assert_eq!(a.apply("how", false), upd("hello world how", false));
        assert_eq!(
            a.apply("how are you", true),
            upd("hello world how are you", true)
        );
        assert_eq!(a.final_text(), "hello world how are you");
    }

    #[test]
    fn empty_finals_are_skipped_but_clear_interim() {
        let mut a = TranscriptAccumulator::new();
        a.apply("one", true);
        a.apply("noise", false);
        assert_eq!(a.apply("", true), upd("one", true));
        assert_eq!(a.apply("   ", true), None, "nothing changed");
        assert_eq!(a.apply("two", true), upd("one two", true));
        assert_eq!(a.full_text(), "one two");
    }

    #[test]
    fn empty_final_first_emits_nothing_new_after_it() {
        let mut a = TranscriptAccumulator::new();
        assert_eq!(a.apply("", true), upd("", true));
        assert_eq!(a.apply("", true), None);
        assert_eq!(a.apply("", false), None, "empty interim == nothing new");
    }

    #[test]
    fn whitespace_is_trimmed_and_joined_by_single_space() {
        let mut a = TranscriptAccumulator::new();
        a.apply("  hello \n", true);
        a.apply("\tworld  ", true);
        assert_eq!(a.full_text(), "hello world");
        a.apply("   next  ", false);
        assert_eq!(a.full_text(), "hello world next");
    }

    #[test]
    fn identical_interim_resend_is_not_re_emitted() {
        let mut a = TranscriptAccumulator::new();
        assert!(a.apply("same", false).is_some());
        assert_eq!(a.apply("same", false), None);
        assert_eq!(a.apply(" same ", false), None);
    }

    #[test]
    fn final_with_same_text_as_interim_flips_is_final() {
        let mut a = TranscriptAccumulator::new();
        a.apply("hello", false);
        assert_eq!(a.apply("hello", true), upd("hello", true));
    }

    #[test]
    fn empty_interim_after_final_emits_nothing() {
        let mut a = TranscriptAccumulator::new();
        a.apply("done", true);
        assert_eq!(a.apply("", false), None);
    }

    #[test]
    fn full_text_keeps_trailing_interim_for_flush() {
        let mut a = TranscriptAccumulator::new();
        a.apply("committed", true);
        a.apply("tail words", false);
        assert_eq!(a.full_text(), "committed tail words");
        let mut b = TranscriptAccumulator::new();
        b.apply("only interim", false);
        assert_eq!(b.full_text(), "only interim");
    }
}
