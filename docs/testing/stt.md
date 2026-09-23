# callcore-stt tests (Deepgram streaming client)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-stt`

Socket tests (`crates/stt/tests/deepgram.rs`) run against a loopback fake
Deepgram (`crates/stt/tests/support/mod.rs`, tokio-tungstenite
`accept_hdr_async` on `127.0.0.1:0`). They use real time; every wait is a
condition wrapped in a 5 s `tokio::time::timeout`, never a fixed sleep. The
one bounded absence check (nothing sent after CloseStream) waits 400 ms =
8 keepalive intervals and is an assertion, not synchronization.

## Behaviour notes (documented decisions)

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

## Unit tests — accumulator (`src/accumulator.rs`)

- `interim_is_replaced_by_next_interim` — a new interim replaces the old one, finals untouched — Deepgram resends the whole in-progress segment; appending would duplicate words.
- `final_commits_and_clears_interim` — a final is appended to committed text and clears the interim; later interims append after it — core full-transcript rule (spec §4.5).
- `empty_finals_are_skipped_but_clear_interim` — empty/whitespace finals add nothing but still clear the interim, no double spaces — Deepgram sends empty finals for silence.
- `empty_final_first_emits_nothing_new_after_it` — repeated empty finals/interims at start emit once then nothing — avoids spamming the page with no-op events.
- `whitespace_is_trimmed_and_joined_by_single_space` — segments are trimmed and joined with exactly one space — transcript is fed byte-stable into the prompt.
- `identical_interim_resend_is_not_re_emitted` — same interim (even with padding) is not re-emitted — events only on change.
- `final_with_same_text_as_interim_flips_is_final` — interim "hello" then final "hello" emits with `is_final=true` — the UI must learn the text became final.
- `empty_interim_after_final_emits_nothing` — `(text,is_final)` unchanged → no event — no duplicate final events.
- `full_text_keeps_trailing_interim_for_flush` — `full_text` includes an unreplaced trailing interim — documents what `Flushed` carries.

## Unit tests — parsing / classification (`src/parse.rs`)

- `parses_a_real_results_frame` — a realistic Results frame yields transcript + is_final — happy path of the strict parser.
- `ignores_non_results_types` — Metadata/SpeechStarted/UtteranceEnd/unknown/missing type → ignored — only Results carry transcript.
- `ignores_bad_json_wrong_types_and_missing_fields` — bad JSON, `{"channel":5}`, wrong-typed/missing/null fields, empty alternatives → ignored — frames are hostile input (spec §6), must never panic.
- `ignores_oversized_frames_even_if_valid` — > 1 MiB frame ignored without parsing — bounds CPU/memory per frame.
- `deeply_nested_json_does_not_overflow_the_stack` — 100k-deep arrays/objects → ignored — serde recursion limit protects the reader task.
- `sanitize_reason_strips_controls_and_caps_length` — control chars removed, whitespace collapsed, ≤200 chars, multibyte-safe — server text is quoted to the user.
- `classify_close_codes` — 1000/none after close → Flushed; 1000 before → unexpected end; 1008/4001/4003/4008 → BadKey with code+reason; 1011 → Server — close classification table (spec §6, §10).

## Unit tests — connector (`src/lib.rs`)

- `production_url_has_no_endpointing_or_no_delay_and_no_key` — prod URL is the spec URL, no endpointing/no_delay/token — spec §6 forbids those params; key never in URL.
- `production_keepalive_is_the_config_constant` — default keepalive is `STT_KEEPALIVE_INTERVAL` (8 s) — the test hook must not leak into production.
- `auth_header_is_token_subprotocol_and_sensitive` — header is `token, <key>` and marked sensitive (Debug redacted) — auth mechanism + keep key out of Debug.
- `auth_header_rejects_control_chars_without_echoing_key` — key with a newline → BadKey whose message/Debug omit the key — header injection guard without leaking.
- `ws_config_caps_incoming_messages_above_the_parse_cap` — tungstenite max message size is set and above the 1 MiB parse cap — memory bound + oversize frames are ignored rather than fatal.
- `rustls_has_a_crypto_provider_for_wss` — `rustls::ClientConfig::builder()` does not panic — tokio-tungstenite panics on wss if no crypto provider is compiled in.

## Socket tests (`tests/deepgram.rs`)

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
