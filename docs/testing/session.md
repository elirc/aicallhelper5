# Session actor tests (`crates/session`)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-session` (71 tests, deterministic; run 3x in a row green).

## How the suite works

- **Fakes, no I/O.** `tests/support/mod.rs` has a scriptable fake for every port: `FakeAudio` (start delay/failure, frames pushed on demand, drain delay + tail frames, records `Start`/`Discard`/`Drain` calls, keeps every sink so a test can post a "late callback"), `FakeStt` (connect delay/failure, logs frames / `Close` / `Abort` / `Dropped` per connection in order, auto or manual `Flushed`, optional late `Failed`, send failure, stream end), `FakeProvider` (scripted `Sleep`/`Delta`/`Return`/`Hang` steps, counts builds and stream calls, captures request bytes, prompts, keys and live stream futures), `FakeSettings` (keys present/absent/blank, read error, a gate that blocks one `get_secret` on its blocking thread), `RecordingSink` (collects `CoreEvent`s, `wait_for(pred)` with a bounded timeout, no fixed sleeps), `FakeClock` (epoch ms driven by tokio's clock).
- **Paused time.** Nearly every test is `#[tokio::test(start_paused = true)]`. All fake delays use `tokio::time`, so timers (cap 120 s, connect 5 s, finalize 5 s, first token 10 s, total 60 s) are exact and instant. No sockets are involved, so paused time is safe. One test runs on real time with a 4-thread runtime to prove nothing depends on current-thread scheduling.
- **Leak checks.** A test counts a task as gone when its resource is released: the audio sink's receiver is closed (relay task gone), the STT event receiver is closed (reader gone), the fake sender logged `Dropped` (writer gone, `abort()` called), the delta receiver is closed and `live_streams()==0` (provider future dropped). Timers are deadlines stored in the session state, not tasks, so dropping the session clears them. Tests also let 300–600 virtual seconds pass after an exit and assert that no event appears.
- **Mutation check (done once by hand).** Each of these changes to the actor made at least one test fail: dropping the ticket re-check, draining before the socket opens, retrying without the attempt limit or after a delta, starting the latency clock after the drain, not dropping the oldest buffered frame, emitting levels after Stop, not discarding on teardown, not auto-stopping on device loss, and removing both invariant-5 defenses (the `Done` guard and the reader abort).

## Design choices the tests pin down

- A superseded or cancelled session emits **nothing** further, not even a terminal event. Shutdown behaves the same way.
- Device-open failure ends the session at once with `internal` + `copy::DEVICE_OPEN`, so an early Stop can never turn it into `no_speech` (§14.11).
- Device loss emits `audio:device{lost}` and then takes the stop path, with no `session:autostopped` (that event is only for the 120 s cap).
- The first-token (10 s) and total (60 s) watchdogs start when the first provider attempt starts. A retry stays inside the same budget.
- Profile, call type and style are re-read when the answer starts, so a style chip flipped mid-recording applies. The provider and its key are the ones read at Record time.
- All audio calls go through one lane task, so a superseded session's `stop_discard` always reaches the device before the next `start`.

## Unit tests (`src/errors.rs`)

- `provider_mapping_is_the_closed_set` — checks every `ProviderFailureKind` against its code (Auth→llm_auth, RateLimit→llm_rate_limit, Aborted→aborted, rest→llm_http) and that the provider's message is kept — why: spec §10 mapping is the contract the UI keys behavior on.
- `provider_mapping_fills_empty_messages` — checks that an empty failure message gets actionable fallback copy — why: the user must never see a blank error.
- `stt_mapping` — checks BadKey→stt_connect keeping "1008", Connect→canonical connect copy, Server/Closed→stt_error — why: §10 STT mapping.
- `audio_mapping` — checks that device errors map to `internal` + `copy::DEVICE_OPEN` — why: §10 says device-open uses internal with the canonical copy.

## Flows (`tests/flows.rs`)

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

## Invariants and lessons (`tests/invariants.rs`)

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

## Answer streaming (`tests/answer.rs`)

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

## Metrics (`tests/metrics.rs`)

- `metrics_are_exact_and_measured_from_stop_acceptance` — checks exact audioDrain/sttFinalize/firstToken/total for known delays after a 30 s recording — why: §9 definitions.
- `injected_drain_delay_increases_first_token_ms_by_exactly_d` — checks that for D in {1, 250, 1234, 1999} ms, firstTokenMs and totalMs grow by exactly D and audioDrainMs == D — why: the explicit §9 test ("never hide latency", lesson 5).
- `recording_length_does_not_count_toward_latency` — checks that a 10 ms recording and a 100 s recording give identical metrics — why: the clock starts at Stop acceptance, not at Record.
- `total_includes_streaming_after_the_first_token` — checks that ask metrics give first=200 and total=900 with drain/finalize 0 — why: total covers the whole stream.

## Lifecycle (`tests/lifecycle.rs`)

- `shutdown_discards_the_live_session_and_later_calls_fail` — checks that shutdown discard-stops audio (done before it returns), aborts the socket, emits nothing, and later start/ask/stop return internal `SHUTTING_DOWN` while cancel and shutdown stay harmless and status stays readable — why: the exit path must be bounded and clean (§13 Close).
- `shutdown_while_answering_drops_the_provider_stream` — checks that shutdown drops an in-flight provider stream — why: no work may outlive exit.
- `shutdown_with_nothing_live_is_quick_and_idempotent` — checks a double shutdown with no session — why: idempotent exit.
- `dropping_every_handle_stops_the_actor_and_discards` — checks that the actor keeps running while any clone lives and discard-stops when the last one drops — why: no orphaned capture if the shell drops the handle.
- `stop_of_unknown_or_ask_session_is_not_taken` — checks that Stop for an unknown id, for an ask session, and a second Stop are refused, with a single drain — why: §4.6 "accepted only for the live, not-yet-stopping session".
- `real_time_multi_thread_smoke` — checks 5 real-time record/stop/done rounds plus a 20-start supersede storm on a 4-thread runtime — why: proves correctness doesn't depend on paused time or current-thread scheduling.
