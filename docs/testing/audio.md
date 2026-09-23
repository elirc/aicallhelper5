# Audio (`callcore-audio`) tests

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-audio`.
No test opens a real audio device. Worker tests run `LoopbackSource` over the
scriptable `fake::FakeBackend` (the same seam other crates can reuse through
`LoopbackSource::with_backend`). Waits are bounded waits for a condition or a
message, never fixed sleeps. The one paused-clock test uses no sockets.
`examples/capture_probe.rs` (full worker + DSP path) and `examples/raw_probe.rs`
(the WASAPI backend alone, device rate) are for manual QA on real hardware only
and are not run by the tests. The WASAPI backend's COM code is exercised only by
those probes; its pure pieces (`src/wasapi_format.rs`) are unit-tested.

## DSP (`src/dsp.rs`, unit tests)

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

## WASAPI format + gap filling (`src/wasapi_format.rs`, unit tests)

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

## WASAPI backend (`src/wasapi.rs`, unit tests, Windows only, no device)

- `subtype_guids_match_ksmedia` — the local `KSDATAFORMAT_SUBTYPE_PCM` / `_IEEE_FLOAT` GUIDs equal the ksmedia.h values — a typo would reject every real device.
- `read_mix_format_parses_extensible_float` — a packed `WAVEFORMATEXTENSIBLE` (48 kHz stereo float, then 24-bit PCM) is read unaligned and classified correctly — covers the raw-pointer parsing of `GetMixFormat`'s result.
- `device_lost_codes` — `AUDCLNT_E_DEVICE_INVALIDATED` and `AUDCLNT_E_SERVICE_NOT_RUNNING` count as device loss, other HRESULTs do not — only a real loss may end capture with `DeviceLost`.

## Manual hardware QA (not automated)

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

## Worker / `LoopbackSource` (`tests/worker.rs`, fake backend)

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
