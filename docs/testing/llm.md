# callcore-llm tests

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-llm`.
No test touches the network: provider tests run against a scripted HTTP/1.1
loopback server (`crates/llm/tests/support/mod.rs`) on 127.0.0.1 that records
each request and replays responses with controllable framing (chunked,
Content-Length, close-delimited), split writes (hostile 1–7 byte cuts that land
mid-line, mid-JSON and mid-UTF-8), and truncated bodies. Real time throughout
(no paused clock with sockets); every wait is on a condition with a generous bound.

## SSE parser (`src/sse.rs`)
- `parses_fields_multiline_comments_and_endings` — `event:`, `data:` with/without space, multi-line data joined by `\n`, comments, `\n`/`\r\n`/`\r`, BOM, id/retry/unknown fields, unterminated final line — the parser must cover the whole EventSource grammar providers actually emit.
- `event_without_data_is_not_dispatched_and_event_name_resets` — an `event:` with no data dispatches nothing and does not leak its name into the next event — per the SSE spec; avoids mislabelled events.
- `crlf_split_between_cr_and_lf_is_one_line_end` — a chunk ending in `\r` followed by one starting with `\n` is ONE line end — otherwise a phantom blank line would dispatch events early.
- `every_single_split_point_yields_identical_events` — for 5 real-looking transcripts, splitting at every byte position gives the same events as parsing whole — the core guarantee: transport chunking must never change what the user sees (spec §7/§16).
- `every_pair_of_split_points_yields_identical_events` — exhaustive two-cut splits for the shorter transcripts — catches state bugs that need two boundaries (e.g. a CRLF and a UTF-8 char both split).
- `random_multi_splits_and_byte_by_byte_yield_identical_events` — byte-by-byte feeding plus 500 random multi-splits per transcript (deterministic xorshift PRNG) — property test for arbitrary chunking.
- `invalid_utf8_is_replaced_not_panicking` — invalid bytes become U+FFFD — a hostile/broken server must not crash the stream.

## Failure copy + mapping (`src/failure.rs`, `src/http.rs`)
- `app_error_mapping_matches_spec_section_10` — every `ProviderFailureKind` maps to the §10 code (Auth→llm_auth, RateLimit→llm_rate_limit, Aborted→aborted, rest→llm_http) and keeps its message — the UI keys behaviour off the code.
- `snippet_prefers_json_error_message` — `error.message`, then `message`, then raw text; whitespace collapsed — users see the provider's actual reason, not a JSON blob.
- `snippet_redacts_key_strips_control_chars_and_truncates` — key replaced by `***`, control chars removed, ≤200 chars, key at the truncation boundary still redacted — spec §14.12 (keys never in errors) and the 200-char snippet rule.
- `secrets_of_extracts_bearer_token_and_raw_value` — redaction covers both `Bearer <key>` and the bare key — servers echo either form.
- `map_status_kinds_and_copy` — 403→Auth, 529→RateLimit with snippet, 404→ModelUnavailable, 502→Http with exact copy — shared status mapping is user copy with the status quoted.

## Pre-warm (`src/http.rs`)
- `throttle_allows_one_fire_per_two_seconds` — at most one pre-warm per 2 s (synthetic `Instant`s, no sleeping) — Record/Ask/Stop all pre-warm; the throttle stops request storms.
- `prewarm_without_runtime_is_a_noop` — calling `prewarm` outside a tokio runtime neither panics nor consumes the throttle — the shell may call it from a non-runtime thread.

## Registry (`src/lib.rs`)
- `registry_lists_both_providers_with_anthropic_default` — `infos()` exact `{id, displayName, keyId, model}` for both providers, default = anthropic, `get` by id, unknown id → None — the settings view and session lookup depend on these exact values.

## Anthropic conformance (`tests/anthropic.rs`)
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

## Groq conformance (`tests/groq.rs`)
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
