# Frontend UI tests (`src/components/**`)

Run: `npx vitest run src/components` (156 tests across 6 files). Components get state only through `useApp()`, so every test renders inside `<AppContext.Provider>` with a hand-built `makeView()` view and `makeActions(() => vi.fn())` mocks. Helpers are in `src/components/testUtils.tsx` (`renderWithView`, `makeEntry`, `makeRecording`, `setScrollGeometry`). The Markdown renderer (`src/markdown`) is mocked where an answer is shown, so these tests stay independent of the renderer's own suite. jsdom has no layout, so scroll tests set `scrollHeight`/`clientHeight` by hand. Clocks use `vi.spyOn(Date, "now")`, with no real waits.

## common/common.test.tsx: shared building blocks
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

## full/full.test.tsx: full layout
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

## prompter/prompter.test.tsx: prompter strip
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

## Root.test.tsx: layout selection and cross-view guarantees
- `root_picks_full_layout` / `root_picks_prompter_layout` / `root_settings_screen_wins_over_layout`: view.screen/layout pick the screen. Why: Root contract.
- `protection_badge_<state>_in_<view>_view` (9 cases): exactly one badge with the right copy in full, prompter and settings for protected/unknown/unprotected, and the protected text never appears otherwise. Why: §11 says every view shows the verdict and never over-claims.
- `no_view_ever_says_microphone`: renders 25 states across all three views, and the HTML never contains "microphone" or "mic". Why: §1/§13 (system audio only, never the microphone).

## settings/draft.test.ts: pure patch/draft logic
- `patch_empty_for_untouched_draft`: an untouched draft gives an empty patch and is not dirty. Why: no spurious saves or close-guard prompts.
- `patch_contains_only_changed_fields`: only changed fields go in the patch. Why: brief requirement, and it limits stale conflicts.
- `patch_replaces_whole_profile_list_when_any_profile_changes`: `profiles` is sent whole. Why: the contract says profiles replaces the list.
- `patch_draft_is_not_aliased_to_settings`: editing the draft never mutates the view. Why: immutability of store state.
- `patch_hotkey_trimmed`: whitespace-only differences are not changes. Why: no phantom dirtiness.
- `secret_typed_value_sets_trimmed` / `secret_queued_remove_removes` / `secret_typed_value_wins_over_remove` / `secret_emptied_field_never_removes`: the secret semantics. Why: §12 write-only secrets rules.
- `new_profile_id_matches_contract_regex_and_is_unique` / `new_profile_id_avoids_collisions_with_constant_rng`: ids look like `p-<base36>`, match the regex and never collide. Why: §12 id schema.
- `duplicate_name_trimmed_to_60`: "<name> copy" is capped at 60. Why: §13.
- `hotkey_valid_combos` / `hotkey_needs_a_modifier` / `hotkey_needs_exactly_one_key` / `hotkey_rejects_empty_parts_and_overlong`: client-side shortcut validation. Why: §13 "any Ctrl/Alt/Shift/Win + key, validated".

## settings/settings.test.tsx: Settings screen
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
