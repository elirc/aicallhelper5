//! Incremental Server-Sent Events parser over raw bytes.
//!
//! Network chunks can split anywhere: mid-line, mid-JSON, mid-UTF-8 character,
//! or between the `\r` and `\n` of a CRLF. The parser therefore buffers BYTES
//! and only decodes a line once its terminator has arrived, so the events it
//! yields are identical however the transport happened to chunk the stream.
//!
//! Supported (per the WHATWG EventSource grammar): `\n`, `\r\n` and bare `\r`
//! line endings; `data:` with or without the single leading space; multi-line
//! `data` joined with `"\n"`; the `event:` field; `:` comment lines; a leading
//! UTF-8 BOM. `id:`/`retry:` and unknown fields are ignored. [`SseParser::finish`]
//! flushes a final unterminated line and dispatches a pending event at EOF.

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, if the event had one.
    pub event: Option<String>,
    /// All `data:` lines of the event joined with `"\n"`.
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the current, not yet terminated line.
    line: Vec<u8>,
    /// The previous chunk ended in `\r`; a leading `\n` in the next chunk is
    /// the second half of that CRLF and must be skipped.
    skip_lf: bool,
    /// Still at the very start of the stream (BOM check pending).
    at_start: bool,
    event: Option<String>,
    data: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            at_start: true,
            ..Self::default()
        }
    }

    /// Feed the next network chunk; returns every event completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for &b in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\n' => self.end_line(&mut out),
                b'\r' => {
                    self.end_line(&mut out);
                    self.skip_lf = true;
                }
                _ => self.line.push(b),
            }
        }
        out
    }

    /// End of stream: process a final unterminated line and dispatch any event
    /// that was still being accumulated.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut out);
        }
        self.dispatch(&mut out);
        self.skip_lf = false;
        out
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) {
        let mut bytes = std::mem::take(&mut self.line);
        if self.at_start {
            self.at_start = false;
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                bytes.drain(..3);
            }
        }
        if bytes.is_empty() {
            self.dispatch(out);
            return;
        }
        let line = String::from_utf8_lossy(&bytes);
        if line.starts_with(':') {
            return; // comment
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (&line[..], ""),
        };
        match field {
            "data" => match &mut self.data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            },
            "event" => self.event = Some(value.to_owned()),
            _ => {} // id, retry, unknown: ignored
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.event.take();
        if let Some(data) = self.data.take() {
            out.push(SseEvent { event, data });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_whole(input: &[u8]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut v = p.feed(input);
        v.extend(p.finish());
        v
    }

    fn parse_split(input: &[u8], cuts: &[usize]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut v = Vec::new();
        let mut last = 0;
        for &c in cuts {
            v.extend(p.feed(&input[last..c]));
            last = c;
        }
        v.extend(p.feed(&input[last..]));
        v.extend(p.finish());
        v
    }

    fn ev(event: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            event: event.map(str::to_owned),
            data: data.to_owned(),
        }
    }

    /// Real-looking transcripts covering every grammar feature.
    fn transcripts() -> Vec<Vec<u8>> {
        let anthropic = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-haiku-4-5\"}}\n\n",
            "event: ping\ndata: {\"type\": \"ping\"}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Héllo — naïve café ✓ 日本語 🚀\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let groq_crlf = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Ünïcödé 😀\"},\"finish_reason\":null}]}\r\n\r\n",
            ": keep-alive comment\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        );
        let cr_only = "event: a\rdata:no-space\rdata: second line\r\rdata: é🚀\r\r";
        let unterminated = "data: first\n\ndata: {\"tail\":\"ü🚀\"}";
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"data: bom\n\nid: 7\nretry: 100\nfoo\ndata\n\n");
        vec![
            anthropic.as_bytes().to_vec(),
            groq_crlf.as_bytes().to_vec(),
            cr_only.as_bytes().to_vec(),
            unterminated.as_bytes().to_vec(),
            bom,
        ]
    }

    #[test]
    fn parses_fields_multiline_comments_and_endings() {
        let t = transcripts();
        let a = parse_whole(&t[0]);
        assert_eq!(a.len(), 5);
        assert_eq!(a[0].event.as_deref(), Some("message_start"));
        assert!(a[2].data.contains("Héllo — naïve café ✓ 日本語 🚀"));
        let g = parse_whole(&t[1]);
        assert_eq!(g.len(), 3, "comment-only block must not dispatch");
        assert_eq!(g[2], ev(None, "[DONE]"));
        let c = parse_whole(&t[2]);
        assert_eq!(
            c,
            vec![ev(Some("a"), "no-space\nsecond line"), ev(None, "é🚀")]
        );
        let u = parse_whole(&t[3]);
        assert_eq!(u, vec![ev(None, "first"), ev(None, "{\"tail\":\"ü🚀\"}")]);
        let b = parse_whole(&t[4]);
        assert_eq!(b, vec![ev(None, "bom"), ev(None, "")]);
    }

    #[test]
    fn event_without_data_is_not_dispatched_and_event_name_resets() {
        let out = parse_whole(b"event: lonely\n\ndata: x\n\n");
        assert_eq!(out, vec![ev(None, "x")]);
    }

    #[test]
    fn crlf_split_between_cr_and_lf_is_one_line_end() {
        let out = parse_split(b"data: a\r\n\r\ndata: b\r\n\r\n", &[8]);
        assert_eq!(out, vec![ev(None, "a"), ev(None, "b")]);
    }

    #[test]
    fn every_single_split_point_yields_identical_events() {
        for t in transcripts() {
            let whole = parse_whole(&t);
            for cut in 0..=t.len() {
                assert_eq!(parse_split(&t, &[cut]), whole, "cut at {cut}");
            }
        }
    }

    #[test]
    fn every_pair_of_split_points_yields_identical_events() {
        // O(n^2) parses: exhaustive for the shorter transcripts; the long
        // Anthropic one is covered by single cuts + random multi-splits.
        for t in transcripts().into_iter().filter(|t| t.len() <= 256) {
            let whole = parse_whole(&t);
            for a in 0..=t.len() {
                for b in a..=t.len() {
                    assert_eq!(parse_split(&t, &[a, b]), whole, "cuts {a},{b}");
                }
            }
        }
    }

    #[test]
    fn random_multi_splits_and_byte_by_byte_yield_identical_events() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for t in transcripts() {
            let whole = parse_whole(&t);
            let every: Vec<usize> = (1..t.len()).collect();
            assert_eq!(parse_split(&t, &every), whole, "byte by byte");
            for _ in 0..500 {
                let n = (next() % 12) as usize;
                let mut cuts: Vec<usize> = (0..n)
                    .map(|_| (next() % (t.len() as u64 + 1)) as usize)
                    .collect();
                cuts.sort_unstable();
                assert_eq!(parse_split(&t, &cuts), whole, "cuts {cuts:?}");
            }
        }
    }

    #[test]
    fn invalid_utf8_is_replaced_not_panicking() {
        let out = parse_whole(b"data: a\xFFb\n\n");
        assert_eq!(out, vec![ev(None, "a\u{FFFD}b")]);
    }
}
