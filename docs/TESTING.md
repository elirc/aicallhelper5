# Testing

This file collects every automated test in the repository, grouped by area. There is one bullet per test, saying what it verifies and why it exists (spec §16). The per-area source files it is merged from live in `docs/testing/*.md`. Agents add a bullet there with each new test. When a per-area file changes, update the matching section here as well. The contract, shell, Tauri-glue and benchmark sections exist only in this file.

## Test rules

- **No network, no live provider, no audio device.** Every external dependency sits behind a port (`crates/contract/src/ports.rs`) and is replaced by a fake. The only sockets are **loopback servers the test starts itself** on `127.0.0.1:0`:
  - a scripted HTTP/1.1 server replaying SSE bytes with hostile chunking (`crates/llm/tests/support`);
  - a tokio-tungstenite server replaying Deepgram frames (`crates/stt/tests/support`).
- **Timing tests wait for conditions and never sleep a fixed time.** Timer logic runs under a **paused tokio clock** (`#[tokio::test(start_paused = true)]` plus `advance`), so 120 s caps and 60 s watchdogs are exact and instant. Paused time and real loopback sockets race tokio's auto-advance on Windows, so socket tests use **real time** with generous bounds or explicit synchronization. They never mix the two.
- **Secrets and profile text stay out of `Debug`, logs, errors and panics.** Several suites assert this directly.
- **Nothing is weakened to pass.** A test that looks flaky is run at least twice and then fixed, never loosened.
- The real DPAPI is exercised only under `cfg(windows)`. No `cargo test` opens a real audio device. Live WASAPI capture is exercised only by the manual probes (`crates/audio/examples/`), and the backend's pure pieces are unit-tested.

## How to run

| Suite | Command |
|---|---|
| Everything (CI) | `cargo test --workspace` and `npx vitest run` |
| One Rust crate | `export CARGO_TARGET_DIR=target-core` (Git Bash), then `cargo test -p callcore-<crate>` (`contract`, `prompt`, `llm`, `stt`, `audio`, `settings`, `session`, `shell`) |
| Tauri glue + wiring integration test | `cargo test -p aicallassistant` (uses the default `target/`; needs `dist/` from `npm run build` for `tauri-build`) |
| Contract + TS bindings | `cargo test -p callcore-contract` (also rewrites `src/generated/*.ts`; CI then runs `git diff --exit-code -- src/generated`) |
| Frontend state layer | `npx vitest run src/ipc src/state src/markdown src/app` |
| Frontend components | `npx vitest run src/components` |
| Prompt snapshots (regenerate after reviewing the diff) | `UPDATE_SNAPSHOTS=1 cargo test -p callcore-prompt --test snapshots` |
| Lint / types | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `npx tsc --noEmit`, `npx eslint src` |
| Latency benchmark (not a test; real clock) | `cargo run -p callcore-session --example latency_bench --release` (see [below](#latency-benchmark)) |
| Prompt eval (offline) | `cargo run -p callcore-prompt --example prompt_eval` |
| Manual audio QA (real device) | `cargo run -p callcore-audio --example capture_probe` / `--example raw_probe` |

## Contents

- Prompt crate tests (`callcore-prompt`, spec §8)  (source: `docs/testing/prompt.md`)
- callcore-llm tests  (source: `docs/testing/llm.md`)
- callcore-stt tests (Deepgram streaming client)  (source: `docs/testing/stt.md`)
- Audio (`callcore-audio`) tests  (source: `docs/testing/audio.md`)
- Settings store tests (`callcore-settings`)  (source: `docs/testing/settings.md`)
- Session actor tests (`crates/session`)  (source: `docs/testing/session.md`)
- Contract (`callcore-contract`)
- Shell logic (`callcore-shell`)
- Tauri glue (`src-tauri`)
- Frontend state layer — tests  (source: `docs/testing/frontend-state.md`)
- Frontend UI tests (`src/components/**`)  (source: `docs/testing/frontend-ui.md`)
- Latency benchmark

## Prompt crate tests (`callcore-prompt`, spec §8)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-prompt` (41 tests, no network, no I/O except reading fixtures/snapshots/SPEC.md).

### Protection layers

1. **Spec conformance** (`tests/spec_conformance.rs`) parses `docs/SPEC.md` §8 at test time and checks every role, grounding, label, jd header, style suffix and the PREAMBLE are byte-identical to the spec. This catches typos in the long prose strings.
2. **Hand-written literals** (`tests/prompt.rs`, `literal_*`): full expected `PromptParts` typed out in the test, not taken from crate constants or snapshots, so a wrong snapshot can't pass itself.
3. **Byte-exact snapshots** (`tests/snapshots.rs`): 6 call types × 3 styles × {empty, full, mixed} = 54 files in `crates/prompt/tests/snapshots/`. To regenerate, review the diff first, then run `UPDATE_SNAPSHOTS=1 cargo test -p callcore-prompt --test snapshots`. `tests/snapshots/.gitattributes` (`* -text`) stops git `autocrlf` from rewriting line endings.

### The `"""` fix

In the transcript, every maximal run of 3 or more `"` gets U+2060 WORD JOINER inserted between adjacent quotes (`neutralize_triple_quotes`). A transcript with no `"""` passes through unchanged, so `user_message` is exactly the spec format. The fix is lossless: removing U+2060 gives back the original. The delimiters sit on their own lines, so a transcript that starts or ends with `"`/`""` can't merge with them.

### Offline eval harness

- `cargo run -p callcore-prompt --example prompt_eval` builds all 27 fixtures × 3 styles (81 prompts). It prints a size table (chars, ~tokens = chars/4) and writes each prompt to `crates/prompt/eval/out/` (gitignored).
- `crates/prompt/eval/fixtures.json` has 27 fixtures (4 or more per call type) and 5 sample profiles. Each fixture has `expect` rules: `mustNotContain`, `mustContainAny`, `maxWords` per style, `minWords`, `firstPerson`, `noWrappingQuotes`.
- `callcore_prompt::eval::check_answer(fixture, style, answer) -> Vec<String>` scores a real answer. It is meant for the live-run tool in the llm area.

### Tests

#### Unit (`src/lib.rs`)
- `neutralize_leaves_short_runs_alone` — runs of 1–2 quotes are returned borrowed and unchanged — the common case must stay byte-exact and allocation-free.
- `neutralize_breaks_every_long_run` — `"""`/`""""` get a joiner between every quote pair, and multi-byte neighbours survive — core of the delimiter fix, including UTF-8 slicing safety.
- `prompt_input_debug_is_redacted` — `PromptInput`'s Debug shows lengths, never profile text — profile text must stay out of logs/panics (AGENTS.md).

#### Unit (`src/eval.rs`)
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

#### Integration (`tests/prompt.rs`)
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

#### Snapshots (`tests/snapshots.rs`)
- `snapshots_every_call_type_style_and_field_case` — the 54 cases match their snapshot files byte for byte; no stale or missing files; CR hint on mismatch — full-matrix regression guard for prompt bytes.
- `snapshot_attributes_file_keeps_bytes_exact` — `.gitattributes` has `* -text` — without it, Windows `autocrlf` checkouts would break every snapshot.

#### Spec conformance (`tests/spec_conformance.rs`)
- `preamble_matches_spec` — PREAMBLE equals the spec line — guards against transcription typos.
- `call_types_match_spec` — for all 6 types, label, jd header, role (resolving `PREAMBLE + `) and grounding equal the spec, and the (DEFAULT) type is `CallType::default()` — guards the long prose.
- `style_suffixes_match_spec` — the 3 suffixes equal the spec, and balanced is the default — guards the style prose.
- `section_headers_and_user_message_match_spec` — section headers and the user_message template equal the spec formulas — guards the structural bytes.

## callcore-llm tests

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-llm`.
No test touches the network: provider tests run against a scripted HTTP/1.1
loopback server (`crates/llm/tests/support/mod.rs`) on 127.0.0.1 that records
each request and replays responses with controllable framing (chunked,
Content-Length, close-delimited), split writes (hostile 1–7 byte cuts that land
mid-line, mid-JSON and mid-UTF-8), and truncated bodies. Real time throughout
(no paused clock with sockets); every wait is on a condition with a generous bound.

### SSE parser (`src/sse.rs`)
- `parses_fields_multiline_comments_and_endings` — `event:`, `data:` with/without space, multi-line data joined by `\n`, comments, `\n`/`\r\n`/`\r`, BOM, id/retry/unknown fields, unterminated final line — the parser must cover the whole EventSource grammar providers actually emit.
- `event_without_data_is_not_dispatched_and_event_name_resets` — an `event:` with no data dispatches nothing and does not leak its name into the next event — per the SSE spec; avoids mislabelled events.
- `crlf_split_between_cr_and_lf_is_one_line_end` — a chunk ending in `\r` followed by one starting with `\n` is ONE line end — otherwise a phantom blank line would dispatch events early.
- `every_single_split_point_yields_identical_events` — for 5 real-looking transcripts, splitting at every byte position gives the same events as parsing whole — the core guarantee: transport chunking must never change what the user sees (spec §7/§16).
- `every_pair_of_split_points_yields_identical_events` — exhaustive two-cut splits for the shorter transcripts — catches state bugs that need two boundaries (e.g. a CRLF and a UTF-8 char both split).
- `random_multi_splits_and_byte_by_byte_yield_identical_events` — byte-by-byte feeding plus 500 random multi-splits per transcript (deterministic xorshift PRNG) — property test for arbitrary chunking.
- `invalid_utf8_is_replaced_not_panicking` — invalid bytes become U+FFFD — a hostile/broken server must not crash the stream.

### Failure copy + mapping (`src/failure.rs`, `src/http.rs`)
- `app_error_mapping_matches_spec_section_10` — every `ProviderFailureKind` maps to the §10 code (Auth→llm_auth, RateLimit→llm_rate_limit, Aborted→aborted, rest→llm_http) and keeps its message — the UI keys behaviour off the code.
- `snippet_prefers_json_error_message` — `error.message`, then `message`, then raw text; whitespace collapsed — users see the provider's actual reason, not a JSON blob.
- `snippet_redacts_key_strips_control_chars_and_truncates` — key replaced by `***`, control chars removed, ≤200 chars, key at the truncation boundary still redacted — spec §14.12 (keys never in errors) and the 200-char snippet rule.
- `secrets_of_extracts_bearer_token_and_raw_value` — redaction covers both `Bearer <key>` and the bare key — servers echo either form.
- `map_status_kinds_and_copy` — 403→Auth, 529→RateLimit with snippet, 404→ModelUnavailable, 502→Http with exact copy — shared status mapping is user copy with the status quoted.

### Pre-warm (`src/http.rs`)
- `throttle_allows_one_fire_per_two_seconds` — at most one pre-warm per 2 s (synthetic `Instant`s, no sleeping) — Record/Ask/Stop all pre-warm; the throttle stops request storms.
- `prewarm_without_runtime_is_a_noop` — calling `prewarm` outside a tokio runtime neither panics nor consumes the throttle — the shell may call it from a non-runtime thread.

### Registry (`src/lib.rs`)
- `registry_lists_both_providers_with_anthropic_default` — `infos()` exact `{id, displayName, keyId, model}` for both providers, default = anthropic, `get` by id, unknown id → None — the settings view and session lookup depend on these exact values.

### Anthropic conformance (`tests/anthropic.rs`)
- `anthropic_success_hostile_chunking_concatenates_blocks` — 1–7 byte chunked writes incl. multi-byte chars; deltas in order; two content blocks joined with nothing between — spec §7 concatenation rule under hostile chunking.
- `anthropic_success_content_length_and_close_delimited_framing` — success over Content-Length and close-delimited bodies — framing must not matter.
- `anthropic_crlf_line_endings_parse` — CRLF transcript streams identically — proxies may rewrite line endings.
- `anthropic_message_stop_in_unterminated_final_line_is_flushed` — `message_stop` in a final line without a terminator still counts — the parser's EOF flush must reach the provider.
- `anthropic_error_event_before_text_is_provider_error` — in-stream `error` before any text → ProviderError quoting the message, no deltas.
- `anthropic_error_event_after_text_is_provider_error_with_deltas_sent` — error after text → ProviderError, earlier deltas already delivered — the session decides what to show; no retry after a delta.
- `anthropic_overloaded_and_rate_limit_events_map_to_rate_limit` — `overloaded_error`→RateLimit{529}, `rate_limit_error`→RateLimit{429} — same codes as the HTTP statuses (spec §7).
- `anthropic_empty_answer_is_empty_answer` — `message_stop` with no text → EmptyAnswer — never a blank "finished" entry.
- `anthropic_whitespace_answer_is_empty_answer` — whitespace-only text → EmptyAnswer.
- `anthropic_premature_eof_is_incomplete` — EOF without `message_stop` → Incomplete (deltas already sent) — success requires the real terminal signal.
- `anthropic_pings_only_is_incomplete` — a stream of only pings is not success.
- `anthropic_token_limit_is_truncated` — `max_tokens` and `model_context_window_exceeded` → finish=truncated.
- `anthropic_refusal_is_refused` — `refusal` → finish=refused.
- `anthropic_other_stop_reasons_are_complete` — end_turn/stop_sequence/pause_turn → complete.
- `anthropic_http_status_matrix` — 401/403→Auth ("Anthropic rejected the API key (401)"), 404→ModelUnavailable naming the model, 429/529→RateLimit, 500→Http with exact snippet copy; status quoted; no key — conformance matrix for HTTP errors.
- `anthropic_key_echoed_in_error_body_is_redacted` — a key echoed in an HTTP error body or an in-stream error is replaced by `***` — spec §14.12.
- `anthropic_long_error_body_snippet_is_capped` — 5 000-char error body → ≤200-char snippet.
- `anthropic_stream_drop_mid_body_is_stream_drop` — body cut short (chunked without final chunk, short Content-Length) → StreamDrop, deltas before it delivered.
- `anthropic_close_before_response_is_connect` — server reads the request then closes without a byte → Connect — the ONLY retryable kind, returned only when no response byte arrived.
- `anthropic_connection_refused_is_connect` — refused connection → Connect with user copy ("Could not connect to Anthropic…"), no key, no Debug dump.
- `anthropic_dropped_receiver_is_aborted` — delta receiver gone → Aborted — the session dropped the answer.
- `anthropic_request_bytes_identical_across_two_sends` — the same `PreparedRequest` sent twice produces byte-identical bodies on the wire, equal to `req.body`; rebuilding is byte-identical too — retry must resend exactly these bytes and the cache prefix must be stable.
- `anthropic_request_body_is_exactly_spec_section_7` — the wire body equals the §7 JSON exactly (two system blocks, cache_control on the prefix, one user message, stream, max_tokens 1024).
- `anthropic_headers_present_and_correct` — POST /v1/messages with x-api-key, anthropic-version 2023-06-01, content-type application/json, no Authorization.
- `anthropic_prepared_request_debug_never_shows_key` — `{:?}` of the built request shows `***`, never the key.
- `anthropic_prewarm_gets_models_without_key` — `prewarm()` fires GET /v1/models with no key header.

### Groq conformance (`tests/groq.rs`)
- `groq_success_hostile_chunking` — 1–7 byte chunked writes; deltas in order; answer complete.
- `groq_success_content_length_crlf` — Content-Length framing + CRLF line endings.
- `groq_done_alone_is_terminal` — `[DONE]` without a finish_reason completes — spec: completion = finish_reason OR [DONE].
- `groq_bytes_after_done_in_same_chunk_still_count` — content after `[DONE]` in the same network chunk is still delivered — `[DONE]` is a skip sentinel, not EOF.
- `groq_finish_reason_without_done_is_terminal` — finish_reason then EOF (no [DONE]) → complete.
- `groq_token_limit_is_truncated` — `length` → truncated.
- `groq_content_filter_is_refused` — `content_filter` → refused.
- `groq_error_payload_before_text_is_provider_error` — `{"error":…}` payload → ProviderError quoting the message.
- `groq_error_payload_after_text_is_provider_error_with_deltas_sent` — error after text → ProviderError, deltas delivered, echoed key redacted.
- `groq_empty_answer_is_empty_answer` — terminal with no text → EmptyAnswer.
- `groq_whitespace_answer_is_empty_answer` — whitespace-only → EmptyAnswer.
- `groq_premature_eof_flushes_unterminated_line_then_incomplete` — the unterminated last line's delta reaches the UI, then Incomplete — spec §7.
- `groq_http_status_matrix` — 401/403→Auth ("Groq rejected the API key (403)"), 404→ModelUnavailable with the exact model message, 429/529→RateLimit, 500→Http; no key.
- `groq_400_model_gone_is_model_unavailable` — 400 with model_decommissioned / model_not_found / "does not exist" → the exact "The model openai/gpt-oss-120b is no longer available — switch … to Claude in Settings or install the latest version." — never tell users to edit source.
- `groq_other_400_is_http_with_snippet` — an unrelated 400 → Http{400} with the provider snippet.
- `groq_key_echoed_in_error_body_is_redacted` — "Bearer <key>" echoed in an error body is redacted.
- `groq_stream_drop_mid_body_is_stream_drop` — truncated chunked / Content-Length body → StreamDrop, deltas delivered.
- `groq_close_before_response_is_connect` — close before any response byte → Connect.
- `groq_connection_refused_is_connect` — refused → Connect with user copy, no key.
- `groq_request_bytes_identical_across_two_sends` — a Connect failure followed by a resend of the same `PreparedRequest` puts identical body + auth header on the wire — the session's retry-once path.
- `groq_request_body_is_exactly_spec_section_7` — exact §7 body (one joined system string, max_completion_tokens 1024, temperature 0.7, reasoning_effort low, include_reasoning false, stream) and NO `reasoning_format`.
- `groq_headers_present_and_correct` — POST /openai/v1/chat/completions with `Authorization: Bearer <key>`, content-type, no x-api-key.
- `groq_prepared_request_debug_never_shows_key` — Debug shows `***`, never the key.
- `groq_prewarm_gets_models_without_key` — GET /v1/models with no Authorization.

## callcore-stt tests (Deepgram streaming client)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-stt`

Socket tests (`crates/stt/tests/deepgram.rs`) run against a loopback fake
Deepgram (`crates/stt/tests/support/mod.rs`, tokio-tungstenite
`accept_hdr_async` on `127.0.0.1:0`). They use real time; every wait is a
condition wrapped in a 5 s `tokio::time::timeout`, never a fixed sleep. The
one bounded absence check (nothing sent after CloseStream) waits 400 ms =
8 keepalive intervals and is an assertion, not synchronization.

### Behaviour notes (documented decisions)

- `Flushed.transcript` = finals + any trailing interim that no final replaced
  (Deepgram normally finalizes everything on CloseStream; if it doesn't, words
  the user already saw are kept rather than dropped).
- `Transcript.is_final` = "the full text has no interim part". An event is
  emitted only when `(text, is_final)` changes, so identical re-sends and empty
  interims after a final are not re-emitted.
- Text frames > 1 MiB are ignored unparsed; tungstenite's own cap is 4 MiB,
  and a frame above THAT is a protocol failure (`Failed{Server}`), never a panic.
- Close 1000 / no-status after `close_stream` → `Flushed`; before it →
  `Failed{Server, "Deepgram ended the stream unexpectedly"}`. Codes
  1008/4001/4003/4008 → `Failed{BadKey}`; others → `Failed{Server}` with the
  code and a sanitized ≤200-char reason. A TCP drop without a close frame is
  `Failed{Server}` even after `close_stream` (the flush may be incomplete).
- Exactly one terminal event; after it the reader stops emitting and the
  channel closes, so a late close after `Flushed` can never produce `Failed`.
- `abort()` aborts both tasks (socket dropped → server sees EOF), never errors;
  later sends return `Err(Closed)`. Dropping the sender before `close_stream`
  also aborts; after `close_stream` the reader stays alive to deliver `Flushed`
  and exits on server close or when the event receiver is dropped.

### Unit tests — accumulator (`src/accumulator.rs`)

- `interim_is_replaced_by_next_interim` — a new interim replaces the old one, finals untouched — Deepgram resends the whole in-progress segment; appending would duplicate words.
- `final_commits_and_clears_interim` — a final is appended to committed text and clears the interim; later interims append after it — core full-transcript rule (spec §4.5).
- `empty_finals_are_skipped_but_clear_interim` — empty/whitespace finals add nothing but still clear the interim, no double spaces — Deepgram sends empty finals for silence.
- `empty_final_first_emits_nothing_new_after_it` — repeated empty finals/interims at start emit once then nothing — avoids spamming the page with no-op events.
- `whitespace_is_trimmed_and_joined_by_single_space` — segments are trimmed and joined with exactly one space — transcript is fed byte-stable into the prompt.
- `identical_interim_resend_is_not_re_emitted` — same interim (even with padding) is not re-emitted — events only on change.
- `final_with_same_text_as_interim_flips_is_final` — interim "hello" then final "hello" emits with `is_final=true` — the UI must learn the text became final.
- `empty_interim_after_final_emits_nothing` — `(text,is_final)` unchanged → no event — no duplicate final events.
- `full_text_keeps_trailing_interim_for_flush` — `full_text` includes an unreplaced trailing interim — documents what `Flushed` carries.

### Unit tests — parsing / classification (`src/parse.rs`)

- `parses_a_real_results_frame` — a realistic Results frame yields transcript + is_final — happy path of the strict parser.
- `ignores_non_results_types` — Metadata/SpeechStarted/UtteranceEnd/unknown/missing type → ignored — only Results carry transcript.
- `ignores_bad_json_wrong_types_and_missing_fields` — bad JSON, `{"channel":5}`, wrong-typed/missing/null fields, empty alternatives → ignored — frames are hostile input (spec §6), must never panic.
- `ignores_oversized_frames_even_if_valid` — > 1 MiB frame ignored without parsing — bounds CPU/memory per frame.
- `deeply_nested_json_does_not_overflow_the_stack` — 100k-deep arrays/objects → ignored — serde recursion limit protects the reader task.
- `sanitize_reason_strips_controls_and_caps_length` — control chars removed, whitespace collapsed, ≤200 chars, multibyte-safe — server text is quoted to the user.
- `classify_close_codes` — 1000/none after close → Flushed; 1000 before → unexpected end; 1008/4001/4003/4008 → BadKey with code+reason; 1011 → Server — close classification table (spec §6, §10).

### Unit tests — connector (`src/lib.rs`)

- `production_url_has_no_endpointing_or_no_delay_and_no_key` — prod URL is the spec URL, no endpointing/no_delay/token — spec §6 forbids those params; key never in URL.
- `production_keepalive_is_the_config_constant` — default keepalive is `STT_KEEPALIVE_INTERVAL` (8 s) — the test hook must not leak into production.
- `auth_header_is_token_subprotocol_and_sensitive` — header is `token, <key>` and marked sensitive (Debug redacted) — auth mechanism + keep key out of Debug.
- `auth_header_rejects_control_chars_without_echoing_key` — key with a newline → BadKey whose message/Debug omit the key — header injection guard without leaking.
- `ws_config_caps_incoming_messages_above_the_parse_cap` — tungstenite max message size is set and above the 1 MiB parse cap — memory bound + oversize frames are ignored rather than fatal.
- `rustls_has_a_crypto_provider_for_wss` — `rustls::ClientConfig::builder()` does not panic — tokio-tungstenite panics on wss if no crypto provider is compiled in.

### Socket tests (`tests/deepgram.rs`)

- `handshake_sends_token_subprotocol_and_keeps_key_out_of_url` — server sees `Sec-WebSocket-Protocol: token, <key>`, URI has no key/endpointing/no_delay, no Authorization header — spec §6 auth scheme; key out of URLs/logs.
- `audio_frames_arrive_byte_exact_in_order` — 12 full frames + a short final frame arrive as binary, byte-exact LE, in order — transcript quality depends on exact PCM; the drain's partial frame must survive.
- `keepalive_sent_while_idle` — with a 100 ms test interval, idle sockets send `{"type":"KeepAlive"}` repeatedly — Deepgram drops idle sockets (~10 s).
- `close_stream_sent_after_all_audio_and_nothing_after_it` — CloseStream arrives after every frame, nothing (not even KeepAlive) follows, send after it is `Closed` — capture-cutoff invariant (§5.4): CloseStream is always last.
- `scripted_results_produce_transcripts_and_flushed` — interim/final script → exact full-text Transcript events, then CloseStream → final + Metadata + close 1000 → one `Flushed` with the full text — end-to-end finalize path.
- `flushed_includes_trailing_interim_not_replaced_by_a_final` — Flushed carries "done dangling" — documented flush semantics.
- `flushed_with_no_speech_is_empty` — no results → `Flushed{""}` only — session turns this into `no_speech`, never an LLM call.
- `hostile_frames_are_ignored_without_crashing` — bad JSON, `{"channel":5}`, wrong types, non-Results types, 1.5 MiB text, server binary, deep nesting emit nothing; the next valid frame is the first event and the socket still works — hostile-input rule (§6).
- `frame_over_tungstenite_cap_fails_as_server_error_not_panic` — 5 MiB frame → one `Failed{Server}`, then channel ends — memory bound fails safely.
- `close_1008_maps_to_bad_key_with_code` — close 1008 → `Failed{BadKey, "Deepgram closed the connection (code 1008: Invalid credentials)"}` (reason sanitized), then sends are `Closed` — spec §6/§10 keeps the code-1008 message.
- `other_close_code_is_server_failure_with_code` — close 1011 → `Failed{Server}` quoting code and reason — `stt_error` copy must be actionable.
- `bad_close_after_close_stream_is_still_a_failure` — 1011 after CloseStream → exactly one `Failed{Server}`, no Flushed — only a normal close counts as a flush.
- `normal_close_before_close_stream_is_unexpected_end` — 1000 before close_stream → `Failed{Server, "Deepgram ended the stream unexpectedly"}` — a server-side end while recording is an error.
- `abrupt_tcp_drop_mid_stream_is_server_failure_then_send_is_closed` — TCP drop without close frame → `Failed{Server}`, channel ends, send/close_stream → `Err(Closed)` — network loss mid-recording surfaces as `stt_error`.
- `late_close_after_flushed_emits_no_failed` — after close 1000 → Flushed, a TCP drop and stray frame produce nothing more; abort afterwards is fine — §5.5: a late close must never kill a streaming answer.
- `handshake_401_and_403_are_bad_key_without_the_key` — HTTP 401/403 upgrade rejection → `BadKey` quoting the status, key absent from message/Debug/URI — bad-key UX + leak guard.
- `handshake_500_is_connect_with_user_copy` — other HTTP status → `Connect` with `copy::STT_CONNECT` — canonical copy.
- `connection_refused_is_connect` — refused TCP → `Connect` with canonical copy, no key in Debug — network failures map to `stt_connect`.
- `invalid_url_is_connect_not_panic` — malformed URL → `Connect` — the test seam can't panic the core.
- `abort_stops_tasks_and_server_sees_eof` — abort (twice) → server sees EOF, event channel ends with no events, later sends `Closed` — cancel/supersede must drop the socket immediately and never error (§5.10).
- `abort_after_close_stream_also_tears_down` — abort during finalize drops the socket and emits nothing — cancel while finalizing.
- `dropping_sender_before_close_stream_tears_down` — dropping the sender (no close_stream) closes the socket — no leaked sockets if the session forgets to abort.
- `dropping_event_receiver_stops_the_reader` — dropping the receiver after close_stream stops the reader and the socket — no leaked reader task when nobody listens.
- `debug_output_never_contains_the_key` — Debug of connector, Secret, and connect failures never contains the key — spec §14.12.

## Audio (`callcore-audio`) tests

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-audio`.
No test opens a real audio device. Worker tests run `LoopbackSource` over the
scriptable `fake::FakeBackend` (the same seam other crates can reuse through
`LoopbackSource::with_backend`). Waits are bounded waits for a condition or a
message, never fixed sleeps. The one paused-clock test uses no sockets.
`examples/capture_probe.rs` (full worker + DSP path) and `examples/raw_probe.rs`
(the WASAPI backend alone, device rate) are for manual QA on real hardware only
and are not run by the tests. The WASAPI backend's COM code is exercised only by
those probes; its pure pieces (`src/wasapi_format.rs`) are unit-tested.

### DSP (`src/dsp.rs`, unit tests)

- `downmix_stereo_f32_averages_channels`: stereo f32 downmixes to the per-frame channel average. Why: mono is what Deepgram receives (spec §6).
- `downmix_mono_is_identity`: a 1-channel input passes through unchanged. Why: guards the fast path.
- `downmix_six_channels`: a 5.1 frame downmixes to one sample equal to sum/6. Why: loopback devices are often multichannel.
- `downmix_integer_formats_scale_to_unit_range`: i16, u16 and i32 inputs map to -1..1 with the correct origin. Why: the `Samples` seam accepts any of these formats from a backend.
- `downmix_drops_trailing_partial_frame`: a buffer that is not a whole number of frames never produces a garbage sample. Why: malformed-driver robustness.
- `downmix_zero_channels_panics_for_callback_guard`: a zero-channel chunk panics. Why: this is the realistic panic that the callback's `catch_unwind` must contain (see `callback_panic_drops_only_that_chunk`).
- `f32_to_i16_clamps_and_rounds`: out-of-range values, ±inf and NaN clamp safely, and rounding is exact. Why: clipping must never wrap around.
- `rms_normalized`: RMS is 0 for silence and empty input, 1.0 for full scale and 0.5 for half scale. Why: `audio:level` expects 0..1.
- `framer_emits_full_frames_and_flushes_partial`: irregular pushes produce 2048-sample frames, and `flush` returns the exact remainder in order. Why: the final partial frame is part of the capture cutoff (§4.7).
- `framer_exact_multiple_has_no_partial`: an exact multiple of 2048 leaves nothing to flush. Why: prevents an empty trailing frame.
- `passthrough_at_16k_is_identity`: 16 kHz input is passed through bit-exact with no filter. Why: no needless filtering or latency.
- `one_khz_sine_keeps_amplitude_within_1db_at_common_rates`: a 1 kHz tone stays within 1 dB at 44.1k, 48k, 96k, 22.05k, 32k, 88.2k and 8k. Why: the speech band must be flat.
- `resampled_sine_has_correct_frequency_and_phase`: output sample n equals the source sine at time n/16000. Why: proves zero phase error and correct timing, with no offset or stretch.
- `twelve_khz_at_48k_is_attenuated_at_least_40db`: a 12 kHz tone at 48 kHz input is cut by at least 40 dB. Why: anti-aliasing requirement (§6).
- `aliasing_tones_are_attenuated_at_other_rates`: above-Nyquist tones at 44.1k, 96k, 32k and 22.05k are cut by at least 40 dB. Why: anti-aliasing holds at every device rate, not only 48k.
- `dc_passes_with_unit_gain`: a DC level passes exactly, away from the edges. Why: every polyphase phase is normalized, so there is no gain ripple.
- `chunk_split_invariance_is_bit_exact`: output is bit-identical to one-shot processing at every tested split point and with irregular chunks (including empty and 1-sample chunks) at 7 rates. Why: phase continuity across callback boundaries (§6). v1 to v3 bugs lived here.
- `output_length_is_exact_over_long_runs_without_drift`: over 20 s per rate in odd-sized chunks, lag stays bounded and the flushed total equals `ceil(N*16000/rate)`. Why: no drift or lost samples over a recording.
- `total_output_len_formula`: spot-checks the exact-length formula. Why: the worker tests assert against it.
- `untabled_odd_rate_matches_precision`: a coprime rate (44,057 Hz, 16,000 phases, coefficients computed on the fly) is still flat and exact-length. Why: covers odd driver rates.
- `empty_flush_produces_nothing`: flushing an unused resampler emits nothing. Why: a stop straight after start must not invent samples.
- `pipeline_produces_frames_and_exact_total`: 1 s at 48k gives exactly 16,000 samples as full frames plus a partial frame, with the right RMS. Why: end-to-end DSP chain.
- `pipeline_rate_change_keeps_exact_counts`: a mid-stream rate change (device switch) flushes the old tail, so the totals add up exactly. Why: default-device follow (§18) keeps continuity.

### WASAPI format + gap filling (`src/wasapi_format.rs`, unit tests)

- `classify_extensible_float_and_pcm` — `WAVE_FORMAT_EXTENSIBLE` with the IEEE-float or PCM sub-format maps to F32/F64/I24/I32 by bit width — the shared-mode mix format is almost always extensible, so this is the production path.
- `classify_plain_tags` — plain `WAVE_FORMAT_PCM` (8/16-bit) and `WAVE_FORMAT_IEEE_FLOAT` tags classify correctly — older drivers report non-extensible formats.
- `classify_rejects_unsupported` — unknown sub-format, missing sub-format, compressed tags, odd widths, 0 channels and a block align that does not match channels × width are all rejected — a wrong guess would turn the byte stream into noise; failing the open is the safe answer (logged, the user sees the device-open copy).
- `convert_f32_and_f64` — little-endian float bytes convert exactly — the common 48 kHz f32 mix format.
- `convert_integer_formats_to_unit_range` — u8, i16 and i32 map to -1..1 with the correct origin — same scaling as the DSP downmix tests.
- `convert_24bit_packed_sign_extends` — 3-byte packed samples sign-extend correctly (-2^23 → -1.0, 0xFFFFFF → -1/2^23, max → ~1.0) — 24-bit packed is the easiest format to get wrong.
- `convert_ignores_trailing_partial_sample_and_replaces_output` — a trailing partial sample is dropped and the reused scratch buffer is replaced, not appended to — the capture thread reuses one buffer per packet.
- `frames_in_is_floor_and_exact` — duration → frame count is an exact floor, with no overflow over an hour at 192 kHz — the silence math depends on it.
- `gap_filler_is_silent_while_packets_flow` — with packets every 10 ms, no silence is ever inserted — silence must never be mixed into real audio.
- `gap_filler_emits_wall_clock_silence_without_packets` — with no packets, nothing is emitted below the 60 ms threshold, then the whole gap is filled, and 3 s of gap gives exactly 3 s of silence across irregular polls — WASAPI loopback sends NO packets while nothing plays, and the session (Deepgram) needs a steady stream.
- `gap_filler_restarts_per_gap_after_packets_resume` — each gap is measured from the last packet on its own — device vs wall clock drift can never build up.
- `gap_filler_finish_fills_trailing_gap_below_the_threshold` — at stop, a trailing packet-less stretch of 25 ms or more is filled, and only what was not already emitted — the last moments before Stop are not dropped (measured on hardware: without it, up to about 70 ms of silence went missing).
- `gap_filler_caps_backlog_after_a_long_stall` — after a long stall (system sleep), at most 1 s of silence is emitted, and the skipped excess is not owed later — never flood the session with hours of zeros.

### WASAPI backend (`src/wasapi.rs`, unit tests, Windows only, no device)

- `subtype_guids_match_ksmedia` — the local `KSDATAFORMAT_SUBTYPE_PCM` / `_IEEE_FLOAT` GUIDs equal the ksmedia.h values — a typo would reject every real device.
- `read_mix_format_parses_extensible_float` — a packed `WAVEFORMATEXTENSIBLE` (48 kHz stereo float, then 24-bit PCM) is read unaligned and classified correctly — covers the raw-pointer parsing of `GetMixFormat`'s result.
- `device_lost_codes` — `AUDCLNT_E_DEVICE_INVALIDATED` and `AUDCLNT_E_SERVICE_NOT_RUNNING` count as device loss, other HRESULTs do not — only a real loss may end capture with `DeviceLost`.

### Manual hardware QA (not automated)

Default device "Speakers (Realtek(R) Audio)", 48 kHz stereo f32, 2026-09-23.
The cpal 0.15 backend (event-driven loopback) delivered 13 callbacks / ~0.13 s
of audio in 3 s with a tone playing, and 0 frames through the worker. It was
replaced by the polling `WasapiBackend`, which gave:

| Scenario | `raw_probe` (device rate) | `capture_probe` (16 kHz, worker) |
|---|---|---|
| continuous 440 Hz tone | 144000 frames = 3.000 s of device packets, 0 silence | 24 frames, 48000 samples (3.00 s), peak RMS 0.611 |
| nothing playing | 0 device packets, 3.00 to 3.07 s synthesized silence | 24 frames, 3.02 to 3.04 s, peak RMS 0 |
| silence, then tone starts | 0.86 s silence + 2.14 s packets = 3.000 s | — |
| tone ends mid-capture | 0.29 s packets + 2.72 s silence = 3.006 s | 24 frames, peak RMS 0.611 |

Reproduce: play `target-core/tone.wav` with
`powershell -NoProfile -Command "(New-Object System.Media.SoundPlayer '<path>').PlaySync()"`
in the background, wait about 2.5 s (PowerShell start-up varies from 1.5 to 3 s),
then run `cargo run -p callcore-audio --example capture_probe` (or `raw_probe`).

### Worker / `LoopbackSource` (`tests/worker.rs`, fake backend)

- `start_then_frames_flow`: after `start`, fed samples arrive as exact 2048-sample frames with RMS. Why: basic capture path.
- `start_returns_only_once_stream_is_playing`: `start` resolves only after `play()` succeeded. Why: the session arms the record cap at that moment (§4.3).
- `drain_delivers_every_pre_stop_sample_including_final_partial_frame`: every sample fed before stop arrives, in order, including the final partial frame, and then the sink closes. Why: capture cutoff (§4.7, §14.4).
- `drain_at_48k_delivers_exact_resampled_total_including_tail`: the drain includes the resampler tail, so the total equals `ceil(N/3)`. Why: flushing only the remainder was the v3 bug. The tail is part of the drain.
- `nothing_reaches_the_sink_after_drain_returns`: after the drain, the stream is dropped, and a late callback or feed produces nothing. Why: nothing may follow CloseStream (§5.4).
- `stop_discard_pushes_nothing_more`: discard drops the buffered partial audio and closes the sink. Why: the cancel path uses discard, never drain (§5.4).
- `late_callback_after_stop_never_reaches_next_session`: a callback from the old stream after a restart is ignored (generation check), so the new session only sees its own audio. Why: §14.4 cross-session leak.
- `callback_panic_drops_only_that_chunk`: a panicking chunk is dropped, the stream keeps running and later audio still flows. Why: §3, a callback panic must not kill capture.
- `open_failure_maps_to_device_open_copy_without_details`: an open failure returns exactly `copy::DEVICE_OPEN`, with no HRESULT or detail in the message. The sink is not kept, and the worker stays usable. Why: §10 closed error set, and details go to logs only.
- `play_failure_maps_to_device_open_and_drops_stream`: a `play()` failure gives the same copy and the stream is released. Why: start is Ok only when actually playing.
- `drain_time_bound_exceeded_on_worker_reports_timed_out_and_clears_sink`: when the drain bound is exceeded (zero timeout), the result is `timed_out = true` and the sink is still cleared. Why: the drain is bounded (2 s) and the cutoff still holds.
- `drain_reply_timeout_when_worker_is_wedged_still_cuts_the_sink`: with the device stop hung (paused clock), the async side times out, cuts the sink itself, and the worker recovers for the next session. Why: a hung driver must not stall Stop or leak frames past the cutoff.
- `device_lost_flushes_captured_audio_then_reports_once`: on device loss, the captured audio (including the partial frame) is flushed, then exactly one `DeviceLost` is sent, with nothing after it. The later drain reports 0 frames. Why: §18 unplug, answering with what was captured.
- `non_fatal_stream_error_keeps_capturing`: a backend-specific stream error is logged and capture continues. Why: only a real device loss stops capture.
- `default_device_change_rebuilds_stream_and_keeps_continuity`: polling sees the new default device, the stream is rebuilt at the new rate, `DeviceChanged` is pushed and the sample totals stay exact across the switch. Why: §18 default-device follow.
- `default_device_change_with_failed_reopen_reports_device_lost`: if reopening fails, captured audio is flushed and `DeviceLost` (not `DeviceChanged`) is sent. Why: a failed switch must end cleanly.
- `shutdown_joins_promptly_and_later_commands_fail_with_worker_gone`: shutdown stops the stream and joins well inside its bound. Later start and drain return `WorkerGone`, discard is a no-op, and shutdown can be called again. Why: bounded exit.
- `stops_are_idempotent_noops_when_idle`: both stops are safe to call repeatedly with no capture running. Why: cancel is idempotent (§5.10).
- `start_while_capturing_replaces_the_old_capture`: a second start discards the first capture and closes its sink. Why: supersede semantics, with a single live capture.
- `multichannel_integer_input_is_downmixed`: stereo i16 callback data arrives as correctly downmixed mono. Why: format conversion is wired through the worker, not only unit-tested.
- `dropping_the_source_stops_capture`: dropping `LoopbackSource` makes the worker release the device. Why: no orphaned loopback stream at exit.

## Settings store tests (`callcore-settings`)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-settings`.
All store tests run in `tempfile` dirs with a fake keystore (`FakeKeystore`: reversible XOR "encryption" whose protect/unprotect can be told to fail and which counts protect calls). The real DPAPI is exercised under `cfg(windows)` only. No test touches the network or the real `%APPDATA%` settings file, and none sleeps a fixed time.

### Pure helpers (`fsio`, `schema`, `keystore`)
- `utc_stamp_formats_known_instants` — backup timestamps render as `yyyyMMddTHHmmss` UTC, incl. epoch and a leap day — the backup name format is part of the spec and must not depend on a date crate.
- `random6_is_six_base36_chars_and_varies` — backup ids are 6 lowercase base-36 chars and don't repeat — two backups in the same second must not collide.
- `truncate_counts_chars_not_bytes` — truncation limits count chars (incl. emoji), never splitting UTF-8 — a byte-based cut would panic or corrupt text.
- `snap_font_clamps_and_floors_to_grid` — out-of-range/odd/fractional font sizes clamp then snap down to the step-2 grid from the minimum — load must never produce a size the UI can't select.
- `profile_id_regex` — `^[A-Za-z0-9_-]{1,64}$` accepted/rejected edge cases (65 chars, empty, space, non-ASCII, slash) — ids end up in DOM keys and file content.
- `plain_prefix_decodes_base64_or_takes_verbatim` — `plain:<base64>` (v3) decodes, `plain:<raw key>` (older) is taken verbatim, 40-hex Deepgram keys aren't mis-decoded — both legacy forms exist in the wild.
- `secret_value_edge_cases` — null/blank/`plain:` = unset; non-string, bad base64, unknown prefix = invalid; unprefixed plaintext only where allowed; `enc:` = blob — the fail-closed decoding table.
- `dpapi_roundtrip_with_entropy` (windows) — real DPAPI protect/unprotect roundtrip, blob doesn't contain the plaintext — proves the Win32 wiring (buffers, LocalFree) works.
- `dpapi_reads_v3_blob_without_entropy` (windows) — a blob written without entropy (as v3 did) still decrypts — upgrading must not lose keys.
- `dpapi_rejects_garbage_blob` (windows) — non-DPAPI bytes error instead of returning junk — undecryptable must read as unset.

### Load / per-field validation (`tests::load`)
- `missing_file_is_first_run_defaults_without_issue` — no file → FirstRun, spec defaults, no load issue, path/build placeholders set — first launch must be silent.
- `first_run_open_does_not_create_file` — opening never writes — load must be side-effect free.
- `missing_directory_is_first_run_and_first_write_creates_it` — a missing `%APPDATA%\AICallAssistant` is first run and the first save creates it — fresh Windows profiles have no folder.
- `valid_file_loads_all_fields` — every field of a valid v4 file round-trips into the view/bounds — baseline for the fallback tests.
- `invalid_scalar_fields_fall_back_individually` — each bad scalar (revision, provider, style, hotkey, alwaysOnTop, layout, fonts, bounds, active id) falls back alone while the resume survives — one bad field must never cost the user their data.
- `unknown_llm_provider_falls_back_to_anthropic` — unknown provider id → "anthropic"; known one kept — a removed provider must not break answering.
- `fonts_clamp_and_snap_to_step_grid` — file font values clamp and snap (15→14, 100→28, 19.7→18, …) — hand-edited files stay usable.
- `hotkey_empty_kept_as_disabled_and_verbatim` — "" stays "" (disabled), unparseable strings are kept verbatim, limit counts chars — validity is the shell's job; empty must not spring back to the default.
- `bounds_sanity_checked_per_layout` — zero/negative/huge sizes, floats, out-of-i32 coords, missing keys and arrays are rejected; negative coords allowed — multi-monitor coords are negative, garbage must not open a 0×0 window.
- `profiles_invalid_entries_dropped_individually` — bad ids / non-objects are dropped one by one, valid neighbours kept — per-profile, not all-or-nothing.
- `profiles_duplicate_ids_keep_first` — duplicate ids are deduped, first wins — ids must be unique for selection.
- `profiles_truncate_by_chars_not_bytes` — name (trimmed) 60, focus 2000, texts 200 000 chars, multibyte-safe — spec limits apply on load without losing the rest.
- `profile_missing_or_blank_name_becomes_untitled_and_non_string_text_is_empty` — nameless profiles get "Untitled"; non-string text fields become "" — keep the profile rather than dropping it.
- `profiles_capped_at_twenty` — only the first 20 valid profiles load; an active id beyond the cap falls back to the first — max-20 invariant.
- `zero_valid_profiles_creates_default` — empty/invalid/non-array profiles → one "Default" profile, active fixed — ≥1 profile invariant.
- `unknown_call_type_becomes_behavioral` — unknown/non-string callType → behavioral — spec §8 fallback.
- `active_profile_falls_back_to_first` — dangling activeProfileId → first profile — never point at a missing profile.
- `utf8_bom_prefixed_file_loads` — a BOM-prefixed file (Notepad/PowerShell) loads as Ok — hand-edits must not be treated as corrupt.
- `non_object_or_invalid_json_is_corrupt` — arrays, strings, null, truncated JSON, empty and non-UTF-8 files → Corrupt issue with defaults — the corrupt path must trigger for every non-object.

### v3 migration (`tests::migrate`)
- `v3_legacy_top_level_resume_becomes_default_profile` — top-level `resume`/`jobDescription` with no profiles → one "Default" profile — the oldest v3 shape.
- `v3_legacy_snake_case_job_description` — top-level `job_description` accepted — snake_case variant seen in older files.
- `v3_profile_snake_case_aliases_camel_wins` — profile `call_type`/`job_description` accepted; camelCase wins when both exist — v3 profile shape variants.
- `v3_top_level_mirrors_ignored_when_profiles_exist` — v3's write-only top-level mirrors don't override real profiles — mirrors could be stale.
- `full_v3_file_migrates_and_is_rewritten_as_v4` — a complete v3 file loads, and the first write produces a v4 file (version 4, no mirrors, `enc:`→`dpapi:`) that reloads with the same key — end-to-end upgrade.
- `legacy_top_level_api_key_fields` — `deepgramApiKey` (unprefixed), `anthropicApiKey` (`plain:` base64), `groqApiKey` (`enc:`) all load as usable encrypted keys — every legacy key location.
- `legacy_api_keys_object` — `apiKeys` object with `plain:`/`dpapi:`; unprefixed there is invalid → Unreadable — only top-level fields may hold raw keys.
- `secret_sources_precedence_secrets_over_apikeys_over_top_level` — `secrets` > `apiKeys` > top-level — deterministic choice when a file has several.
- `plain_secret_reencrypted_on_first_write_and_plaintext_gone` — plaintext stays untouched on open, and after the first save neither the key nor its base64 nor `plain:`/legacy fields remain on disk — spec §14.9: no plaintext left behind.
- `plain_secret_reencrypted_by_geometry_autosave_too` — geometry autosave also migrates plaintext — "first successful write of any kind".
- `plain_secret_when_dpapi_fails_reads_unset_and_is_preserved` — if DPAPI can't encrypt a legacy key it reads as unset/Unreadable (fail closed), is preserved on disk, and is migrated on a later write once DPAPI works — never use or lose a key we can't protect.
- `undecryptable_blob_reads_unset_and_is_preserved` — a foreign-user blob reads as unset/Unreadable and survives writes verbatim — copying the file back must still work.
- `invalid_secret_values_read_unreadable_and_are_dropped_on_write` — bad base64 / non-string → Unreadable, empty → Unset; dropped on write — garbage isn't carried forever.
- `unknown_secret_ids_are_preserved_but_not_listed` — secrets for unknown ids survive writes but aren't in `keys` — a provider removed in one build keeps its key for the next.

### Corrupt / unreadable file backup (`tests::backup`)
- `corrupt_file_untouched_when_closed_without_writing` — open corrupt, view, drop: bytes identical, no backup/temp files — spec §14.8: closing untouched must not destroy the file.
- `corrupt_file_backed_up_before_first_patch` — first patch creates exactly one `settings.json.corrupt-<stamp>-<id>.bak` holding the original bytes, issue gets `backupPath` and a message naming it — backup-before-overwrite + UI surfacing.
- `corrupt_file_backed_up_before_first_geometry_save` — geometry autosave obeys the same rule — the v3 bug path (autosave clobbered the corrupt file).
- `backup_happens_only_once` — later writes don't make more backups — no .bak spam.
- `rejected_patch_does_not_back_up_or_write` — stale/invalid patches on a corrupt load leave file and dir untouched — only real writes trigger the backup.
- `first_run_write_makes_no_backup` — no file → no backup — nothing to preserve.
- `unreadable_file_loads_defaults_and_blocks_writes_when_backup_fails` — `settings.json` as a directory → Unreadable issue; the first write's backup fails → `writesBlocked`, actionable error, all later writes refused, original untouched — "if the backup fails, refuse to write".
- `unreadable_file_is_backed_up_by_reread_when_it_becomes_readable` — an unreadable file that becomes readable before the first write is re-read and backed up as `settings.json.unreadable-…bak` — transient AV locks.
- `corrupt_backup_failure_blocks_all_writes_even_after_recovery` — backup impossible (settings dir replaced by a file) → internal error naming the file, `writesBlocked`, and writes stay refused even after the dir comes back; nothing applied in memory — refusal is sticky because the original was never preserved.

### Writes, revisions, concurrency (`tests::write`)
- `stale_revision_rejected_nothing_applied_file_identical` — a stale `baseRevision` → internal error saying nothing was saved; file bytes, memory, revision identical; no encryption attempted — spec §14.10 revisions.
- `future_revision_is_also_rejected` — base ahead of current is rejected too — only exact matches apply.
- `revision_bumps_exactly_once_per_save` — each successful save bumps by exactly 1, persisted and reloaded — the page's baseRevision bookkeeping depends on it.
- `save_bounds_persists_without_bumping_revision` — geometry saves persist per layout and don't bump the revision, so an open settings form stays valid — autosave must not make every save stale.
- `save_bounds_rejects_insane_sizes` — 0 or ≥100000 sizes are refused, nothing written — garbage geometry never reaches disk.
- `invalid_patch_values_rejected_nothing_applied` — odd/out-of-range fonts, unknown provider, long hotkey, 0/21 profiles, bad/long/duplicate ids, blank/long names, over-long focus/texts, unknown active id, and a mix of valid+invalid fields → Err, file and view unchanged — saves must not silently coerce.
- `profile_text_error_does_not_echo_profile_text` — validation errors never quote resume text — §14.12.
- `valid_patch_applies_every_field` — every patch field applies (names trimmed, empty hotkey kept) and reloads identically — baseline for the rejection tests.
- `replacing_profiles_without_active_falls_back_to_first` — deleting the active profile without naming a new one selects the first; an explicit id in the same patch wins — ≥1/active invariants on save.
- `twenty_profiles_accepted` — exactly 20 profiles save — boundary of the max-20 rule.
- `concurrent_same_base_patches_exactly_one_wins` — two threads patching from the same base (×10 rounds): exactly one succeeds, revision 1, disk = winner — serialized, revision-checked writes.
- `concurrent_writers_are_serialized_without_lost_updates` — 6 threads × 5 retried patches interleaved with geometry saves end at revision 30 with no temp files — no lost updates under contention.
- `atomic_write_leaves_no_temp_files` — repeated saves leave only `settings.json` — temp files are always renamed or removed.
- `failed_write_leaves_memory_and_no_temp_file` — a failing rename (target is a non-empty dir) → error saying nothing was saved, memory unchanged, temp removed — memory must match disk.
- `written_file_is_pretty_camel_case_v4` — the file is pretty-printed with exactly the 14 v4 camelCase keys and snake_case enum values — on-disk schema contract.
- `answer_config_reflects_active_profile_style_provider` — `answer_config()` returns the active profile, its call type, style and provider — what the session builds prompts from.

### Secrets (`tests::secrets`)
- `set_secret_encrypts_on_disk_and_view_shows_status_only` — Set stores `dpapi:` (no plaintext on disk), view shows has_key/Encrypted, reload decrypts — write-only secrets.
- `set_secret_value_is_trimmed` — pasted keys lose surrounding whitespace — trailing newlines break auth headers.
- `empty_set_is_a_noop_and_keeps_the_key` — Set with blank value keeps the old key and encrypts nothing (save still succeeds and bumps revision) — "emptying the field does not remove a key".
- `remove_deletes_the_key` — explicit Remove deletes only that key (absent key OK) — the only way to delete.
- `secret_changes_apply_in_order` — Set+Remove / Remove+Set in one patch apply in order — "typing a new key cancels a queued Remove".
- `encryption_failure_is_fail_closed` — DPAPI failure refuses the whole patch (other fields too): file bytes identical, no plaintext, revision unchanged, previous key still decrypts — spec §14.9.
- `unknown_key_id_rejected` — Set/Remove for an unknown key id → Err, nothing written — no stray secrets.
- `malformed_key_rejected_without_echoing_it` — smart quotes, inner spaces, control chars, >4096 chars → Err whose message doesn't contain the value; nothing encrypted — helpful error without leaking.
- `get_secret_unset_unknown_and_undecryptable_return_none` — unset, unknown id and decrypt failure all read as `Ok(None)` — "undecryptable values read as unset".
- `keys_list_ids_labels_and_urls` — keys = deepgram then provider key ids (deduped), with the spec labels/URLs; unknown ids use the id as label — the Settings key list.
- `view_never_contains_key_material` — the serialized view contains no key, legacy plaintext, blob prefix or blob bytes — key material never enters a view.
- `debug_never_prints_keys_or_profile_text` — `{:?}`/`{:#?}` of the store (incl. a Plain legacy secret) contains no key, resume or notes text — §14.12.
- `errors_never_contain_key_material` — encryption, stale and unknown-id errors never include the key — errors reach the UI and logs.
- `default_dir_is_appdata_aicallassistant` — `default_dir()` is `%APPDATA%\AICallAssistant` — keeps the v3 path so migration finds the file.
- `real_dpapi_store_roundtrip` (windows) — store + real DpapiKeystore: set, file has no plaintext, reopen decrypts, status Encrypted — production wiring.
- `dpapi_fails_closed_off_windows` (non-windows) — DpapiKeystore errors off Windows — no plaintext fallback anywhere.

## Session actor tests (`crates/session`)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-session` (71 tests, deterministic; run 3x in a row green).

### How the suite works

- **Fakes, no I/O.** `tests/support/mod.rs` has a scriptable fake for every port: `FakeAudio` (start delay/failure, frames pushed on demand, drain delay + tail frames, records `Start`/`Discard`/`Drain` calls, keeps every sink so a test can post a "late callback"), `FakeStt` (connect delay/failure, logs frames / `Close` / `Abort` / `Dropped` per connection in order, auto or manual `Flushed`, optional late `Failed`, send failure, stream end), `FakeProvider` (scripted `Sleep`/`Delta`/`Return`/`Hang` steps, counts builds and stream calls, captures request bytes, prompts, keys and live stream futures), `FakeSettings` (keys present/absent/blank, read error, a gate that blocks one `get_secret` on its blocking thread), `RecordingSink` (collects `CoreEvent`s, `wait_for(pred)` with a bounded timeout, no fixed sleeps), `FakeClock` (epoch ms driven by tokio's clock).
- **Paused time.** Nearly every test is `#[tokio::test(start_paused = true)]`. All fake delays use `tokio::time`, so timers (cap 120 s, connect 5 s, finalize 5 s, first token 10 s, total 60 s) are exact and instant. No sockets are involved, so paused time is safe. One test runs on real time with a 4-thread runtime to prove nothing depends on current-thread scheduling.
- **Leak checks.** A test counts a task as gone when its resource is released: the audio sink's receiver is closed (relay task gone), the STT event receiver is closed (reader gone), the fake sender logged `Dropped` (writer gone, `abort()` called), the delta receiver is closed and `live_streams()==0` (provider future dropped). Timers are deadlines stored in the session state, not tasks, so dropping the session clears them. Tests also let 300–600 virtual seconds pass after an exit and assert that no event appears.
- **Mutation check (done once by hand).** Each of these changes to the actor made at least one test fail: dropping the ticket re-check, draining before the socket opens, retrying without the attempt limit or after a delta, starting the latency clock after the drain, not dropping the oldest buffered frame, emitting levels after Stop, not discarding on teardown, not auto-stopping on device loss, and removing both invariant-5 defenses (the `Done` guard and the reader abort).

### Design choices the tests pin down

- A superseded or cancelled session emits **nothing** further, not even a terminal event. Shutdown behaves the same way.
- Device-open failure ends the session at once with `internal` + `copy::DEVICE_OPEN`, so an early Stop can never turn it into `no_speech` (§14.11).
- Device loss emits `audio:device{lost}` and then takes the stop path, with no `session:autostopped` (that event is only for the 120 s cap).
- The first-token (10 s) and total (60 s) watchdogs start when the first provider attempt starts. A retry stays inside the same budget.
- Profile, call type and style are re-read when the answer starts, so a style chip flipped mid-recording applies. The provider and its key are the ones read at Record time.
- All audio calls go through one lane task, so a superseded session's `stop_discard` always reaches the device before the next `start`.

### Unit tests (`src/errors.rs`)

- `provider_mapping_is_the_closed_set` — checks every `ProviderFailureKind` against its code (Auth→llm_auth, RateLimit→llm_rate_limit, Aborted→aborted, rest→llm_http) and that the provider's message is kept — why: spec §10 mapping is the contract the UI keys behavior on.
- `provider_mapping_fills_empty_messages` — checks that an empty failure message gets actionable fallback copy — why: the user must never see a blank error.
- `stt_mapping` — checks BadKey→stt_connect keeping "1008", Connect→canonical connect copy, Server/Closed→stt_error — why: §10 STT mapping.
- `audio_mapping` — checks that device errors map to `internal` + `copy::DEVICE_OPEN` — why: §10 says device-open uses internal with the canonical copy.

### Flows (`tests/flows.rs`)

- `happy_path_record_stop_done_exact_sequence` — checks the exact event sequence for record→frames→partial→stop→done, the frames→Close→Abort→Dropped order on the socket, Start+Drain on audio, keys used, one build, one stream, two prewarms, and idle status afterwards — why: the core flow of §4, pinned end to end.
- `prompt_is_built_from_profile_style_and_call_type_at_answer_time` — checks that the provider receives exactly `build_prompt(...)` for the active profile with the style/call type current at answer time, and that `llm:done.callType` matches — why: the prompt must come from the real prompt crate with the right inputs, without depending on its text.
- `stop_during_connect_waits_for_socket_then_drains` — checks that Stop while connecting is accepted, the drain waits for the socket, all frames (including those captured after Stop and the drain tail) reach STT before Close, `audioDrainMs` equals only the drain (200 ms), and firstTokenMs includes the connect wait — why: §4.7 and the §18 gap "audioDrainMs excluded STT connect time".
- `pre_connect_buffer_caps_at_120_frames_dropping_oldest` — checks that 130 frames sent before the socket opens leave frames 10..129 flushed in order, followed by live frames — why: §4.2 buffer rule (max 120, drop oldest).
- `record_cap_autostops_then_answers` — checks that at exactly the advertised deadline the session emits `session:autostopped`, drains, answers, and a later Stop is refused — why: §4.3 cap plus "same path as stop".
- `device_lost_autostops_and_answers_with_captured_audio` — checks that DeviceLost emits `audio:device{lost}` with canonical copy, then drains and answers with what was captured, without `session:autostopped` — why: §18 known gap (mid-recording unplug).
- `device_changed_only_notifies` — checks that DeviceChanged emits `audio:device{changed}` and recording continues — why: §18 (default-device switch is followed, not fatal).
- `ask_answers_directly_with_zero_audio_metrics` — checks that ask trims, emits one final `stt:partial`, starts in answering, drain/finalize are 0, first/total are measured from ask acceptance, and no audio/STT is used — why: §4 Ask.
- `ask_without_deepgram_key_still_works` — checks that ask only needs the provider key — why: a typed question must not require a Deepgram key.
- `ask_while_recording_supersedes_silently` — checks that ask during a recording discards the old capture (never drains, never sends CloseStream, aborts the socket) and the old session gets no terminal event — why: invariant 1 and the chosen silent-supersede semantics.
- `empty_ask_is_rejected_and_leaves_live_session_untouched` — checks that empty/whitespace asks return `EMPTY_QUESTION` without reading keys, and the live recording stays stoppable — why: §4 "bad input must not kill a live session".
- `missing_keys_error_without_touching_the_live_session` — checks no_stt_key (missing or blank), no_llm_key, and key-read errors → internal, with the live session unaffected — why: a failed start must never supersede.
- `selected_provider_is_used_and_unknown_id_falls_back_to_default` — checks that the configured provider and its key are used, and an unknown id falls back to the default — why: the provider-agnostic registry lookup.
- `status_walks_starting_recording_finalizing_answering_idle` — checks the status watch transitions Idle→Starting→Recording→Finalizing→Answering→Idle, and that the status recording info equals the `session:recording` event (deadline = clock + cap) — why: the shell's snapshot revision and page re-adoption rely on it (§9, §18).
- `session_ids_are_sequential_and_never_reused` — checks ids s1, s2, s3 across start/start/ask — why: ids must be unique per process run.

### Invariants and lessons (`tests/invariants.rs`)

- `inv1_new_start_supersedes_the_live_session` — checks that a second start discards the first before starting (lane order Start, Discard, Start), aborts its socket, refuses Stop for s1, and s2 works — why: invariant 1.
- `inv1_superseded_session_emits_nothing_further` — checks that a superseded ask with a hung provider gets no further event even after its watchdogs would have fired, and its stream is dropped — why: invariants 1, 8 and 9.
- `inv1_supersede_during_audio_start_discards_after_start` — checks that superseding while the device is still opening leaves s1's late start result ignored, followed by Discard then the new Start — why: device calls must stay ordered so a late discard never kills the next capture (§14.4).
- `inv2_stalled_key_read_loses_to_newer_start` — checks that a start whose key read stalls, then resolves after a newer start installed, returns `aborted`, never installs, never touches the winner, and consumes no session number — why: invariant 2 and §4.1 "a stalled read must never supersede a newer command".
- `inv2_late_cancel_of_an_old_command_only_hits_its_own_session` — checks that cancelling s1 after s2 exists leaves s2 alone — why: invariant 2 (late cleanup is scoped) and lesson 2.
- `inv3_late_results_of_a_superseded_session_are_dropped` — checks that late STT transcript/flush events and a late audio callback for s1 produce no events and no frames — why: invariant 3.
- `inv3_late_provider_result_after_cancel_is_ignored` — checks that a provider that would answer 2 s after cancel produces nothing — why: invariant 3 and lesson 2.
- `inv4_capture_cutoff_and_close_stream_is_last` — checks that frames arriving during the drain and the drain's final partial frames are sent, CloseStream is sent exactly once after the last frame, the sink is dead after the cutoff, and no levels are emitted after Stop — why: invariant 4 and lesson 4.
- `inv4_cancel_uses_discard_never_drain_or_close` — checks that cancel while recording gives Discard (no Drain), socket Abort (no Close), and no event — why: invariant 4 (cancel = discard) and invariant 10.
- `inv4_cancel_during_finalize_discards_and_emits_nothing` — checks that cancel after CloseStream ignores a later Flushed and never calls the provider — why: cancel wins in every phase.
- `inv5_late_stt_failure_after_flushed_is_ignored` — checks that a socket failure right after the final transcript does not kill the streaming answer — why: invariant 5.
- `inv5_stt_stream_end_after_flushed_is_ignored` — checks that a transport error after a manual Flushed is ignored and there is exactly one terminal event — why: invariant 5.
- `inv6_no_retry_after_a_delta` — checks that a Connect failure after a delta was painted becomes an error, with a single stream call — why: invariant 6.
- `inv7_whitespace_transcript_is_no_speech_and_provider_never_called` — checks that an empty/whitespace final transcript gives `no_speech` with zero builds and zero stream calls — why: invariant 7.
- `inv8_watchdog_then_late_provider_result_is_one_terminal` — checks that after a first-token timeout, the provider's late answer adds no delta or second terminal — why: invariant 8.
- `inv8_stt_failure_racing_finalize_is_one_terminal` — checks that Failed followed by Flushed gives one stt_error and no provider call — why: invariant 8.
- `inv9_no_timer_fires_and_no_task_survives_after_done` — checks that after llm:done, 600 s of virtual time produce no events and every relay/reader/writer/stream is gone — why: invariant 9 plus no leaked tasks.
- `inv9_no_task_survives_cancel_in_any_phase` — checks that cancel while recording and while answering (hung provider) releases every task, and no watchdog fires later — why: invariant 9.
- `inv9_no_task_survives_an_error_exit` — checks that an STT error while recording discard-stops the audio, releases tasks, and stays a single terminal — why: invariant 9 on the error path.
- `inv10_cancel_is_idempotent_and_scoped` — checks that cancelling an unknown id, cancelling repeatedly, and cancelling after done are all harmless no-ops that only affect their own id — why: invariant 10.
- `inv11_status_deadline_equals_event_deadline_and_cap_fires_there` — checks that status deadline == event deadline and the autostop happens at exactly that epoch ms — why: invariant 11.
- `lesson11_device_open_failure_with_early_stop_reports_device_error` — checks that when Stop lands before the failing device open resolves, the empty transcript reports the device error (not no_speech) and the provider is never called — why: §14.11.
- `device_open_failure_while_recording_errors_immediately` — checks that a device-open failure ends the session at once with the device error, with no Drain or Discard for a device that never opened — why: the device error must surface promptly and correctly.
- `secrets_and_profile_text_never_appear_in_events` — checks that the serialized events of a full run plus an auth error contain neither key nor resume text — why: §14.12.

### Answer streaming (`tests/answer.rs`)

- `retries_once_on_connect_before_any_delta_with_identical_bytes` — checks that a Connect failure before any delta retries once with the byte-identical request (built once), and first-token time includes the failed attempt — why: §5.6/§7 retry policy.
- `second_connect_failure_is_not_retried_again` — checks that two Connect failures give an llm_http error after exactly 2 calls — why: "retry ONCE".
- `empty_answer_is_never_retried` — checks that EmptyAnswer gives llm_http after one call — why: §7 empty_answer is never retried.
- `whitespace_ok_outcome_is_an_error_not_a_blank_done` — checks that a provider "success" with a whitespace answer becomes an error, not a blank llm:done — why: §7 "never a blank finished entry".
- `non_connect_failures_are_not_retried_and_map_to_the_closed_set` — checks every other failure kind maps to its §10 code with the provider's copy, and none are retried — why: §10 plus the retry policy.
- `finish_reason_is_passed_through` — checks that `finish=truncated` reaches llm:done — why: the UI shows a note for truncated/refused.
- `first_token_timeout_fires_at_ten_seconds_and_drops_the_stream` — checks llm_first_token_timeout at exactly 10 s and that the provider future is dropped — why: first-token watchdog, abort by dropping the future.
- `total_timeout_fires_at_sixty_seconds_even_while_streaming` — checks llm_timeout at exactly 60 s despite deltas flowing, with the stream dropped and no retry — why: total watchdog.
- `a_delta_disarms_the_first_token_watchdog` — checks that a first delta at 9 s followed by a 20 s gap still completes, with exact first/total metrics — why: the first-token watchdog must disarm on the first delta.
- `stt_failure_before_flushed_is_stt_error` — checks that a Server failure after CloseStream but before Flushed gives stt_error with its message and no LLM call — why: §5.5/§10.
- `stt_bad_key_mid_stream_keeps_the_1008_message` — checks BadKey → stt_connect keeping the "code 1008" copy — why: §6/§10.
- `stt_event_stream_ending_before_flushed_is_stt_error` — checks that the event stream ending without Flushed gives stt_error — why: a silent socket death must not hang or turn into no_speech.
- `stt_send_failure_is_stt_error` — checks that a failed frame send gives stt_error and discards the capture — why: the writer's failure path.
- `stt_connect_failure_is_stt_connect` — checks that a connect failure gives stt_connect with canonical copy and stops the capture — why: §10.
- `stt_connect_timeout_is_stt_connect_after_five_seconds` — checks that a hanging connect fails at exactly 5 s — why: the STT connect timeout.
- `stop_during_connect_then_connect_timeout_is_stt_connect` — checks that Stop during a connect that never completes ends with stt_connect, no drain, and no LLM call — why: "wait for the socket (bounded by the connect timeout)".
- `finalize_timeout_is_stt_timeout_after_five_seconds` — checks that no Flushed within 5 s of CloseStream gives stt_timeout and no LLM call — why: the finalize watchdog.
- `prompt_uses_the_flushed_transcript_not_the_last_partial` — checks that the prompt and llm:done carry the final flushed transcript — why: the server flush is the source of truth.

### Metrics (`tests/metrics.rs`)

- `metrics_are_exact_and_measured_from_stop_acceptance` — checks exact audioDrain/sttFinalize/firstToken/total for known delays after a 30 s recording — why: §9 definitions.
- `injected_drain_delay_increases_first_token_ms_by_exactly_d` — checks that for D in {1, 250, 1234, 1999} ms, firstTokenMs and totalMs grow by exactly D and audioDrainMs == D — why: the explicit §9 test ("never hide latency", lesson 5).
- `recording_length_does_not_count_toward_latency` — checks that a 10 ms recording and a 100 s recording give identical metrics — why: the clock starts at Stop acceptance, not at Record.
- `total_includes_streaming_after_the_first_token` — checks that ask metrics give first=200 and total=900 with drain/finalize 0 — why: total covers the whole stream.

### Lifecycle (`tests/lifecycle.rs`)

- `shutdown_discards_the_live_session_and_later_calls_fail` — checks that shutdown discard-stops audio (done before it returns), aborts the socket, emits nothing, and later start/ask/stop return internal `SHUTTING_DOWN` while cancel and shutdown stay harmless and status stays readable — why: the exit path must be bounded and clean (§13 Close).
- `shutdown_while_answering_drops_the_provider_stream` — checks that shutdown drops an in-flight provider stream — why: no work may outlive exit.
- `shutdown_with_nothing_live_is_quick_and_idempotent` — checks a double shutdown with no session — why: idempotent exit.
- `dropping_every_handle_stops_the_actor_and_discards` — checks that the actor keeps running while any clone lives and discard-stops when the last one drops — why: no orphaned capture if the shell drops the handle.
- `stop_of_unknown_or_ask_session_is_not_taken` — checks that Stop for an unknown id, for an ask session, and a second Stop are refused, with a single drain — why: §4.6 "accepted only for the live, not-yet-stopping session".
- `real_time_multi_thread_smoke` — checks 5 real-time record/stop/done rounds plus a 20-start supersede storm on a 4-thread runtime — why: proves correctness doesn't depend on paused time or current-thread scheduling.

## Contract (`callcore-contract`)

Run: `cargo test -p callcore-contract`. This also exports every `#[ts(export)]` type to `src/generated/`.

- `event_envelope_wire_shape` — `EventEnvelope` flattens `seq` next to the internally tagged event (`{"seq":7,"type":"stt:partial","sessionId":…}`), `llm:done` serializes camelCase with snake_case enums, and `hotkey:toggle` has no payload. Why: the page parses exactly these shapes, and the TS types can't catch a serde rename drift.
- `cmd_result_wire_shape` — `CmdResult` serializes as `{ok:true,value}` / `{ok:false,error:{code,message}}`. Why: every command reply uses this envelope (§9), and it is hand-written on the TS side.
- `secret_change_wire_shape` — `{"keyId","action":"set","value"}` and `{"action":"remove"}` deserialize to `SecretAction`. Why: the settings form sends exactly these.
- `lossy_fallbacks` — unknown call type → behavioral, unknown style → balanced. Why: spec §8 fallbacks.
- `debug_never_shows_the_key` (`secret.rs`) — `{:?}` of a `Secret` (also inside `Option`) never shows the key. Why: §14.12. Every crate relies on `Secret`'s redacting Debug.

## Shell logic (`callcore-shell`)

Run: `cargo test -p callcore-shell`. These are pure unit tests. The event-pump tests use a real worker thread with bounded condition waits. The command-gate tests use a paused clock (no sockets).

### Event pump (`src/events.rs`)
- `delivers_in_order_with_increasing_seq` — envelopes arrive in emit order with strictly increasing `seq`. Why: §9, the page orders and dedupes by `seq`.
- `buffers_while_detached_and_flushes_on_attach` — events emitted before any page subscribed are delivered, in order, on attach. Why: startup and reload windows must not lose events.
- `seq_is_monotonic_across_attach_and_detach` — attach/detach never resets `seq`. Why: §14.7, a reset would make the page drop legitimate events.
- `levels_coalesce_latest_wins_per_session` — a queued `audio:level` is replaced in place by the newer one for the same session. Why: §9, levels coalesce latest-wins.
- `must_deliver_and_deltas_survive_saturation` — over the soft bound, only levels are dropped: terminal/protection events and `llm:delta`/`stt:partial` all arrive. Why: §14.6, backpressure must never drop terminal or protection events.
- `delivery_failure_detaches_and_keeps_the_event` — a failed delivery detaches the target and re-queues the envelope, which the next attach delivers. Why: §14.6, nothing is lost across a renderer crash.
- `recovers_after_many_failures_and_reattaches` — repeated failures never ratchet the pump shut, and a new attach works. Why: the v3 "hung dispatch" counter bug (§14.6).
- `a_panicking_delivery_is_treated_as_a_failure` — a panic in `deliver` is caught, detaches, and keeps the event. Why: the pump thread must survive.
- `detached_buffer_drops_levels_first` — the detached soft bound drops levels before anything else. Why: bounded memory without losing content.
- `hard_cap_never_drops_must_deliver` — past the hard cap only non-must-deliver events are evicted. Why: terminal/protection events are never dropped.
- `replacing_the_target_routes_new_events_to_the_new_one` — a re-subscribe sends new events only to the new channel. Why: page reload replaces the old channel (§9 `subscribe_events`).
- `emit_does_not_block_while_the_renderer_is_stalled` — 10 000 emits return quickly while delivery is stalled, and everything is delivered once it resumes. Why: `emit` is called from the session actor and must never block (the `EventSink` contract).

### Status hub (`src/status.rs`)
- `starts_starting_unknown_idle` — the initial snapshot is core `starting`, protection `unknown`, session idle. Why: never claim protection or readiness before it is confirmed (§11, §3).
- `every_change_bumps_the_revision_and_no_op_does_not` — core/protection/session changes bump the revision once, and identical sets don't. Why: §9 revision rules.
- `core_failed_carries_the_error` — `set_core_failed` stores the error in the snapshot. Why: the page shows the core's actionable error.
- `protection_regression_is_a_change` — protected → unprotected bumps the revision. Why: a regression must reach the page and beat older snapshots.
- `unknown_session_snapshot_keeps_revision_and_id` — the `get_status` busy fallback reports phase `unknown` without bumping the revision. Why: "unknown" is not a state change, and the page treats it as "wait again" (§14.3).
- `snapshot_serializes_camel_case` — the wire shape of `StatusSnapshot`. Why: contract with the page.

### Hotkey (`src/hotkey.rs`)
- `parses_the_default` — `Ctrl+Shift+Space` parses to the right mods and key. Why: the default must always be valid.
- `modifiers_are_case_insensitive_and_normalized_in_order` — `shift+CTRL+space` becomes `Ctrl+Shift+Space`, and aliases (Control, Super/Meta/Cmd → Win) are accepted. Why: users type shortcuts loosely, and the display must be canonical.
- `key_families` — letters, digits, F1–F24, arrows, navigation keys and punctuation map to canonical names. Why: "any Ctrl/Alt/Shift/Win + key" (§13).
- `empty_is_disabled` — an empty or whitespace string means disabled, not invalid. Why: §12, empty = disabled.
- `invalid_inputs` — no modifier, two keys, empty parts and unknown keys are invalid with a reason. Why: honest status.
- `exactly_100_chars_is_accepted_by_the_length_rule` — the 100-char limit boundary. Why: §12 `hotkey ≤ 100 chars`.
- `messages` — exact user copy for disabled / invalid / taken. Why: the settings view shows it verbatim.
- `all_key_names_are_distinct_and_parse_back` — every canonical key name round-trips. Why: the glue test checks the plugin accepts each one.
- `happy_path_registers` — a valid hotkey plans a register and reports `registered` on success. Why: basic registrar flow.
- `change_unregisters_the_previous_one` — a new hotkey unregisters the old accelerator first. Why: no ghost shortcuts.
- `taken_hotkey_reports_unavailable` — a failed registration reports `unavailable` with the "already taken" copy. Why: honest status.
- `disabled_and_invalid_are_immediate_and_unregister` — no OS registration is attempted, and the old one is removed. Why: turning the hotkey off must really free it.
- `late_success_after_a_newer_request_is_undone` — a stale registration that succeeds late yields `Undo`. Why: §13 "a registration that completes late must be undone".
- `late_success_after_disable_is_undone` — the same after the user disabled the hotkey. Why: disabled must mean disabled.
- `late_failure_is_ignored` — a stale failure changes nothing. Why: only the newest request reports.
- `late_success_of_the_same_combo_is_adopted_when_the_newer_failed` — if the newest request wants exactly that combo, the late success is kept. Why: don't unregister what the user wants.
- `in_flight_registration_is_not_reported_as_registered` — while registering, status is not `registered`. Why: honest status.
- `late_success_before_newer_completes_is_undone` — an ordering variant of the late-success race. Why: the undo must not depend on completion order.

### Geometry (`src/geometry.rs`)
- `dock_centres_on_the_work_area_top` — dock = top of the work area, horizontally centred. Why: §13 camera-line docking.
- `dock_on_negative_monitor` — docking on a monitor left of the primary (negative x). Why: multi-monitor coordinates can be negative.
- `dock_wider_than_work_area_starts_at_left_edge` — an oversized window docks at the left edge. Why: the title bar must stay reachable.
- `saved_position_on_left_negative_monitor_is_reused` — a reachable saved position on a negative monitor is kept. Why: restore must work on every monitor.
- `saved_position_from_unplugged_monitor_docks_on_current` — a position on a monitor that is gone docks on the current monitor at the saved size. Why: §13 geometry restore.
- `title_bar_off_screen_top_is_not_reachable` — a window whose title strip sits above every work area is not reused. Why: the window must never be ungrabbable.
- `only_a_sliver_visible_is_not_reachable` — less than 40 px of title strip visible is rejected. Why: §13 "≥ 40 px".
- `straddling_two_monitors_is_reachable` — a strip split across monitors counts. Why: no false resets.
- `stacked_monitors` — vertically stacked layouts. Why: covers negative y.
- `mixed_dpi_default_size_scales_with_the_target_monitor` — default sizes scale by the target monitor's DPI (100/125/150 %). Why: §13 per-monitor DPI.
- `saved_size_is_clamped_to_min` — a saved size below the layout minimum is raised. Why: 380×520 / 380×160 minimums.
- `tiny_work_area_clamps_but_never_below_min` — the size fits the work area, but the minimum wins. Why: tiny screens still get a usable window.
- `prompter_default_fits_wide_screen` — the 900×200 prompter default fits and centres. Why: §13 prompter strip.
- `no_monitors_still_gives_a_usable_size` — with no monitor info, a usable size at (0,0). Why: §13 "with no monitor info still apply a usable size".
- `first_run_docks_top_centre_of_primary` — no saved bounds → dock top-centre of the primary. Why: §13 first run.
- `oversized_saved_window_shrinks_to_its_monitor` — a saved window larger than its monitor is shrunk. Why: after a resolution change.
- `zero_sized_saved_bounds_fall_back_to_default` — 0×0 saved bounds use the default size. Why: garbage geometry never opens an invisible window.
- `layout_sizes_match_spec` — min/default logical sizes per layout. Why: spec §13 values.
- `nonsense_scale_is_treated_as_100_percent` — NaN/0/negative scale → 1.0. Why: robustness against bad monitor info.

### External URLs and navigation (`src/url.rs`)
- `app_origins_are_allowed` — `tauri://localhost`, `http(s)://tauri.localhost` and the Vite dev server are allowed. Why: the app must load its own page.
- `everything_else_is_blocked` — any other origin is blocked. Why: the privileged webview must never navigate away.
- `accepts_plain_https` — ordinary https links (with port/path/query) pass unchanged. Why: "Get a key" links.
- `rejects_other_schemes` — http, file, javascript and others are refused. Why: §9 https only.
- `rejects_missing_host` — empty or dotted hosts are refused. Why: §9 host required.
- `rejects_whitespace_and_controls` — whitespace, controls, quotes and bidi/zero-width characters are refused. Why: argument splitting and host spoofing.
- `rejects_userinfo` — `user@host` is refused. Why: a classic spoofing vector.
- `length_limit` — more than 2048 chars is refused. Why: §9.
- `bad_ports` — non-numeric or out-of-range ports are refused. Why: strict parsing.
- `multibyte_prefix_does_not_panic` — a multibyte string shorter than the scheme is rejected without a slicing panic. Why: hostile input from the page.

### Close guard (`src/close_guard.rs`)
- `inactive_guard_always_allows` — no unsaved work → close. Why: never block a clean close.
- `first_close_cancelled_second_within_window_allowed` — first close cancelled and notified, a second within 10 s closes. Why: §13 close flow.
- `cycle_restarts_after_window_when_page_responded` — after 10 s, a responsive page gets a new cancel. Why: the guard keeps working for later closes.
- `unresponsive_page_allows_next_close_even_after_window` — no ping after the request → the next close is allowed. Why: §13 "an unresponsive page always closes".
- `a_ping_after_the_ack_window_is_not_an_ack` — a ping later than 3 s doesn't count as a response. Why: prevents a stale ping from arming the guard.
- `no_attached_page_allows` — no event channel → close. Why: nobody could show the unsaved bar.
- `clearing_the_guard_allows_and_resets` — `set_close_guard(false)` resets the state. Why: after save or discard.
- `never_uncloseable_under_repeated_clicks` — rapid repeated closes always end in `Allow`. Why: the window must never become uncloseable.

### Commands boundary (`src/commands.rs`)
- `timeout_maps_to_internal` — a command exceeding its limit resolves to `internal` with the timeout copy. Why: §9.
- `guarded_passes_values_and_errors_through` — ok and error values are enveloped unchanged. Why: the command wrapper must be transparent.
- `guarded_command_exceeding_30s_resolves_internal` — a pending command resolves to `internal` at 30 s. Why: §9 "a command exceeding 30 s resolves to internal".
- `panics_become_internal_without_the_payload` — a panic becomes `internal` and the panic text (possibly user data) is not included. Why: §14.12, nothing panics across the boundary.
- `gate_waits_while_starting_then_returns_core` — commands wait while the core starts. Why: §3 bounded wait.
- `gate_times_out_with_core_starting_copy` — after 25 s → `copy::CORE_STARTING`. Why: bounded (~25 s) wait.
- `gate_fails_immediately_after_failure` — after `core:failed`, `copy::CORE_FAILED` with zero wait. Why: §3 "after failure they return an actionable error at once".
- `gate_waiter_released_by_failure` — a waiting command is released by a failure. Why: no hang when startup fails mid-wait.

### Diagnostics (`src/diagnostics.rs`)
- `redacts_provider_keys` — `sk-…`, `gsk_…` tokens become `[redacted]`. Why: §17, diagnostics never contain secrets.
- `redacts_long_hex_and_base64_runs` — Deepgram-style hex keys and DPAPI base64 blobs are redacted. Why: a last line of defence for the free-text log lines.
- `keeps_ordinary_text` — paths, words and short ids survive. Why: diagnostics must stay useful.
- `render_includes_status_and_redacts_logs` — version, build, OS, protection, core, settings and hotkey lines are present, and log lines are redacted. Why: the §9 `get_diagnostics` contents.
- `render_caps_log_lines` — at most 200 log lines. Why: bounded clipboard payload.

### Build info (`src/build_info.rs`)
- `missing_values_are_unknown` — absent or blank env values become "unknown", not dirty. Why: builds without git must still work.
- `present_values_pass_through` — the revision, dirty flag and time are used. Why: §17 build info.
- `macro_uses_this_crates_version` — `build_info!()` embeds the calling crate's version. Why: one version source.
- `iso8601_known_values` — epoch and leap-day formatting. Why: log and build timestamps without a date crate.

## Tauri glue (`src-tauri`)

Run: `cargo test -p aicallassistant` (default `target/`).

### Unit (`src/logging.rs`, `src/platform.rs`, `src/protection.rs`)
- `tail_keeps_last_lines` — the in-memory tail holds the last 200 lines. Why: "Copy diagnostics" includes recent log lines.
- `partial_writes_join_into_one_line` — split writes form one tail line. Why: the tracing writer may flush mid-line.
- `rotates_by_size_and_keeps_three_files` — `aica.log` rotates at the size limit into `aica.1.log`/`aica.2.log`. Why: §17 rotated log.
- `panic_line_redacts_keys` — the panic-hook line redacts key-like text and truncates. Why: §14.12, no secrets in crash logs.
- `os_version_reads_a_real_version` — the OS string reads `Windows 10.x.y` (or 6.x). Why: diagnostics accuracy.
- `every_parsed_hotkey_is_accepted_by_the_plugin_parser` — every canonical key name × several modifier sets is accepted by the global-shortcut plugin's parser. Why: a combo we call valid must be registrable.
- `only_0x11_is_protected` — only a read-back of `0x11` is `Protected`. Why: §11, never claim protection unless Windows confirms it.
- `verdict_trusts_the_readback_not_the_apply_result` — a failed apply with a good read-back is protected, and a "successful" apply with a bad read-back is not. Why: the read-back is the source of truth.
- `invalid_hwnd_is_unprotected` — the real Win32 calls on an invalid HWND give `Unprotected`. Why: failure must never read as protected.

### Wiring integration (`tests/wiring.rs`, §14.1)
Builds exactly what `run()` builds, minus Tauri: the real `SettingsStore` (fake keystore), `build_core`, the real `EventPump`, the same `app::*` command functions the handlers call, and the production Channel delivery over a **real `tauri::ipc::Channel`**. Fakes stand in for audio, STT, provider and window. Real time on a multi-thread runtime, with condition waits.
- `record_flow_events_land_on_the_page_channel` — start → frames → stop produce `stt:partial`, every `llm:delta` in order, and exactly one `llm:done` on the page's channel, with increasing seq, and the subscribe published `protection:ok`. Why: §14.1, v3 shipped four commits where commands worked but no event reached the page.
- `resubscribe_mid_session_still_delivers_the_terminal_event` — a page reload mid-session (a new channel) re-adopts the session via `get_status` and still gets `llm:done`, while the old channel gets nothing new. Why: §9 re-subscribe and §14.6 recovery after reload.
- `ask_flow_and_status_revisions` — ask trims, emits the question as a final partial, answers, and bumps the status revision. An empty ask returns `EMPTY_QUESTION`. Why: §4 Ask plus the §9 revision rule.
- `core_failure_gives_actionable_errors_and_status_still_answers` — after `core:failed`, start and get_settings return `CORE_FAILED` at once, `get_status` still answers, and cancel is ok. Why: §3 failure behavior.
- `set_settings_side_effects_run_and_stale_revision_applies_nothing` — a save registers the hotkey, toggles always-on-top and switches layout (persisting the outgoing geometry). A stale save runs no side effects and changes nothing. Why: §12 serialized saves with side effects, and revisions.
- `taken_and_invalid_hotkeys_report_honestly` — taken → `unavailable` with the exact copy, `Ctrl+Banana` → invalid, empty → disabled. Why: §13 honest status.
- `late_hotkey_registration_is_undone` — a late success for the current request is adopted (and `settings:changed` emitted), and a stale one superseded by a newer request is unregistered. Why: §13 late-registration rule on the real glue.
- `protection_is_verified_with_retries_and_never_claimed_early` — scripted failures retry until protected, all-fail publishes `protection:failed` with the hub revision, and nothing claims protection early. Why: §11.
- `close_guard_cancels_once_and_notifies_the_page` — with the guard active, the first close is cancelled and emits `window:close-requested`, and the second closes. Why: §13 close flow end to end.
- `open_external_validates_before_touching_the_os` — invalid URLs are rejected before any platform call. Why: §9 URL rules.
- `dock_persists_bounds_for_the_current_layout` — dock moves the window and saves bounds for that layout. Why: each layout remembers its own position.
- `diagnostics_have_status_and_no_secrets` — diagnostics contain status lines and no key, and settings views carry no key material. Why: §17 and §14.12.
- `shutdown_is_bounded_and_idempotent` — shutdown finishes in time, can run twice, and later commands fail cleanly. Why: §13 exit must never hang.
- `commands_resolve_through_tauri_ipc` — every `#[tauri::command]` handler resolves through Tauri's mock-runtime IPC (argument decoding, camelCase, the `{ok,value}` envelope, the Channel argument). Why: a wrong argument name fails only at runtime in Tauri.

## Frontend state layer — tests

Scope: `src/ipc/{tauri,fake}.ts`, `src/state/**`, `src/markdown/**`, `src/app/{AppProvider,controller}.tsx?`, `src/App.tsx`, `src/main.tsx`.

Run: `npx vitest run src/ipc src/state src/markdown src/app`

Architecture in one line: `reducer(state, action)` is pure (every time value arrives on the action), `selectView(state, now)` derives the pinned `AppView`, and `createController()` (`src/app/controller.ts`) owns every side effect: commands with generations, event intake (seq dedupe, rAF delta coalescing), refused-stop recovery and serialized settings saves. `AppProvider` wires it to React through `useReducer`.

### src/state/reducer.test.ts — pure reducer

#### session lifecycle from events
- `start ok -> starting until session:recording -> recording with deadline` — the phase stays `starting` until capture is really running, then takes the core's deadline, with startedAt = deadline − cap. Why: §4.3, one deadline shared by the UI countdown and the core cap (§5.11).
- `stt:partial replaces the transcript with the full text` — partials carry the FULL transcript and are not appended. Why: §4.5 contract; appending would duplicate text.
- `audio:level tracks rms and the last loud time` — the level updates, and only rms ≥ 0.01 moves lastLoudAt. Why: drives the level meter and the silence hint (§13).
- `stop ok -> finalizing; first delta -> answering; llm:done -> idle + history` — the whole happy-path phase machine plus the history push with finish/callType/metrics. Why: core UX flow §4.
- `session:error -> idle; pushed to history only when there is a question or answer` — errors end the session, and an empty failure (no_speech) does not clutter history, while a failed answer to a real question stays so the user can Regenerate it. Why: history holds answered entries; regenerating a failed question is useful.
- `at most one terminal per session: a second terminal is ignored` — a late error after `llm:done` changes nothing. Why: §5.8, defence in depth on the page side.
- `session:autostopped -> finalizing + cap-reached status` — the 120 s cap shows "Reached the 120s limit — answering now". Why: §13 status copy.
- `audio:device lost -> notice + finalizing (not the 120s copy); changed -> notice only` — a device unplug auto-stops with an honest message and never claims the cap was hit; a device switch only shows a notice. Why: §18 device handling.
- `ask: starts directly in answering with the question as transcript` — a typed question skips the recording phases. Why: §4 Ask flow.

#### seq, stale sessions and buffering
- `drops envelopes with seq <= lastSeq (duplicates / replays)` — duplicated or older envelopes are no-ops. Why: the CoreApi contract says listeners must tolerate duplicates.
- `lastSeq starts at 0 on load and is never reset by snapshots or core:ready` — seq tracking resets exactly once per page load (in the initial state) and never again. Why: §14.7, the v3 double bump dropped legitimate events.
- `events for a session id that is not current are dropped (§5.3)` — stale deltas or errors from an old session can't touch the current one. Why: §5.3 invariant.
- `events arriving before the start result are buffered and replayed only if they are ours` — while the command's id is unknown, session events are buffered, then replayed for the adopted id; other ids are discarded. Why: the channel and the command reply race independently.

#### events that precede the command reply (orchestrator note)
- `ask: the question's final stt:partial emitted during install is replayed once the reply adopts the id` — the question transcript and an early delta survive the race. Why: the session crate emits the ask partial during install, before the reply.
- `record: session:recording + early partial/level before the start reply are replayed in order` — deadline, transcript, level and loud time (with the original receipt time) are all applied. Why: capture can start before `start_session` returns.
- `buffered events are dropped when the command fails` — a failed start leaves no ghost deadline. Why: a failed command must not adopt anything.
- `buffered events are dropped when the command is superseded; the newer command gets only its own` — the old command's buffer is cleared on supersede, and the new one replays only its own id. Why: §5.2.
- `a superseded session needs no terminal event: the UI has already moved on locally` — supersede and cancel move the UI on without waiting for the core (which emits nothing more for those sessions). Why: orchestrator note / session-crate behavior.

#### command generations (§14.2)
- `a late start-failed must not flip a newer session to idle` — a failure from an older command generation is ignored. Why: lesson §14.2.
- `a late start-aborted / start-ok must not null or hijack a newer session` — late results from an older generation never replace the newer session's id. Why: lesson §14.2.
- `the current command's failure shows the error and returns to idle` — the latest command's failure is surfaced. Why: the counterpart of the two tests above.
- `a stop is a command generation but does not orphan the pending start's slot` — Stop bumps cmdGen but the start result still owns its slot, and the stop is queued. Why: Stop during connect must work (§4.6).
- `a stop result for another session is ignored` — stop results are scoped by session id. Why: §5.3.
- `cancel while start is pending leaves an idle slot that ignores the late result` — a cancelled pending start is never adopted. Why: §5.2 (the provider then cancels the orphan).

#### revision adoption
- `adopts snapshots/events only when revision >= last seen` — older snapshots and protection/core events never override newer ones. Why: §9 snapshot rules; protection status must never regress.
- `re-adopts a live session from the snapshot, including its deadline` — on reload the recording session comes back with its countdown. Why: §9 and §18 ("re-adopted session keeps its countdown deadline").
- `does not re-adopt an ended session or replace a live one from a snapshot` — a stale snapshot can't resurrect a finished session or steal the slot from a live one. Why: snapshots race with events.
- `settings adopt only when settingsRevision >= current` — a stale get_settings response can't overwrite a newer save. Why: lesson §14.10, ordering by revisions.

#### history
- `caps at 6 entries, dropping the oldest` — Why: §13, last 6 answered entries.
- `prev/next navigation walks entries and returns to live` — "n/m" indices and canPrev/canNext at the edges. Why: §13 history UI.
- `clear empties history and the finished live entry` — Why: §13 Clear.
- `a new session jumps the view back to live` — Why: the user must see the new answer, not an old entry.
- `regenerate (ask with the viewed question) creates a NEW entry, never overwrites` — Why: §4 Regenerate.
- `history is view state only: entries hold no profile/settings data` — the entry shape is pinned to view fields only. Why: history is never persisted and never sent as context (§8, §13).

#### first paint + misc
- `visibleFirstWordMs is measured once from the stop click` — measured only after text exists, and the first report wins. Why: §15 frontend "visible first word" metric.
- `window:close-requested sets closeRequested; dismiss clears it` — Why: §13 close guard flow.
- `closing settings clears the dirty flag` — Why: the close guard must not stay armed after leaving Settings.
- `recovery/gone returns the scoped session to idle with a notice; other ids untouched` — Why: §14.3, recovery is scoped to its session.

### src/state/select.test.ts — selectView

- `core starting / failed` — "Starting…" while the core starts, and the core's own error text once it has failed. Why: §3 readiness reporting.
- `ready vs needsSetup` — the first-run prompt replaces "Ready…" when keys are missing. Why: §13 first-run status.
- `starting / recording elapsed / finalizing / answering` — exact §13 copy, including "Recording — 0:12" elapsed formatting. Why: the status line is spec copy.
- `no status string or copy constant ever says "microphone"` — scans every copy constant, every reachable status line and the hotkey hints. Why: §1/§13, the app captures system output audio and never the microphone.
- `silent after 5 s with no loud level, measured from start` — boundary at exactly 5000 ms. Why: §13 "No call audio detected yet".
- `a loud level (rms >= 0.01) resets the window; quiet levels do not` — the threshold is inclusive at 0.01. Why: the silence rule.
- `never silent outside recording` — Why: no false hint while connecting.
- `false until settings load` / `true without a deepgram key` / `true when the selected provider's key is missing` — needsSetup semantics. Why: first-run prompt accuracy.
- `record when core ready and idle or answering (supersedes); stop while starting/recording and not already stopping` — button enablement, including no double Stop and no Record while finalizing. Why: §13 button rules; a Record supersedes a streaming answer (§4).
- `ask needs a ready core; regenerate needs a displayed entry with a question` — Why: §13 and the Regenerate rules.
- `aborted errors are not surfaced as a session error` — user-initiated aborts aren't shown as alerts. Why: avoids noisy false errors.
- `provider name, style, layout, fonts, active profile` — derived settings fields. Why: header chip and layout rendering.
- `hotkey hint reflects the honest registration status` — registered, disabled, invalid and unavailable, preferring the core's message. Why: §12/§13 honest hotkey status.

### src/ipc/tauri.test.ts — real IPC adapter (mocks `@tauri-apps/api/core`)

- `passes the exact command names and argument objects (incl. sessionId)` — `{patch}`, `{sessionId}`, `{active}`, `{url}`, `{text}`, and none for argument-less commands. Why: a wrong arg name fails only at runtime in Tauri.
- `returns the CmdResult the Rust side sends` — ok and error envelopes pass through. Why: the Rust side returns the CmdResult JSON itself.
- `never rejects: thrown / rejected invokes map to internal` — covers rejected strings, synchronous throws and odd error objects. Why: §9, nothing throws across the boundary.
- `validates the response shape defensively` — malformed values become `internal`, and unknown error codes are coerced to `internal`. Why: the UI keys behavior off a closed code set.
- `resolves to internal after the 30 s client-side timeout` — exact boundary under fake timers. Why: §9 command timeout, so a hung core can't wedge the UI.
- `subscribe attaches a Channel via subscribe_events and unsubscribe stops forwarding` — invalid messages are dropped and an unsubscribed listener stays silent. Why: §14.1 event-channel wiring.
- `retries a failed subscribe_events attach (bounded)` — 3 attempts. Why: lesson §14.1, a silently detached event sink broke v3.
- `treats a missing value on ok as null (unit commands)` — Why: Rust `()` may serialize as null or be absent.

### src/ipc/fake.test.ts — scriptable fake core

- `emit assigns increasing seq and reaches only active listeners` — Why: tests depend on realistic seq behavior.
- `logs calls and lets tests override responses` — Why: the fake's scripting API.
- `set_settings enforces baseRevision and bumps settingsRevision` — Why: needed to test the stale-save flow realistically.
- `stop is only taken for the live session` — the second stop gets "not taken". Why: needed to test refused-stop recovery realistically.
- `demo mode plays start -> recording -> partials -> stop -> deltas -> done` — Why: `npm run dev` in a plain browser must show a working flow.

### src/markdown/markdown.test.tsx — safe streaming renderer

#### golden cases
- `paragraphs split on blank lines; soft breaks stay in one paragraph`
- `ATX headings h1..h6, closing hashes stripped; #tag is not a heading`
- `bullet lists with -, *, +`
- `numbered lists`
- `nested lists by indent`
- `indented continuation lines join the item`
- `bold, italic, inline code` — including `***both***` and nesting.
- `intraword underscores and lone stars stay literal` — snake_case and arithmetic aren't mangled.
- `code spans protect their content; backslash escapes`
- `fenced code blocks keep content verbatim with a language class`
- `an unterminated fence (streaming) renders as a code block so far`
- `links are NOT rendered: [x](y) stays literal text`
- `a list interrupts a paragraph; a heading ends a list`
- `custom className is appended to the wrapper`
- `parseInline is pure and merges adjacent text`

Each golden case pins one construct of the §13 subset exactly. Why: a regression in any construct is visible to users mid-call.

#### XSS corpus
- `renders %j with only allowed elements and class attributes` (15 inputs: `<script>`, `<img onerror>`, `javascript:` links, `<iframe>`, entities, U+202E, `<svg onload>`, `<style>`, a fence info-string injection, …) — the output contains only div/p/h1–h6/ul/ol/li/pre/code/strong/em, and the only attribute is `class`. Why: answers are untrusted LLM output shown in a privileged window.
- `dangerous text survives as visible text (not dropped, not executed)` — Why: the user must still see what the model said.
- `the source never uses innerHTML / dangerouslySetInnerHTML` — scans the renderer's own source. Why: §13 "every string a text node".

#### streaming property
- `every streamed prefix renders safely, and the concatenation renders exactly like the whole` — fast-check, 300 runs of random markdown-ish strings with any cut points. Every intermediate paint is safe and doesn't throw. Why: §13 property test.
- `a live component updated chunk-by-chunk renders the same DOM as rendering the whole at once` — fast-check, 200 runs. One React root is re-rendered at each streamed prefix, and its innerHTML must equal the `renderToStaticMarkup` of the whole. Why: the real invariant is that per-block memoization never leaves stale DOM.
- `per-block memoization: finished blocks are not re-rendered while the last block streams` — earlier blocks keep the same DOM nodes. Why: the per-block memoization requirement, and paint cost while streaming.

#### error boundary
- `falls back to plain text when rendering throws, and retries on new source` — Why: §13, a renderer bug must never blank the answer.

### src/app/AppProvider.test.tsx — provider + controller against the fake core

#### mount wiring
- `subscribes to events BEFORE calling get_status / get_settings (§14.1)` — Why: events emitted between the snapshot and the subscription would be lost.
- `re-adopts a live session from get_status on load, keeping its deadline` — events for the adopted id land, and Stop targets it. Why: §9 reload behavior.
- `StrictMode double-mount: one initial fetch, every event applied exactly once` — Why: lesson §14.7, generation bookkeeping must bump exactly once.
- `core:ready refetches settings + status; settings:changed refetches settings` — Why: §3 readiness, and settings changed elsewhere.
- `duplicate envelopes (seq <= last) are ignored, including side effects` — a duplicated `hotkey:toggle` doesn't double-toggle. Why: side effects must be deduped as well as state.

#### delta coalescing
- `buffers llm:delta and dispatches at most once per animation frame` — 3 deltas produce 1 render. Why: §13 "one paint per animation frame".
- `a non-delta event flushes buffered deltas first, preserving order` — Why: `llm:done` must never overtake buffered text.

#### hotkey + window
- `hotkey toggles record/stop on the main screen` — Why: §13 global hotkey.
- `while Settings is open the hotkey may only STOP, never start` — Why: §13, explicit rule.
- `window:close-requested sets closeRequested; the close guard follows settingsDirty` — `set_close_guard(true/false)` on each change. Why: §13 close flow.

#### commands
- `Stop clicked while start is still in flight is sent once the id arrives` — Why: §4.6, Stop during connect.
- `a start that resolves after Cancel tears down only the session it created` — Why: §5.2 and §14.2.
- `a late start result after a newer ask does not touch the newer session` — the orphan is cancelled and the newer session keeps streaming. Why: §14.2.
- `ask trims, rejects empty with a notice, and measures visible first word` — Why: §4 validate first; §15 visible first word.
- `regenerate asks with the viewed entry's question and adds a new entry` — Why: §4 Regenerate.
- `events emitted by the core BEFORE the start/ask reply are not lost (fake core emits during the call)` — Why: the orchestrator note, tested through the real controller.
- `cancel calls cancel_session for the live session` — Why: §4 Cancel.

#### settings
- `stale save: re-fetches settings, reports stale:true, and the next save uses the fresh revision` — Why: §13 stale-revision flow; lesson §14.10.
- `a non-stale failure reports the core's message with stale:false` — Why: e.g. fail-closed encryption errors must be shown verbatim.
- `quick saves are optimistic and serialized (rapid A+ A+ both land)` — each save reads baseRevision when it runs. Why: without serialization the second click is a guaranteed stale rejection.
- `font bumps clamp to the allowed ranges` — Why: §12 limits (12–22 answer font, 14–28 prompter font).
- `setCallType patches the active profile; setStyle/setLayout/setActiveProfile save` — Why: §13 call-type dropdown saves the active profile.
- `copyDiagnostics copies the text; failures return null with a notice` — Why: §17 "copy diagnostics".
- `openExternal failures surface as a notice; dock calls dock_window` — Why: user feedback on refused links.

#### refused-stop recovery (§14.3)
- `core still recording X -> retries the stop -> finalizing` — Why: the stop was refused transiently.
- `core reports unknown -> waits and asks again; X gone -> idle + cancel_session(X)` — Why: `unknown` means "wait again", and recovery ends with a safety cancel.
- `core reports X finalizing/answering -> adopts that phase, no cancel` — Why: the core is progressing, so don't kill it.
- `progress for X (a delta) disarms the recovery: no more polls, no cancel` — Why: §14.3 disarm on progress.
- `progress for ANOTHER session does not disarm it (scoped to X)` — Why: §14.3 scoping.
- `an accepted ask disarms the recovery` — Why: §14.3.
- `bounded: a core that stays unknown gives up after 5 polls -> idle + cancel_session(X)` — polling stops afterwards. Why: recovery must be bounded, and the UI must never stay stuck in "recording".
- `a newer session is never touched by an old recovery` — Why: §14.3 scoping.

#### recording clock
- `ticks while recording: elapsed timer and silence hint` — a 250 ms tick drives "Recording — 0:05" and the silence hint, and a loud level clears it. Why: §13 timer and hint; the tick runs only while recording (idle CPU ≈ 0).

### src/app/App.test.tsx — real page wiring
- `record -> events -> stop -> streamed answer lands in the DOM` — the real `<App>` (AppProvider + frontend-ui components) against the fake core: button click → command, events → status line, transcript and streamed Markdown (real rAF scheduler), done → Ready. Why: lesson §14.1, catches seam breakage that component tests with hand-built views can't.

## Frontend UI tests (`src/components/**`)

Run: `npx vitest run src/components` (156 tests across 6 files). Components get state only through `useApp()`, so every test renders inside `<AppContext.Provider>` with a hand-built `makeView()` view and `makeActions(() => vi.fn())` mocks. Helpers are in `src/components/testUtils.tsx` (`renderWithView`, `makeEntry`, `makeRecording`, `setScrollGeometry`). The Markdown renderer (`src/markdown`) is mocked where an answer is shown, so these tests stay independent of the renderer's own suite. jsdom has no layout, so scroll tests set `scrollHeight`/`clientHeight` by hand. Clocks use `vi.spyOn(Date, "now")`, with no real waits.

### common/common.test.tsx: shared building blocks
- `formatClock_renders_m_ss`: ms is formatted as m:ss and negative values clamp to 0. Why: the recording timer must never show garbage.
- `countdownSeconds_only_in_last_30s`: the countdown is null above 30 s and at or after the deadline, and rounds up inside the window. Why: §13 says the countdown shows only in the last 30 s, driven by the core's deadline.
- `call_type_labels_match_spec`: all six call-type labels, in order. Why: §8 labels are user-visible copy.
- `profile_field_labels_switch_for_sales_and_meeting`: Resume/Job description become Background/Call context for sales and meeting. Why: §8 UI labels.
- `protection_badge_protected_is_calm`: shows "Hidden from screen capture" with no alert role. Why: §11 verdict copy.
- `protection_badge_unknown_never_claims_protection`: unknown shows "not confirmed yet" and never the protected text. Why: never claim protection before Windows confirms it (the moat).
- `protection_badge_unprotected_is_alert`: unprotected is a red role=alert. Why: §11 says the user must be warned loudly.
- `record_button_idle_records`: idle shows "Record" and calls record(). Why: the core flow.
- `record_button_disabled_unless_canRecord`: Record is disabled when canRecord is false. Why: the view decides when recording is allowed.
- `record_button_disabled_when_core_not_ready`: Record is disabled while the core is starting. Why: §3 readiness.
- `record_button_recording_stops`: while recording the button is "Stop & Answer" and calls stop() only. Why: toggling semantics.
- `record_button_starting_shows_stop`: Stop is available while the session is still starting. Why: §4.6 lets Stop land during connect.
- `record_button_finalizing_aria_disabled_keeps_focus`: the same element becomes "Finalizing…" with aria-disabled and no disabled attribute, keeps focus, and ignores clicks. Why: §13 says focus must be kept while finalizing.
- `record_button_answering_allows_new_record`: Record works while an answer streams. Why: a new start supersedes (§5.1).
- `style_chips_radio_semantics`: radiogroup with aria-checked on the current style. Why: accessibility of the global style switch.
- `style_chip_click_sets_style`: a click calls setStyle. Why: wiring.
- `style_chips_arrow_keys_move_selection`: arrow keys cycle the style. Why: expected keyboard behavior for a radio group.
- `level_meter_width_follows_level`: role=meter and aria-valuenow/width follow `recording.level`. Why: live level feedback.
- `level_meter_hidden_when_not_recording`: no meter outside recording. Why: less noise.
- `countdown_hidden_before_last_30s`: no countdown with 60 s left. Why: §13.
- `countdown_shown_in_last_30s`: shows "Auto-stop in 12 s" computed from deadlineMs vs Date.now(). Why: the single-deadline invariant (§5.11).
- `silence_note_only_when_silent`: the exact "No call audio detected yet …" copy appears only when `silent`. Why: §13 silence guidance.
- `recording_timer_shows_elapsed`: elapsed time comes from startedAtMs. Why: the status line timer.
- `history_hidden_without_entries`: no history bar at count 0. Why: less noise.
- `history_arrows_position_tag_and_clear`: shows "n/m" and the call-type tag, and the prev/next/Clear actions are wired. Why: §13 history.
- `history_clear_available_from_one_entry`: Clear is enabled at count 1 and the arrows are disabled. Why: §13 "Clear available from one entry".
- `status_dot_tone_by_core_and_phase`: the dot color is mapped from core and phase, with an accessible label. Why: the header status dot.
- `copy_uses_async_clipboard`: the Clipboard API is tried first. Why: Copy.
- `copy_falls_back_to_textarea_when_clipboard_rejects`: when the Clipboard API is denied, a hidden textarea plus execCommand copies the exact text and is then removed. Why: §13 clipboard fallback (WebView2 can deny the API).
- `copy_reports_failure_when_everything_fails`: returns false when no method works. Why: the UI must be able to report "Copy failed".

### full/full.test.tsx: full layout
- `header_chip_and_buttons`: the chip reads "<provider> · <profile>", and the prompter/dock/settings buttons call their actions. Why: §13 header.
- `icon_buttons_have_title_and_label`: icon buttons have both an accessible name and a title. Why: accessibility of the glyph-only buttons.
- `status_line_is_polite_live_region`: statusText is rendered in an aria-live=polite region. Why: §13 accessibility.
- `status_line_needs_setup_opens_settings`: first run shows an "Open Settings" button. Why: the §13 first-run prompt.
- `status_line_shows_timer_while_recording`: the timer appears in the status line. Why: §13.
- `answer_renders_markdown_at_font_size`: the answer goes through Markdown at answerFontPx. Why: A−/A+ sizing.
- `answer_empty_state_hint`: a hint shows and Copy is disabled when there is no entry. Why: empty state.
- `answer_font_buttons_bump`: A−/A+ call bumpAnswerFont(-1/+1). Why: wiring.
- `regenerate_follows_canRegenerate`: Regenerate is enabled only when allowed. Why: §4 regenerate.
- `latency_chip_and_tooltip`: "first word in X ms" plus a tooltip with all four metrics and the visible first word. Why: §13/§15 never hide latency.
- `latency_tooltip_without_visible_measure`: a missing visible measure shows "—". Why: honest display.
- `latency_chip_absent_without_metrics`: no chip while streaming. Why: metrics arrive only with llm:done.
- `truncated_note` / `refused_note` / `no_note_when_complete`: the finish notes and their exact copy. Why: §13.
- `copy_copies_markdown_source_and_shows_feedback`: the raw markdown source is copied and "Copied" is shown. Why: §13 says Copy takes the source.
- `copy_fallback_when_clipboard_missing`: the textarea fallback path works from the panel. Why: §13.
- `answer_region_aria_busy_while_streaming`: the answer region is polite-live and aria-busy while streaming. Why: don't re-announce every delta.
- `auto_scroll_sticks_then_stops_on_user_scroll_up`: the panel follows the bottom while streaming, stops after the user scrolls up, and resumes at the bottom. Why: §13 sticky auto-scroll.
- `report_first_paint_once_per_stream`: reportFirstPaint fires once, on the first non-empty streaming text, and re-arms for the next session. Why: §15 visible-first-word metric.
- `no_first_paint_for_finished_history_entry`: finished entries never report. Why: avoids bogus measurements.
- `session_error_alert_with_settings_button`: the error is role=alert and offers "Open Settings" for key errors. Why: actionable errors.
- `session_error_without_settings_button_for_other_codes`: no Settings button for e.g. no_speech. Why: don't send users to the wrong place.
- `session_error_llm_auth_and_no_stt_key_offer_settings`: the other settings-fixable codes. Why: coverage of the code set.
- `question_shows_live_transcript_while_live` / `question_shows_entry_question_when_idle`: the source of the QUESTION HEARD text. Why: §13.
- `ask_enter_submits_trimmed_and_clears`: Enter submits trimmed text and clears the input. Why: §13 Ask.
- `ask_button_submits`: the Ask button path. Why: wiring.
- `ask_blank_does_nothing`: whitespace never calls ask. Why: validate first (§4 Ask).
- `ask_disabled_unless_canAsk`: Ask is blocked when not allowed. Why: view gating.
- `profile_dropdown_hidden_with_one_profile` / `profile_dropdown_shown_with_two_profiles`: the profile dropdown shows only with more than one profile and calls setActiveProfile. Why: §13.
- `call_type_change_calls_setCallType`: 6 options, and a change calls setCallType. Why: §13.
- `notice_dismissable` / `notice_absent_when_null`: the notice bar. Why: device/notice messages.
- `full_layout_answer_panel_comes_before_question_and_controls`: DOM order is answer, then question, then record. Why: §13 answer first, at camera level.
- `full_layout_hotkey_hint`: the hotkey hint (or why it's unavailable) renders. Why: §13.
- `full_layout_core_starting_disables_controls`: "Starting…" shows and the controls are disabled. Why: §3 readiness.
- `full_layout_core_failed_alert`: an alert with coreError.message. Why: §3 failure is actionable.
- `full_layout_recording_shows_meter_and_silence`: the meter and silence note show while recording. Why: §13.
- `full_layout_countdown_only_in_last_30s`: the countdown appears only after crossing 30 s. Why: §13/§5.11.

### prompter/prompter.test.tsx: prompter strip
- `prompter_shows_answer_at_prompter_font`: large text at prompterFontPx. Why: §13 prompter.
- `prompter_escape_exits_to_full`: Esc calls setLayout("full"). Why: §13.
- `prompter_exit_and_redock_buttons`: the ⤒ re-dock and ⤢ exit buttons. Why: §13.
- `prompter_font_buttons`: bumpPrompterFont(-1/+1). Why: §13.
- `prompter_does_not_auto_scroll_while_streaming`: scrollTop stays 0 while text grows. Why: §13 says the text must not jump.
- `prompter_more_hint_when_overflowing`: "▼ more" appears on overflow, scrolls when clicked, and hides at the bottom. Why: §13.
- `prompter_status_line_includes_finalizing`: the status line shows finalizing and the button is aria-disabled. Why: §13.
- `prompter_record_button_and_timer`: the compact record/stop button and timer. Why: §13.
- `prompter_one_line_question_live_or_entry`: the one-line question has a title tooltip. Why: §13.
- `prompter_has_style_chips_and_history`: the compact style chips and history arrows. Why: §13.
- `prompter_reports_first_paint`: first-paint reporting also works in the prompter. Why: §15 in either layout.
- `prompter_truncated_note` / `prompter_session_error_alert`: notes and errors are still visible in the strip. Why: no silent failures.

### Root.test.tsx: layout selection and cross-view guarantees
- `root_picks_full_layout` / `root_picks_prompter_layout` / `root_settings_screen_wins_over_layout`: view.screen/layout pick the screen. Why: Root contract.
- `protection_badge_<state>_in_<view>_view` (9 cases): exactly one badge with the right copy in full, prompter and settings for protected/unknown/unprotected, and the protected text never appears otherwise. Why: §11 says every view shows the verdict and never over-claims.
- `no_view_ever_says_microphone`: renders 25 states across all three views, and the HTML never contains "microphone" or "mic". Why: §1/§13 (system audio only, never the microphone).

### settings/draft.test.ts: pure patch/draft logic
- `patch_empty_for_untouched_draft`: an untouched draft gives an empty patch and is not dirty. Why: no spurious saves or close-guard prompts.
- `patch_contains_only_changed_fields`: only changed fields go in the patch. Why: brief requirement, and it limits stale conflicts.
- `patch_replaces_whole_profile_list_when_any_profile_changes`: `profiles` is sent whole. Why: the contract says profiles replaces the list.
- `patch_draft_is_not_aliased_to_settings`: editing the draft never mutates the view. Why: immutability of store state.
- `patch_hotkey_trimmed`: whitespace-only differences are not changes. Why: no phantom dirtiness.
- `secret_typed_value_sets_trimmed` / `secret_queued_remove_removes` / `secret_typed_value_wins_over_remove` / `secret_emptied_field_never_removes`: the secret semantics. Why: §12 write-only secrets rules.
- `new_profile_id_matches_contract_regex_and_is_unique` / `new_profile_id_avoids_collisions_with_constant_rng`: ids look like `p-<base36>`, match the regex and never collide. Why: §12 id schema.
- `duplicate_name_trimmed_to_60`: "<name> copy" is capped at 60. Why: §13.
- `hotkey_valid_combos` / `hotkey_needs_a_modifier` / `hotkey_needs_exactly_one_key` / `hotkey_rejects_empty_parts_and_overlong`: client-side shortcut validation. Why: §13 "any Ctrl/Alt/Shift/Win + key, validated".

### settings/settings.test.tsx: Settings screen
- `key_rows_placeholder_and_status`: the "saved — type to replace" placeholder, password inputs and storage text. Why: §13 keys.
- `key_row_unreadable_status`: "stored key couldn't be read — enter it again", with Remove offered. Why: §12 undecryptable reads as unset.
- `key_disclosure_text`: the DPAPI/plain-text disclosure copy. Why: §12/§13.
- `get_a_key_opens_external`: "Get a key" calls openExternal(getKeyUrl). Why: wiring.
- `key_remove_queues_remove_in_patch`: Remove queues `{keyId, action:"remove"}`. Why: §12 explicit Remove.
- `key_typing_cancels_queued_remove`: typing turns a queued remove into a set. Why: §12.
- `key_emptying_field_does_not_remove`: type then clear gives no patch and is not dirty. Why: §12 "emptying the field does not remove".
- `key_undo_remove`: a queued remove can be undone. Why: recoverable UI.
- `key_value_cleared_after_successful_save`: key material leaves the DOM after saving. Why: write-only secrets.
- `labels_switch_for_sales_and_meeting`: the field labels switch live in the editor. Why: §8/§13.
- `focus_counter_counts`: the focus counter and the maxLength limits. Why: §12 limits.
- `add_profile_appends_and_selects`: Add creates a valid new profile and selects it. Why: §13.
- `duplicate_profile_names_copy`: Duplicate copies the fields, gets a new id and a 60-char name. Why: §13.
- `delete_disabled_for_last_profile`: at least 1 profile always. Why: §12.
- `delete_active_profile_moves_active_to_first`: deleting the active profile repoints activeProfileId. Why: never leave a dangling active id.
- `add_and_duplicate_disabled_at_20`: the limit of 20 profiles. Why: §12/§13.
- `use_for_answers_sets_active_profile`: switching the active profile from the editor. Why: convenience; the patch has only activeProfileId.
- `empty_profile_name_blocks_save`: an empty name blocks Save with a reason. Why: validation before save.
- `provider_and_style_patch`: the provider/style/keep-on-top changes give exactly those fields. Why: patch correctness.
- `hotkey_validation_hint_blocks_save`: the hint, aria-invalid, and Save disabled until valid. Why: §13 validated shortcut.
- `hotkey_status_text_is_honest`: registered/disabled/invalid/unavailable copy plus the core's message. Why: §13 honest status.
- `version_chip_and_single_turn_note`: `v<version> · <rev>` and the single-turn note. Why: §13/§8.
- `copy_diagnostics` / `copy_diagnostics_failure_message`: the diagnostics action is copied, with feedback. Why: §17.
- `load_issue_banner`: the message and backup path, with no alert when writes are allowed. Why: §12 surface corruption.
- `load_issue_writes_blocked_alert_and_save_disabled`: an alert and Save blocked when the backup failed. Why: §12 refuse to write.
- `saving_disables_form_and_shows_saving`: the fieldset is disabled and the button reads "Saving…" until the save resolves, then "Saved". Why: §13 serialized saves.
- `stale_rejection_keeps_draft_and_explains`: the stale message shows and the draft survives the reload with a new revision. Why: §13 stale-revision rule.
- `other_failure_shows_message`: the non-stale failure message is shown as an alert. Why: actionable errors.
- `settings_change_adopted_when_clean`: a clean form picks up external changes. Why: no stale UI.
- `dirty_reported_on_edit_and_cleared_after_save`: setSettingsDirty tracks edits and clears once the saved settings come back. Why: drives the §13 close guard.
- `back_when_clean_closes`: Back with no changes closes Settings. Why: no needless prompts.
- `back_when_dirty_shows_bar_keep_editing_keeps_draft`: the bar appears with focus moved into it, and Keep editing keeps the draft. Why: §13.
- `escape_when_dirty_shows_bar_discard_throws_away`: Escape opens the bar, and Discard closes without saving and clears dirty. Why: §13 "only Discard throws the draft away".
- `escape_when_clean_closes`: Escape works as Back. Why: §13.
- `save_and_go_back_saves_then_closes`: save, then closeSettings. Why: §13.
- `save_and_go_back_stale_stays_with_draft`: a stale save keeps the user in Settings with the draft. Why: nothing is lost.
- `close_requested_opens_bar_and_dismisses_when_resolved`: closeRequested opens the bar, and resolving calls dismissCloseRequest. Why: §13 close guard.
- `close_requested_when_clean_is_dismissed`: with nothing to save, the request is acknowledged at once. Why: the window must never get stuck.
- `session_status_bar_with_stop`: an active session shows its status and Stop & Answer in Settings. Why: §13.
- `session_status_bar_finalizing_has_no_stop`: no Stop once finalizing. Why: canStop gating.
- `no_session_bar_when_idle`: no bar when idle. Why: less noise.
- `settings_loading_state`: a null settings view shows loading, and the badge still shows. Why: §11 badge in every view.

## Latency benchmark

`crates/session/examples/latency_bench.rs` is a harness, not a test (spec §15). Run it with:

```bash
export CARGO_TARGET_DIR=target-core
cargo run -p callcore-session --example latency_bench --release
# flags: --cycles N --drain MS --finalize MS --first-token MS --delta MS --deltas N --record MS --tolerance MS
```

- It spawns a **real** `SessionHandle` with inline fakes: `BenchAudio` (drain takes `--drain` ms, then delivers a final partial frame), `BenchStt` (`Flushed` arrives `--finalize` ms after CloseStream) and `BenchProvider` (first delta after `--first-token` ms, then `--deltas` deltas every `--delta` ms). The default is 20 cycles of record (300 ms) → stop → answer on the **real** clock.
- It prints p50/p90/min/max for `audioDrainMs`, `sttFinalizeMs`, `firstTokenMs` and `totalMs` from each `llm:done`, plus the provider fake's **measured** wait before its first delta.
- **Check 1 (pass/fail, the exit code):** the p50 of `firstTokenMs − audioDrainMs − sttFinalizeMs − provider wait` (the session's own overhead on the Stop → first-token path) must be within `--tolerance` (default ±15 ms). This is the end-to-end form of the §9 rule that the latency clock starts at Stop acceptance, before the drain. If the clock started after the drain, the overhead would be about −`audioDrainMs`. Because it subtracts what the fakes *actually* waited, the check holds on a busy machine too.
- **Check 2 (warning only):** p50 `firstTokenMs` against the *configured* drain + finalize + first-token (±`--tolerance`). It passes on an idle machine. When other builds load the CPU, the fakes oversleep their injected delays and it prints `WARN`.
- The exit code is 1 on any failed cycle or a failed check 1.
- Windows' default timer tick is about 15.6 ms, so a plain `sleep(20 ms)` lasts about 31 ms. The fakes use a precise sleep (a coarse sleep to within 20 ms, then a yield-spin) so the injected delays are as exact as the scheduler allows.
- Example output (Windows 11, release build). First on a quiet machine, then while other agents' cargo builds were running:

```
latency_bench: 20 cycles | injected drain 40 ms, finalize 150 ms, first token 300 ms, 10 deltas x 20 ms

metric (ms)          p50     p90     min     max
audioDrainMs          40      69      40      81
sttFinalizeMs        150     156     150     186
firstTokenMs         490     534     490     551
totalMs              689     842     670     881
```

```
metric (ms)          p50     p90     min     max
audioDrainMs          40      77      40     143
sttFinalizeMs        155     186     150     216
firstTokenMs         531     616     490     673
totalMs              763     915     670    1014
(provider wait)      305     368     300     428

check 1: p50 session overhead (firstTokenMs - drain - finalize - provider wait) = 1 ms (tolerance ±15 ms) -> PASS
check 2: p50 firstTokenMs 531 vs injected drain+finalize+first-token 490 (diff 41 ms, tolerance ±15 ms) -> WARN (the fakes overslept their injected delays - machine under load; check 1 is authoritative)
```
