# Shell tests (`crates/shell` + `src-tauri`)

Run:

    export CARGO_TARGET_DIR=target-core; cargo test -p callcore-shell     # pure logic, fast
    cargo test -p aicallassistant                                         # glue + §14.1 wiring (default target/)

No test touches the network, a provider, an audio device or a real window's
screen-capture state. Timing tests wait for conditions (condvars, polling with
an overall deadline) or use `tokio::time::pause`; none sleeps a fixed time to
"let things settle".

Windows note: `src-tauri/build.rs` embeds `windows-app-manifest.xml` with the
linker for every target. Without that, the `tauri::test` integration binary
dies at load with `STATUS_ENTRYPOINT_NOT_FOUND` (comctl32 v6 imports), because
tauri-build's default only embeds the manifest into the app binary.

## callcore-shell — events (`EventPump`)

- `delivers_in_order_with_increasing_seq` — 200 deltas + done arrive in emit order, seq starts at 1 and strictly increases — the core ordering contract of §9.
- `buffers_while_detached_and_flushes_on_attach` — events emitted before any page subscribed are kept and flushed on attach — `core:ready` fires before the page subscribes.
- `seq_is_monotonic_across_attach_and_detach` — seq never resets across detach/re-attach — the page drops `seq <= last seen`, so a reset would silently drop events.
- `levels_coalesce_latest_wins_per_session` — 50 queued levels of one session collapse to the latest; other sessions keep their own — §9 "audio levels coalesce latest-wins".
- `must_deliver_and_deltas_survive_saturation` — with a stalled renderer and a tiny queue bound, every delta, every protection event and the terminal event arrive in order; only levels are dropped — §14.6.
- `delivery_failure_detaches_and_keeps_the_event` — a failing Channel detaches the target and the failed envelope is re-delivered first to the next target — no event lost on renderer crash.
- `recovers_after_many_failures_and_reattaches` — 25 failing re-attaches, then a good one receives all 26 events — v3's "hung dispatch" counter ratcheted shut after two crashes (§14.6).
- `a_panicking_delivery_is_treated_as_a_failure` — a panic inside delivery detaches the target, the pump keeps working — the pump thread must never die.
- `detached_buffer_drops_levels_first` — over the bound while detached, levels go, deltas stay — "bounded, levels dropped first".
- `hard_cap_never_drops_must_deliver` — past the absolute cap only non-must-deliver events are dropped — terminal events survive any backlog.
- `replacing_the_target_routes_new_events_to_the_new_one` — after a re-subscribe new events go only to the new Channel — page reload replaces the old channel (§9 subscribe_events).
- `emit_does_not_block_while_the_renderer_is_stalled` — 10 000 emits against a blocked delivery return promptly and all arrive later — `EventSink::emit` must not block the session actor.

## callcore-shell — status (`StatusHub`)

- `starts_starting_unknown_idle` — initial snapshot is core starting / protection unknown / idle — "never claim protection before Windows confirmed it".
- `every_change_bumps_the_revision_and_no_op_does_not` — core, protection and session changes each bump the revision; identical sets don't — §9 snapshot rules.
- `core_failed_carries_the_error` — failed state includes the error in the snapshot — the page shows an actionable message after reload.
- `protection_regression_is_a_change` — protected → unprotected bumps the revision — a regression must reach the page.
- `unknown_session_snapshot_keeps_revision_and_id` — the 2 s fallback snapshot says `unknown` without bumping the revision — "unknown" is "ask again", not a state change.
- `snapshot_serializes_camel_case` — wire shape matches the generated TS — cross-language contract.

## callcore-shell — geometry

- `dock_centres_on_the_work_area_top` — dock = top of the work area, horizontally centred (incl. top taskbar) — answer under the webcam.
- `dock_on_negative_monitor` — docking works on a monitor at negative coordinates — left/upper secondary displays.
- `dock_wider_than_work_area_starts_at_left_edge` — oversized window stays reachable — never place the title bar off-screen.
- `saved_position_on_left_negative_monitor_is_reused` — a valid saved position on a negative-x screen is kept — each layout remembers its position.
- `saved_position_from_unplugged_monitor_docks_on_current` — a position on a now-missing monitor docks on the current display at the saved size — unplugged monitor fixture.
- `title_bar_off_screen_top_is_not_reachable` — body visible but title strip above the work area → not reused — the title bar must be reachable.
- `only_a_sliver_visible_is_not_reachable` — 30 px visible fails, 40 px passes — the ≥40 px rule.
- `straddling_two_monitors_is_reachable` — a window across two screens counts — dual-monitor fixture.
- `stacked_monitors` — vertically stacked displays; the gap between work areas is not reachable — stacked fixture.
- `mixed_dpi_default_size_scales_with_the_target_monitor` — default size and title strip scale at 100/125/150 % — mixed-DPI fixture.
- `saved_size_is_clamped_to_min` — saved size below the layout minimum grows to it (DPI-scaled) — min sizes per layout.
- `tiny_work_area_clamps_but_never_below_min` — small work area clamps size, min wins; prompter clamps to work-area width — tiny work area fixture.
- `prompter_default_fits_wide_screen` — prompter strip default 900×200 docked top-centre — prompter layout.
- `no_monitors_still_gives_a_usable_size` — with no monitor info a usable size at (0,0) — "with no monitor info still apply a usable size".
- `first_run_docks_top_centre_of_primary` — first run docks on the primary (listed first) — "first run starts docked top-centre".
- `oversized_saved_window_shrinks_to_its_monitor` — saved size larger than the work area shrinks — resolution drops between runs.
- `zero_sized_saved_bounds_fall_back_to_default` — degenerate saved bounds are ignored — corrupt geometry.
- `layout_sizes_match_spec` — full 380×520/440×640, prompter min 380×160 — spec §13 numbers.
- `nonsense_scale_is_treated_as_100_percent` — NaN/0 scale factors don't produce 0-size windows — defensive DPI math.

## callcore-shell — hotkey

- `parses_the_default` — "Ctrl+Shift+Space" round-trips — default hotkey.
- `modifiers_are_case_insensitive_and_normalized_in_order` — any case/order/alias (Control, Meta, Super, Win) normalizes to Ctrl+Alt+Shift+Win order; Win → "Super" in the accelerator — the plugin spells the Windows key "Super".
- `key_families` — letters, digits, F1–F24, arrows, Enter, Tab, punctuation (symbol and name) — supported key set.
- `empty_is_disabled` — empty/whitespace = Disabled, not Invalid — "empty = disabled".
- `invalid_inputs` — no modifier, no key, two keys, empty token, F25/F0/F01, unknown key/modifier, trailing '+', >100 chars → Invalid — honest status.
- `exactly_100_chars_is_accepted_by_the_length_rule` — the ≤100 boundary — off-by-one guard.
- `messages` — exact "… is already taken by another app" copy; registered has no message — UI copy.
- `all_key_names_are_distinct_and_parse_back` — the exported key list is canonical — feeds the plugin-compat test.
- `happy_path_registers` — begin → complete(ok) → Registered with the accelerator recorded — registrar basics.
- `change_unregisters_the_previous_one` — a new hotkey plan unregisters the old accelerator — no ghost key.
- `taken_hotkey_reports_unavailable` — OS refusal → Unavailable with the taken message — honest status.
- `disabled_and_invalid_are_immediate_and_unregister` — no registration needed; previous key unregistered — disabling must actually release the key.
- `in_flight_registration_is_not_reported_as_registered` — while the OS call is pending the report is not "registered" — never claim a key that doesn't work yet.
- `late_success_after_a_newer_request_is_undone` — a stale registration that succeeds after a newer one is undone — §13 "a registration that completes late must be undone".
- `late_success_after_disable_is_undone` — late success after the user disabled the hotkey is undone — same, for Disabled.
- `late_failure_is_ignored` — a stale failure doesn't clobber the newer status — only the latest request changes state.
- `late_success_of_the_same_combo_is_adopted_when_the_newer_failed` — same combo raced itself: keep the working registration — avoids unregistering the key the user wants.
- `late_success_before_newer_completes_is_undone` — stale success while the newer is still in flight is undone — ordering edge.

## callcore-shell — url

- `accepts_plain_https` — normal https links, ports, IPv6 literal, uppercase scheme — "Get a key" links work.
- `rejects_other_schemes` — javascript:, http:, file:, ms-settings:, mailto:, scheme-relative, empty — https only.
- `rejects_missing_host` — `https://`, `https:///x`, `https://?q`, `https://:443/` — host required.
- `rejects_whitespace_and_controls` — spaces, tabs, newlines, C0/C1 controls, NBSP, bidi override, zero-width, quotes, backslash — nothing that could split an argument or spoof a host.
- `rejects_userinfo` — `user:pass@host` and `google.com@evil.com`; '@' in the path is fine — no userinfo.
- `length_limit` — 2048 accepted, 2049 rejected — ≤2048 chars.
- `bad_ports` — out-of-range/non-numeric/empty ports — well-formed authority.
- `multibyte_prefix_does_not_panic` — a multi-byte char straddling byte 8 doesn't panic — hostile input.
- `app_origins_are_allowed` — tauri://localhost, http(s)://tauri.localhost, localhost:5173 — navigation policy allows the app.
- `everything_else_is_blocked` — other hosts/ports/schemes are blocked — the webview never navigates away.

## callcore-shell — close guard

- `inactive_guard_always_allows` — no unsaved work → close — default.
- `first_close_cancelled_second_within_window_allowed` — cancel once, second close within 10 s closes — §13 Close.
- `cycle_restarts_after_window_when_page_responded` — after 10 s with an acked request the next close is cancelled again — responsive page keeps protection of the draft.
- `unresponsive_page_allows_next_close_even_after_window` — no ping after the request → next close closes — "an unresponsive page always closes".
- `a_ping_after_the_ack_window_is_not_an_ack` — a ping > 3 s after the request doesn't count — ack must be a reaction.
- `no_attached_page_allows` — nobody to receive `window:close-requested` → close — never uncloseable.
- `clearing_the_guard_allows_and_resets` — `set_close_guard(false)` closes and resets the cycle — draft saved/discarded.
- `never_uncloseable_under_repeated_clicks` — rapid clicks: the second closes — the window must never become uncloseable.

## callcore-shell — build info / diagnostics / command helpers

- `missing_values_are_unknown` — missing env → "unknown", not dirty — builds without git.
- `present_values_pass_through` — rev/dirty/time pass through — §17 build info.
- `macro_uses_this_crates_version` — `build_info!()` expands `CARGO_PKG_VERSION` in the caller — single version source.
- `iso8601_known_values` — epoch → ISO-8601 incl. leap day and 2100 — hand-written date math.
- `redacts_provider_keys` — `sk-…`, `gsk_…` (also after `key=`) are redacted — §14.12.
- `redacts_long_hex_and_base64_runs` — 40-hex Deepgram-style keys and base64 blobs incl. padding — §14.12.
- `keeps_ordinary_text` — timestamps, paths, long words survive — diagnostics stay useful.
- `render_includes_status_and_redacts_logs` — version, build, OS, protection, core error, settings status, hotkey, event stats, logs — and a key in a log line is redacted — "Copy diagnostics" contents.
- `render_caps_log_lines` — at most 200 log lines — bounded output.
- `timeout_maps_to_internal` — a command past 30 s → `internal` — §9.
- `guarded_passes_values_and_errors_through` — envelope preserves values and error codes — boundary contract.
- `guarded_command_exceeding_30s_resolves_internal` — a never-resolving command resolves `internal` — nothing hangs across the boundary.
- `panics_become_internal_without_the_payload` — a panic becomes `internal` without leaking the payload text — nothing panics across the boundary; no secrets in errors.
- `gate_waits_while_starting_then_returns_core` — commands wait for the core while starting — §3.
- `gate_times_out_with_core_starting_copy` — after 25 s still starting → CORE_STARTING — bounded wait.
- `gate_fails_immediately_after_failure` — after failure → CORE_FAILED with zero wait — "after failure they return an actionable error at once".
- `gate_waiter_released_by_failure` — a waiting command is released when startup fails — no command stuck for 25 s.

## src-tauri — unit tests

- `logging::tests::tail_keeps_last_lines` — the in-memory tail keeps the last 200 lines — diagnostics source.
- `logging::tests::partial_writes_join_into_one_line` — split writes form one line — tracing writes may be chunked.
- `logging::tests::rotates_by_size_and_keeps_three_files` — size rotation keeps aica.log + 2 old files, each under the cap — rotating log (§17).
- `logging::tests::panic_line_redacts_keys` — panic log line is redacted and truncated — crash log never holds secrets.
- `platform::tests::os_version_reads_a_real_version` — RtlGetVersion returns a real Windows version — diagnostics OS string.
- `platform::tests::every_parsed_hotkey_is_accepted_by_the_plugin_parser` — every key × modifier combo our parser accepts also parses in tauri-plugin-global-shortcut — "valid" in Settings must mean registrable.
- `protection::tests::only_0x11_is_protected` — only WDA_EXCLUDEFROMCAPTURE counts (not WDA_MONITOR/none/failed read) — the moat.
- `protection::tests::verdict_trusts_the_readback_not_the_apply_result` — apply "success" without the read-back is NOT protected; unreadable is not protected — never claim protection Windows didn't confirm.
- `protection::tests::invalid_hwnd_is_unprotected` — real Win32 calls on a bad handle → Unprotected, no crash — failure path.
- `tests::csp_is_tight` — shipped CSP has default/script/style 'self', object/base/form none, no unsafe-*, connect-src only IPC — §13 CSP.
- `tests::identity_and_version_are_stable` — identifier `com.aicallassistant.app`, version == workspace, per-user NSIS — §17.
- `tests::capability_grants_no_plugin_or_window_apis` — the page gets no plugin/window permissions — it can't touch shortcuts, window or protection.
- `tests::navigation_stays_on_the_app_origin` — navigation hook allows only the app/dev origins — no in-app navigation away.
- `tests::manifest_declares_per_monitor_v2_and_common_controls` — manifest has PerMonitorV2 + comctl v6 — DPI requirement + test-binary loading.

## src-tauri — §14.1 wiring integration (`tests/wiring.rs`)

Built on the real `build_core`, `AppCtx`, command functions, `EventPump` and
production `ChannelDelivery` over a real `tauri::ipc::Channel`, with fake
audio / STT / provider / keystore / window behind the ports and a real
`SettingsStore` in a temp dir.

- `record_flow_events_land_on_the_page_channel` — start → frames → stop through the command functions; `stt:partial`, every `llm:delta` in order and exactly one `llm:done` land on the Channel with increasing seq — §14.1: the v3 "sink never attached" bug.
- `resubscribe_mid_session_still_delivers_the_terminal_event` — a new Channel mid-session (reload) gets `llm:done`, the old one doesn't; `get_status` re-adopts the live session — reload recovery (§9, §14.6).
- `ask_flow_and_status_revisions` — ask emits the question as a final `stt:partial` and the answer; status revision grows; empty question errors — ask flow + snapshot revisions.
- `core_failure_gives_actionable_errors_and_status_still_answers` — after `core:failed` commands return CORE_FAILED at once, get_status still answers, cancel is still ok — §3 startup failure.
- `set_settings_side_effects_run_and_stale_revision_applies_nothing` — a save re-registers the hotkey, flips always-on-top, switches layout (persisting the old layout's bounds first); a stale-revision save errors with no side effects — §12 save side effects + revisions.
- `taken_and_invalid_hotkeys_report_honestly` — taken → Unavailable with the exact copy (visible in get_settings), invalid → Invalid, empty → Disabled — honest hotkey status.
- `late_hotkey_registration_is_undone` — a registration stuck on a busy UI thread is applied late (with `settings:changed`) when still current, and undone when superseded — §13 late registration.
- `protection_is_verified_with_retries_and_never_claimed_early` — retries until Windows confirms; all-fail publishes `protection:failed` whose revision equals the snapshot's — §11.
- `close_guard_cancels_once_and_notifies_the_page` — guard + attached page: first close cancelled and `window:close-requested` lands; second closes; no page → closes — §13 Close.
- `open_external_validates_before_touching_the_os` — bad URLs never reach the OS; https does — open_external.
- `dock_persists_bounds_for_the_current_layout` — dock_window saves the docked bounds for the current layout — each layout remembers its geometry.
- `diagnostics_have_status_and_no_secrets` — diagnostics contain version/OS/core/log lines and no key material; settings view never carries keys — §14.12.
- `shutdown_is_bounded_and_idempotent` — shutdown with a live session finishes within SHUTDOWN_BUDGET, later commands fail cleanly, second shutdown is harmless — the process must never hang.
- `invoke::commands_resolve_through_tauri_ipc` — on the `tauri::test` mock runtime, the real `#[tauri::command]` handlers resolve through IPC: `{ok,value}` envelopes, camelCase `sessionId`, error envelope (not a rejection), cancel always ok, Channel argument attaches the pump, start_session returns an id — invoke-level check of the handler layer.
