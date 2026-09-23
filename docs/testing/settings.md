# Settings store tests (`callcore-settings`)

Run: `export CARGO_TARGET_DIR=target-core; cargo test -p callcore-settings`.
All store tests run in `tempfile` dirs with a fake keystore (`FakeKeystore`: reversible XOR "encryption" whose protect/unprotect can be told to fail and which counts protect calls). The real DPAPI is exercised under `cfg(windows)` only. No test touches the network or the real `%APPDATA%` settings file, and none sleeps a fixed time.

## Pure helpers (`fsio`, `schema`, `keystore`)
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

## Load / per-field validation (`tests::load`)
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

## v3 migration (`tests::migrate`)
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

## Corrupt / unreadable file backup (`tests::backup`)
- `corrupt_file_untouched_when_closed_without_writing` — open corrupt, view, drop: bytes identical, no backup/temp files — spec §14.8: closing untouched must not destroy the file.
- `corrupt_file_backed_up_before_first_patch` — first patch creates exactly one `settings.json.corrupt-<stamp>-<id>.bak` holding the original bytes, issue gets `backupPath` and a message naming it — backup-before-overwrite + UI surfacing.
- `corrupt_file_backed_up_before_first_geometry_save` — geometry autosave obeys the same rule — the v3 bug path (autosave clobbered the corrupt file).
- `backup_happens_only_once` — later writes don't make more backups — no .bak spam.
- `rejected_patch_does_not_back_up_or_write` — stale/invalid patches on a corrupt load leave file and dir untouched — only real writes trigger the backup.
- `first_run_write_makes_no_backup` — no file → no backup — nothing to preserve.
- `unreadable_file_loads_defaults_and_blocks_writes_when_backup_fails` — `settings.json` as a directory → Unreadable issue; the first write's backup fails → `writesBlocked`, actionable error, all later writes refused, original untouched — "if the backup fails, refuse to write".
- `unreadable_file_is_backed_up_by_reread_when_it_becomes_readable` — an unreadable file that becomes readable before the first write is re-read and backed up as `settings.json.unreadable-…bak` — transient AV locks.
- `corrupt_backup_failure_blocks_all_writes_even_after_recovery` — backup impossible (settings dir replaced by a file) → internal error naming the file, `writesBlocked`, and writes stay refused even after the dir comes back; nothing applied in memory — refusal is sticky because the original was never preserved.

## Writes, revisions, concurrency (`tests::write`)
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

## Secrets (`tests::secrets`)
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
