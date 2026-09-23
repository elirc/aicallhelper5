# Frontend state layer — tests

Scope: `src/ipc/{tauri,fake}.ts`, `src/state/**`, `src/markdown/**`, `src/app/{AppProvider,controller}.tsx?`, `src/App.tsx`, `src/main.tsx`.

Run: `npx vitest run src/ipc src/state src/markdown src/app`

Architecture in one line: `reducer(state, action)` is pure (every time value arrives on the action), `selectView(state, now)` derives the pinned `AppView`, and `createController()` (`src/app/controller.ts`) owns every side effect: commands with generations, event intake (seq dedupe, rAF delta coalescing), refused-stop recovery and serialized settings saves. `AppProvider` wires it to React through `useReducer`.

## src/state/reducer.test.ts — pure reducer

### session lifecycle from events
- `start ok -> starting until session:recording -> recording with deadline` — the phase stays `starting` until capture is really running, then takes the core's deadline, with startedAt = deadline − cap. Why: §4.3, one deadline shared by the UI countdown and the core cap (§5.11).
- `stt:partial replaces the transcript with the full text` — partials carry the FULL transcript and are not appended. Why: §4.5 contract; appending would duplicate text.
- `audio:level tracks rms and the last loud time` — the level updates, and only rms ≥ 0.01 moves lastLoudAt. Why: drives the level meter and the silence hint (§13).
- `stop ok -> finalizing; first delta -> answering; llm:done -> idle + history` — the whole happy-path phase machine plus the history push with finish/callType/metrics. Why: core UX flow §4.
- `session:error -> idle; pushed to history only when there is a question or answer` — errors end the session, and an empty failure (no_speech) does not clutter history, while a failed answer to a real question stays so the user can Regenerate it. Why: history holds answered entries; regenerating a failed question is useful.
- `at most one terminal per session: a second terminal is ignored` — a late error after `llm:done` changes nothing. Why: §5.8, defence in depth on the page side.
- `session:autostopped -> finalizing + cap-reached status` — the 120 s cap shows "Reached the 120s limit — answering now". Why: §13 status copy.
- `audio:device lost -> notice + finalizing (not the 120s copy); changed -> notice only` — a device unplug auto-stops with an honest message and never claims the cap was hit; a device switch only shows a notice. Why: §18 device handling.
- `ask: starts directly in answering with the question as transcript` — a typed question skips the recording phases. Why: §4 Ask flow.

### seq, stale sessions and buffering
- `drops envelopes with seq <= lastSeq (duplicates / replays)` — duplicated or older envelopes are no-ops. Why: the CoreApi contract says listeners must tolerate duplicates.
- `lastSeq starts at 0 on load and is never reset by snapshots or core:ready` — seq tracking resets exactly once per page load (in the initial state) and never again. Why: §14.7, the v3 double bump dropped legitimate events.
- `events for a session id that is not current are dropped (§5.3)` — stale deltas or errors from an old session can't touch the current one. Why: §5.3 invariant.
- `events arriving before the start result are buffered and replayed only if they are ours` — while the command's id is unknown, session events are buffered, then replayed for the adopted id; other ids are discarded. Why: the channel and the command reply race independently.

### events that precede the command reply (orchestrator note)
- `ask: the question's final stt:partial emitted during install is replayed once the reply adopts the id` — the question transcript and an early delta survive the race. Why: the session crate emits the ask partial during install, before the reply.
- `record: session:recording + early partial/level before the start reply are replayed in order` — deadline, transcript, level and loud time (with the original receipt time) are all applied. Why: capture can start before `start_session` returns.
- `buffered events are dropped when the command fails` — a failed start leaves no ghost deadline. Why: a failed command must not adopt anything.
- `buffered events are dropped when the command is superseded; the newer command gets only its own` — the old command's buffer is cleared on supersede, and the new one replays only its own id. Why: §5.2.
- `a superseded session needs no terminal event: the UI has already moved on locally` — supersede and cancel move the UI on without waiting for the core (which emits nothing more for those sessions). Why: orchestrator note / session-crate behavior.

### command generations (§14.2)
- `a late start-failed must not flip a newer session to idle` — a failure from an older command generation is ignored. Why: lesson §14.2.
- `a late start-aborted / start-ok must not null or hijack a newer session` — late results from an older generation never replace the newer session's id. Why: lesson §14.2.
- `the current command's failure shows the error and returns to idle` — the latest command's failure is surfaced. Why: the counterpart of the two tests above.
- `a stop is a command generation but does not orphan the pending start's slot` — Stop bumps cmdGen but the start result still owns its slot, and the stop is queued. Why: Stop during connect must work (§4.6).
- `a stop result for another session is ignored` — stop results are scoped by session id. Why: §5.3.
- `cancel while start is pending leaves an idle slot that ignores the late result` — a cancelled pending start is never adopted. Why: §5.2 (the provider then cancels the orphan).

### revision adoption
- `adopts snapshots/events only when revision >= last seen` — older snapshots and protection/core events never override newer ones. Why: §9 snapshot rules; protection status must never regress.
- `re-adopts a live session from the snapshot, including its deadline` — on reload the recording session comes back with its countdown. Why: §9 and §18 ("re-adopted session keeps its countdown deadline").
- `does not re-adopt an ended session or replace a live one from a snapshot` — a stale snapshot can't resurrect a finished session or steal the slot from a live one. Why: snapshots race with events.
- `settings adopt only when settingsRevision >= current` — a stale get_settings response can't overwrite a newer save. Why: lesson §14.10, ordering by revisions.

### history
- `caps at 6 entries, dropping the oldest` — Why: §13, last 6 answered entries.
- `prev/next navigation walks entries and returns to live` — "n/m" indices and canPrev/canNext at the edges. Why: §13 history UI.
- `clear empties history and the finished live entry` — Why: §13 Clear.
- `a new session jumps the view back to live` — Why: the user must see the new answer, not an old entry.
- `regenerate (ask with the viewed question) creates a NEW entry, never overwrites` — Why: §4 Regenerate.
- `history is view state only: entries hold no profile/settings data` — the entry shape is pinned to view fields only. Why: history is never persisted and never sent as context (§8, §13).

### first paint + misc
- `visibleFirstWordMs is measured once from the stop click` — measured only after text exists, and the first report wins. Why: §15 frontend "visible first word" metric.
- `window:close-requested sets closeRequested; dismiss clears it` — Why: §13 close guard flow.
- `closing settings clears the dirty flag` — Why: the close guard must not stay armed after leaving Settings.
- `recovery/gone returns the scoped session to idle with a notice; other ids untouched` — Why: §14.3, recovery is scoped to its session.

## src/state/select.test.ts — selectView

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

## src/ipc/tauri.test.ts — real IPC adapter (mocks `@tauri-apps/api/core`)

- `passes the exact command names and argument objects (incl. sessionId)` — `{patch}`, `{sessionId}`, `{active}`, `{url}`, `{text}`, and none for argument-less commands. Why: a wrong arg name fails only at runtime in Tauri.
- `returns the CmdResult the Rust side sends` — ok and error envelopes pass through. Why: the Rust side returns the CmdResult JSON itself.
- `never rejects: thrown / rejected invokes map to internal` — covers rejected strings, synchronous throws and odd error objects. Why: §9, nothing throws across the boundary.
- `validates the response shape defensively` — malformed values become `internal`, and unknown error codes are coerced to `internal`. Why: the UI keys behavior off a closed code set.
- `resolves to internal after the 30 s client-side timeout` — exact boundary under fake timers. Why: §9 command timeout, so a hung core can't wedge the UI.
- `subscribe attaches a Channel via subscribe_events and unsubscribe stops forwarding` — invalid messages are dropped and an unsubscribed listener stays silent. Why: §14.1 event-channel wiring.
- `retries a failed subscribe_events attach (bounded)` — 3 attempts. Why: lesson §14.1, a silently detached event sink broke v3.
- `treats a missing value on ok as null (unit commands)` — Why: Rust `()` may serialize as null or be absent.

## src/ipc/fake.test.ts — scriptable fake core

- `emit assigns increasing seq and reaches only active listeners` — Why: tests depend on realistic seq behavior.
- `logs calls and lets tests override responses` — Why: the fake's scripting API.
- `set_settings enforces baseRevision and bumps settingsRevision` — Why: needed to test the stale-save flow realistically.
- `stop is only taken for the live session` — the second stop gets "not taken". Why: needed to test refused-stop recovery realistically.
- `demo mode plays start -> recording -> partials -> stop -> deltas -> done` — Why: `npm run dev` in a plain browser must show a working flow.

## src/markdown/markdown.test.tsx — safe streaming renderer

### golden cases
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

### XSS corpus
- `renders %j with only allowed elements and class attributes` (15 inputs: `<script>`, `<img onerror>`, `javascript:` links, `<iframe>`, entities, U+202E, `<svg onload>`, `<style>`, a fence info-string injection, …) — the output contains only div/p/h1–h6/ul/ol/li/pre/code/strong/em, and the only attribute is `class`. Why: answers are untrusted LLM output shown in a privileged window.
- `dangerous text survives as visible text (not dropped, not executed)` — Why: the user must still see what the model said.
- `the source never uses innerHTML / dangerouslySetInnerHTML` — scans the renderer's own source. Why: §13 "every string a text node".

### streaming property
- `every streamed prefix renders safely, and the concatenation renders exactly like the whole` — fast-check, 300 runs of random markdown-ish strings with any cut points. Every intermediate paint is safe and doesn't throw. Why: §13 property test.
- `a live component updated chunk-by-chunk renders the same DOM as rendering the whole at once` — fast-check, 200 runs. One React root is re-rendered at each streamed prefix, and its innerHTML must equal the `renderToStaticMarkup` of the whole. Why: the real invariant is that per-block memoization never leaves stale DOM.
- `per-block memoization: finished blocks are not re-rendered while the last block streams` — earlier blocks keep the same DOM nodes. Why: the per-block memoization requirement, and paint cost while streaming.

### error boundary
- `falls back to plain text when rendering throws, and retries on new source` — Why: §13, a renderer bug must never blank the answer.

## src/app/AppProvider.test.tsx — provider + controller against the fake core

### mount wiring
- `subscribes to events BEFORE calling get_status / get_settings (§14.1)` — Why: events emitted between the snapshot and the subscription would be lost.
- `re-adopts a live session from get_status on load, keeping its deadline` — events for the adopted id land, and Stop targets it. Why: §9 reload behavior.
- `StrictMode double-mount: one initial fetch, every event applied exactly once` — Why: lesson §14.7, generation bookkeeping must bump exactly once.
- `core:ready refetches settings + status; settings:changed refetches settings` — Why: §3 readiness, and settings changed elsewhere.
- `duplicate envelopes (seq <= last) are ignored, including side effects` — a duplicated `hotkey:toggle` doesn't double-toggle. Why: side effects must be deduped as well as state.

### delta coalescing
- `buffers llm:delta and dispatches at most once per animation frame` — 3 deltas produce 1 render. Why: §13 "one paint per animation frame".
- `a non-delta event flushes buffered deltas first, preserving order` — Why: `llm:done` must never overtake buffered text.

### hotkey + window
- `hotkey toggles record/stop on the main screen` — Why: §13 global hotkey.
- `while Settings is open the hotkey may only STOP, never start` — Why: §13, explicit rule.
- `window:close-requested sets closeRequested; the close guard follows settingsDirty` — `set_close_guard(true/false)` on each change. Why: §13 close flow.

### commands
- `Stop clicked while start is still in flight is sent once the id arrives` — Why: §4.6, Stop during connect.
- `a start that resolves after Cancel tears down only the session it created` — Why: §5.2 and §14.2.
- `a late start result after a newer ask does not touch the newer session` — the orphan is cancelled and the newer session keeps streaming. Why: §14.2.
- `ask trims, rejects empty with a notice, and measures visible first word` — Why: §4 validate first; §15 visible first word.
- `regenerate asks with the viewed entry's question and adds a new entry` — Why: §4 Regenerate.
- `events emitted by the core BEFORE the start/ask reply are not lost (fake core emits during the call)` — Why: the orchestrator note, tested through the real controller.
- `cancel calls cancel_session for the live session` — Why: §4 Cancel.

### settings
- `stale save: re-fetches settings, reports stale:true, and the next save uses the fresh revision` — Why: §13 stale-revision flow; lesson §14.10.
- `a non-stale failure reports the core's message with stale:false` — Why: e.g. fail-closed encryption errors must be shown verbatim.
- `quick saves are optimistic and serialized (rapid A+ A+ both land)` — each save reads baseRevision when it runs. Why: without serialization the second click is a guaranteed stale rejection.
- `font bumps clamp to the allowed ranges` — Why: §12 limits (12–22 answer font, 14–28 prompter font).
- `setCallType patches the active profile; setStyle/setLayout/setActiveProfile save` — Why: §13 call-type dropdown saves the active profile.
- `copyDiagnostics copies the text; failures return null with a notice` — Why: §17 "copy diagnostics".
- `openExternal failures surface as a notice; dock calls dock_window` — Why: user feedback on refused links.

### refused-stop recovery (§14.3)
- `core still recording X -> retries the stop -> finalizing` — Why: the stop was refused transiently.
- `core reports unknown -> waits and asks again; X gone -> idle + cancel_session(X)` — Why: `unknown` means "wait again", and recovery ends with a safety cancel.
- `core reports X finalizing/answering -> adopts that phase, no cancel` — Why: the core is progressing, so don't kill it.
- `progress for X (a delta) disarms the recovery: no more polls, no cancel` — Why: §14.3 disarm on progress.
- `progress for ANOTHER session does not disarm it (scoped to X)` — Why: §14.3 scoping.
- `an accepted ask disarms the recovery` — Why: §14.3.
- `bounded: a core that stays unknown gives up after 5 polls -> idle + cancel_session(X)` — polling stops afterwards. Why: recovery must be bounded, and the UI must never stay stuck in "recording".
- `a newer session is never touched by an old recovery` — Why: §14.3 scoping.

### recording clock
- `ticks while recording: elapsed timer and silence hint` — a 250 ms tick drives "Recording — 0:05" and the silence hint, and a loud level clears it. Why: §13 timer and hint; the tick runs only while recording (idle CPU ≈ 0).

## src/app/App.test.tsx — real page wiring
- `record -> events -> stop -> streamed answer lands in the DOM` — the real `<App>` (AppProvider + frontend-ui components) against the fake core: button click → command, events → status line, transcript and streamed Markdown (real rAF scheduler), done → Ready. Why: lesson §14.1, catches seam breakage that component tests with hand-built views can't.
## src/app (orchestrator addition)
- `window:close-requested with no unsaved work sends no guard ack` — a close request only re-arms the guard when the page actually has unsaved work — the shell treats setCloseGuard(true) as the page's ack of a cancelled close
