# Prompt crate tests (`callcore-prompt`, spec §8)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-prompt` (41 tests, no network, no I/O except reading fixtures/snapshots/SPEC.md).

## Protection layers

1. **Spec conformance** (`tests/spec_conformance.rs`) parses `docs/SPEC.md` §8 at test time and checks every role, grounding, label, jd header, style suffix and the PREAMBLE are byte-identical to the spec. This catches typos in the long prose strings.
2. **Hand-written literals** (`tests/prompt.rs`, `literal_*`): full expected `PromptParts` typed out in the test, not taken from crate constants or snapshots, so a wrong snapshot can't pass itself.
3. **Byte-exact snapshots** (`tests/snapshots.rs`): 6 call types × 3 styles × {empty, full, mixed} = 54 files in `crates/prompt/tests/snapshots/`. To regenerate, review the diff first, then run `UPDATE_SNAPSHOTS=1 cargo test -p callcore-prompt --test snapshots`. `tests/snapshots/.gitattributes` (`* -text`) stops git `autocrlf` from rewriting line endings.

## The `"""` fix

In the transcript, every maximal run of 3 or more `"` gets U+2060 WORD JOINER inserted between adjacent quotes (`neutralize_triple_quotes`). A transcript with no `"""` passes through unchanged, so `user_message` is exactly the spec format. The fix is lossless: removing U+2060 gives back the original. The delimiters sit on their own lines, so a transcript that starts or ends with `"`/`""` can't merge with them.

## Offline eval harness

- `cargo run -p callcore-prompt --example prompt_eval` builds all 27 fixtures × 3 styles (81 prompts). It prints a size table (chars, ~tokens = chars/4) and writes each prompt to `crates/prompt/eval/out/` (gitignored).
- `crates/prompt/eval/fixtures.json` has 27 fixtures (4 or more per call type) and 5 sample profiles. Each fixture has `expect` rules: `mustNotContain`, `mustContainAny`, `maxWords` per style, `minWords`, `firstPerson`, `noWrappingQuotes`.
- `callcore_prompt::eval::check_answer(fixture, style, answer) -> Vec<String>` scores a real answer. It is meant for the live-run tool in the llm area.

## Tests

### Unit (`src/lib.rs`)
- `neutralize_leaves_short_runs_alone` — runs of 1–2 quotes are returned borrowed and unchanged — the common case must stay byte-exact and allocation-free.
- `neutralize_breaks_every_long_run` — `"""`/`""""` get a joiner between every quote pair, and multi-byte neighbours survive — core of the delimiter fix, including UTF-8 slicing safety.
- `prompt_input_debug_is_redacted` — `PromptInput`'s Debug shows lengths, never profile text — profile text must stay out of logs/panics (AGENTS.md).

### Unit (`src/eval.rs`)
- `check_answer_passes_good_answer` — a compliant answer gives no violations — baseline so the rules aren't always failing.
- `check_answer_flags_empty` — an empty/whitespace answer is always a violation, even with no rules — mirrors the app's `empty_answer` rule.
- `check_answer_must_not_contain_is_case_insensitive` — "as an ai" matches "As an AI", and a leaked `"""` is caught — the forbidden phrases must catch case variants.
- `check_answer_max_words_is_per_style` — word limits apply per style — brief/balanced/detailed have different budgets.
- `check_answer_min_words` — too-short answers are flagged — catches truncated or degenerate replies.
- `check_answer_first_person` — "the user" and missing first-person words are flagged; curly `I’ve` counts; "I" inside a word doesn't — the answer must be spoken as the user.
- `check_answer_wrapping_quotes` — straight or curly wrapping quotes are flagged, interior quotes are fine — the preamble forbids quotation marks around the answer.
- `check_answer_must_contain_any` — at least one keyword must appear, case-insensitively — lets fixtures require grounding facts (e.g. SOC 2).
- `check_answer_violations_do_not_echo_answer` — violation text never quotes the answer — violations are safe to log.
- `bundled_fixtures_parse_and_cover_every_call_type` — `fixtures.json` parses and validates, has ≥4 fixtures per call type (≥24 total), and every fixture forbids `"""` and "As an AI" and sets max_words for all styles — keeps the eval suite complete.
- `suite_validation_rejects_bad_files` — duplicate ids, unknown profiles, non-filename-safe ids and unknown JSON fields are rejected — ids become output filenames, and typos in rule names must not be silently ignored.
- `estimate_tokens_rounds_up` — chars/4 is rounded up and counts chars, not bytes — cost budgeting must not undercount.
- `profile_debug_is_redacted` — eval `Profile` Debug hides the text — same logging rule for sample profiles.

### Integration (`tests/prompt.rs`)
- `literal_behavioral_balanced_all_fields` — full literal PromptParts for behavioral with all four fields, edge whitespace trimmed — independent oracle, not a snapshot.
- `literal_sales_brief_only_notes` — full literal for sales with only notes (other fields whitespace-only) — independent oracle for PREAMBLE + role, and for grounding firing on one field.
- `literal_technical_detailed_nothing` — full literal for technical with no fields and an empty transcript — no sections and no grounding; empty transcript format.
- `literal_meeting_jd_header_and_grounding` — meeting and sales use "ABOUT THIS MEETING" / "ABOUT THIS CALL" jd headers — per-call-type jd headers.
- `call_type_info_labels_and_jd_headers` — id, label and jd header for all 6 types; `CALL_TYPE_LABELS` matches the info and the `CallType::ALL` order — the UI pickers depend on these.
- `non_behavioral_roles_start_with_preamble` — the 5 non-behavioral roles are PREAMBLE + " This …"; behavioral isn't — spec composition rule.
- `prompt_strings_use_ascii_apostrophes_and_real_em_dashes` — no curly quotes, en-dashes, `--`, CR or double spaces in any prose; em-dash is U+2014 — common copy-paste corruption.
- `no_fields_means_role_only_no_grounding` — with no fields, the prefix is exactly the role — grounding only when a field is present.
- `whitespace_only_field_counts_as_empty` — whitespace-only fields (including U+3000 and CRLF) add no section and no grounding — spec edge trimming.
- `each_single_field_alone_triggers_grounding` — each of the 4 fields alone gives role + its section + grounding, for all types — grounding when ANY field is non-empty; exact header bytes.
- `sections_appear_in_spec_order` — resume < jd < focus < notes, then grounding last — fixed order keeps output byte-stable.
- `edge_trimming_keeps_interior_whitespace` — interior blank lines, indentation and tabs are kept; only the edges are trimmed — spec: trim the edges only.
- `determinism_same_input_twice_identical` — same input gives identical bytes — byte-stable prompts for caching and retries.
- `style_flip_leaves_cached_prefix_identical` — for every type, all 3 styles give the same `cached_prefix`, and the suffix follows the style — style flips must not invalidate the Anthropic prompt cache.
- `unknown_ids_fall_back_via_contract_helpers` — unknown call type falls back to behavioral, unknown style to balanced; ids round-trip — spec fallback rules.
- `transcript_without_triple_quotes_is_byte_exact` — for many quote-bearing transcripts without `"""`, `user_message` is exactly the spec format — spec requirement for the common case.
- `triple_quotes_in_transcript_cannot_close_the_section` — adversarial transcripts (`"""`, `""""`, 7 quotes, a fake closing delimiter plus injected instruction, leading/trailing runs, unicode) leave exactly the 2 real delimiters at fixed offsets; the transcript is recoverable by removing U+2060 — the §8 improvement: no breaking out of the quoted section.
- `transcript_edge_quotes_cannot_merge_with_delimiters_across_newline` — transcripts starting or ending in `"`/`""` are untouched and create no extra `"""` window — proves the newline stops delimiter merging.
- `build_prompt_user_message_uses_neutralized_transcript` — `build_prompt` applies the fix — the hot path actually uses it.

### Snapshots (`tests/snapshots.rs`)
- `snapshots_every_call_type_style_and_field_case` — the 54 cases match their snapshot files byte for byte; no stale or missing files; CR hint on mismatch — full-matrix regression guard for prompt bytes.
- `snapshot_attributes_file_keeps_bytes_exact` — `.gitattributes` has `* -text` — without it, Windows `autocrlf` checkouts would break every snapshot.

### Spec conformance (`tests/spec_conformance.rs`)
- `preamble_matches_spec` — PREAMBLE equals the spec line — guards against transcription typos.
- `call_types_match_spec` — for all 6 types, label, jd header, role (resolving `PREAMBLE + `) and grounding equal the spec, and the (DEFAULT) type is `CallType::default()` — guards the long prose.
- `style_suffixes_match_spec` — the 3 suffixes equal the spec, and balanced is the default — guards the style prose.
- `section_headers_and_user_message_match_spec` — section headers and the user_message template equal the spec formulas — guards the structural bytes.
