# AI Call Assistant v4 — full specification

Ground-up rebuild of the Python + pywebview app (v3.1.0) on Tauri 2 (Rust core) + React/TypeScript.
Product behavior, UX, prompts and reliability rules below are proven and must carry over.
Contract types live in `crates/contract` (Rust, source of truth) and are generated into `src/generated` (TS).

---
## 1. Product in one paragraph

A push-to-record interview/call copilot for Windows 10 2004+ / 11. During a video call the user presses Record while the other person asks a question. The app captures SYSTEM OUTPUT audio (WASAPI loopback of the default render device — never the microphone), streams it to Deepgram for a live transcript, and on "Stop & Answer" streams an LLM answer (what the user should say, first person, spoken English) grounded in the user's active profile (resume, job description, focus, notes, call type). The answer shows at the top-centre of the screen, right under the webcam, so the user reads it while looking at the camera. The window is EXCLUDED FROM SCREEN CAPTURE (invisible in Zoom/Teams/Meet/OBS shares) — this is the product's moat and must never regress. Design target: ~1 s from Stop press to first answer word.

No account, no backend of ours, no telemetry. Bring-your-own API keys.

---
## 2. Tech stack

- Shell/runtime: Tauri 2 (Rust), WebView2. NSIS per-user bundle.
- Core: Rust, tokio. Audio: direct WASAPI loopback (`windows` crate, polling mode — cpal 0.15 event-mode loopback delivered ~4% of samples on real hardware) on a dedicated audio thread; anti-aliased resampler to 16 kHz mono i16. STT: `tokio-tungstenite`. LLM: `reqwest` streaming + hand-written incremental SSE parser. Secrets: Windows DPAPI (CryptProtectData, current-user) — must fail CLOSED. `serde`, `thiserror`, `tracing` to a rotating file (never secrets or prompt text).
- Window/OS: Tauri plugins `global-shortcut`, `single-instance`; `windows` crate for SetWindowDisplayAffinity/GetWindowDisplayAffinity, monitor work areas, per-monitor DPI.
- Frontend: React 19 + TypeScript strict + Vite, plain CSS with design tokens, pure reducer. IPC: Tauri commands (request/response) + a Tauri Channel (core→page). TS types generated from Rust (`ts-rs`).
- Tests: `cargo test` (tokio::test, trait fakes, loopback WS/HTTP servers), Vitest + Testing Library. Lint: `clippy -D warnings`, `rustfmt`, `eslint`, `tsc --noEmit`.
- CI: GitHub Actions windows-latest: lint, test, `cargo audit`, `npm audit --omit=dev`, build + bundle. One version source; a check fails on drift.

---
## 3. Architecture and threading

    loopback device ──(audio thread: capture, mixdown, resample→16 kHz i16, 2048-sample/128 ms frames + RMS)──► core (tokio) session actor
    session actor ──► Deepgram WebSocket (sender task / reader task) ──► transcript accumulator
    on Stop: stop-and-drain audio → CloseStream behind all audio → final transcript → build prompt → provider request → SSE stream → deltas ──► page
    page ◄── ordered event stream (Channel) ; page ──► commands (invoke) ──► core

- ONE session actor owns all session state (single active session slot, phase, flags, command ticket, timers). Everything else talks to it by message. No shared mutable session state.
- All blocking device calls (open, stop, drain, close) run on ONE dedicated audio worker thread, serialized, so start/stop ordering is explicit and the async runtime never blocks.
- The audio callback does minimal work; a panic/error there must drop the chunk, not kill the stream.
- Every external dependency sits behind a trait (see `crates/contract/src/ports.rs`).
- Startup: show the window FIRST, then build heavy parts; report readiness explicitly (`core:ready` / `core:failed`). Commands that need the core wait (bounded, ~25 s) only while it is starting; after failure they return an actionable error at once.

---
## 4. User flows (exact behavior)

### Record → Stop & Answer
1. Record → `start_session()`. Claim a command ticket BEFORE any await; read keys off the runtime thread; re-check the ticket (a stalled read must never supersede a newer command); supersede any active session; install; pre-warm the LLM origin; return session id ("s1","s2",…).
2. Device open (audio worker) and Deepgram connect run IN PARALLEL. Frames captured before the socket opens are buffered (max 120 frames ≈ 15 s, drop oldest) and flushed on open.
3. The recording cap (120 s) is armed when capture is ACTUALLY running (not at connect), and `session:recording {deadlineMs (epoch ms), capMs}` tells the UI the single deadline.
4. Each frame → Deepgram, and one `audio:level {rms 0..1}` (coalesced latest-wins per session).
5. Deepgram results → accumulator → `stt:partial {text (FULL transcript so far), isFinal}`.
6. Stop → `stop_session(id)`: accepted only for the live, not-yet-stopping session (including one still connecting); otherwise an error "not taken".
7. **Latency clock starts at Stop acceptance, BEFORE the drain** (the user waits for it, so it counts). Phase → finalizing; re-warm origin; audio worker runs `stop_and_drain`: stop device input and deliver EVERY pre-Stop sample (including the final partial frame) to the core, bounded at 2 s. Frames are accepted until the drain completes (the "capture cutoff") and rejected afterwards, so nothing can follow CloseStream. Then send CloseStream BEHIND all audio, wait for the server flush (5 s cap), take the transcript. Empty/whitespace transcript → `no_speech` error, NEVER an LLM call (but if the device failed to open, report the device error instead). A Stop during connect waits for the socket, then follows the same path.
8. Answer: build prompt, build the request ONCE, stream over a pre-warmed pooled client with retry-once (see §7). Deltas → `llm:delta {delta}` under a 10 s first-token and 60 s total watchdog. Success requires the provider's real terminal signal; otherwise error. Then `llm:done {transcript, answer, finish, callType, metrics}`.

### Ask (typed question)
Validate + trim FIRST (bad input must not kill a live session), then ticket/keys/supersede. Session starts directly in `answering`; metrics clock = ask acceptance; audioDrainMs = sttFinalizeMs = 0. Emit the question as one final `stt:partial` so it renders like a transcript.

### Regenerate
Frontend-only: calls `ask` with the viewed entry's question; lands as a NEW history entry, never overwrites.

### Cancel
`cancel_session(id)`: fire-and-forget, always ok, affects only that id.

---
## 5. Session machine invariants (each needs a test)

1. One live session at a time; a new start/ask supersedes (aborts) the old one.
2. A start/ask that resolves after it lost the race tears itself down and never installs over the winner. A late result for a cancelled/timed-out command cancels ONLY the session it created, never a newer one.
3. Events for a stale session id are dropped (core side and page side).
4. Capture cutoff: frames accepted until the stop-drain completes; rejected after; CloseStream is always last. Cancel uses discard-stop, never drain.
5. Once the transcript is final, the STT stream's job is done — a late socket close must NOT kill a streaming answer.
6. Never retry after a delta has been painted.
7. Never call the LLM on an empty prompt.
8. At most one terminal event per session (`llm:done` XOR `session:error`).
9. Timers (cap, first-token, total, drain) are owned by the session and cleared on every exit path.
10. Cancel is idempotent and never errors.
11. The UI countdown and the core cap use the same deadline.

Timeouts (see `crates/contract/src/config.rs`): audio drain 2 s; STT connect 5 s; STT finalize 5 s; KeepAlive every 8 s; LLM first token 10 s; LLM total 60 s; record cap 120 s; command timeout 30 s; transport timeouts (connect 10 / read 75 / write 10 / pool 10 s) stay ABOVE the machine's timers.

---
## 6. Speech-to-text (Deepgram)

- URL: `wss://api.deepgram.com/v1/listen?model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true`. Auth via `Sec-WebSocket-Protocol: token, <key>` (keeps the key out of URLs/logs). Do NOT set `endpointing` / `no_delay`.
- Send binary PCM frames; `{"type":"KeepAlive"}` every 8 s; `{"type":"CloseStream"}` to finalize.
- Parse Results frames STRICTLY as hostile input (bad JSON, wrong types, huge payloads must not crash). Accumulator: finalized segments + current interim → full transcript.
- Classify closes: 1008/401 → bad key ("Deepgram closed the connection (code 1008 …)"), network → `stt_connect`, server error mid-stream → `stt_error`, finalize timeout → `stt_timeout`.
- Resampling must be anti-aliased and phase-continuous across chunks (old: 63-tap FIR, 7.2 kHz cutoff). Downmix to mono. Frames of 2048 samples (128 ms).

---
## 7. Answer providers

Provider trait; the machine, retry, metrics and UI are provider-agnostic. Adding a provider = one module + one registry line. Registry exposes `{id, displayName}` and the key ids it needs (the settings view gets per-key status generated from it).

**Anthropic (default, "Claude Haiku 4.5 (recommended)")**
- POST `https://api.anthropic.com/v1/messages`, headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`, `stream: true`, model `claude-haiku-4-5` (single constant), `max_tokens: 1024`.
- System = TWO blocks: [cached_prefix with `cache_control: {type: "ephemeral"}`, style_suffix]; one user message. Style flips must not invalidate the cache.
- Content deltas across blocks concatenate with NOTHING between.
- Success requires `message_stop`. `stop_reason` from `message_delta`: `max_tokens` / `model_context_window_exceeded` → finish=`truncated`; `refusal` → `refused`; else `complete`. In-stream `error` event → provider_error (overloaded/rate-limit → same codes as HTTP 529/429). EOF without `message_stop` → `incomplete`. A stream of only pings is not success.

**Groq ("fastest" preset; template for any OpenAI-compatible provider)**
- POST `https://api.groq.com/openai/v1/chat/completions`, Bearer auth, model `openai/gpt-oss-120b`, `max_completion_tokens: 1024`, `temperature: 0.7`, `reasoning_effort: "low"`, `include_reasoning: false`, `stream: true`, NEVER send `reasoning_format`. ONE joined system string (cached_prefix + "\n\n" + style_suffix).
- `[DONE]` is a sentinel to skip, not EOF (bytes after it in the same chunk still count). Completion = `finish_reason` seen or `[DONE]`; `length` → truncated, `content_filter` → refused; `{"error":…}` payload → provider_error; neither terminal signal → `incomplete` (but the unterminated last SSE line is still flushed to the UI first).
- 404, or 400 with `model_decommissioned`/`model_not_found`/"does not exist" → actionable message naming the model: "switch the answer provider to Claude in Settings or install the latest version". Never tell users to edit source.

**Shared**
- Incremental SSE parser over raw bytes: survives splits mid-line, mid-JSON and mid-UTF-8 char; flushes a final unterminated line.
- Retry ONCE, only on connection-level failure before any delta, with the byte-identical request.
- Empty/whitespace answer → `empty_answer` error (never retried, never a blank "finished" entry).
- Failure kinds: connect, http, auth, rate_limit, stream_drop, aborted, provider_error, incomplete, empty_answer → mapped to the closed ErrorCode set (§10) with actionable copy (status code quoted, ≤200-char provider snippet, never a raw stack trace).
- Pre-warm: throttled (2 s) fire-and-forget GET `<origin>/v1/models` (no key, 3 s timeout) on Record, Ask and Stop so a pooled TLS connection is ready. One shared HTTP client.
- Secrets never appear in Debug/Display impls, logs, errors or panics.

---
## 8. Prompt construction (reproduce byte-for-byte; byte-stable, no timestamps/unordered joins)

Three parts: `cached_prefix` | `style_suffix` | `user_message`.

cached_prefix = call-type role
+ (if resume non-empty after trimming edges) `"\n\n--- THE USER'S RESUME ---\n" + resume`
+ (if jd non-empty) `"\n\n--- " + jd_header + " ---\n" + jd`
+ (if focus non-empty) `"\n\n--- FOCUS FOR THIS CALL ---\n" + focus`
+ (if notes non-empty) `"\n\n--- THE USER'S NOTES ---\n" + notes`
+ (grounding, only if any of those four is non-empty).

Each field is trimmed at the edges only (interior whitespace kept). Unknown call type → behavioral; unknown style → balanced.

user_message:

    The other person on the call just said:
    """
    {transcript}
    """

    What should I say?

i.e. `"The other person on the call just said:\n\"\"\"\n" + transcript + "\n\"\"\"\n\nWhat should I say?"`.
(Improvement to make: the transcript can contain `"""` and break out of the quoted section; choose a robust delimiter or escaping, and add a test. When the transcript contains no `"""` the bytes must be exactly as above.)

PREAMBLE = "You are a real-time call assistant helping the user during a live call. You are given a transcript of what the other person just said. Reply with exactly what the user should say next, written in first person, in natural spoken English. Do not add meta commentary, greetings, or quotation marks — output only the words to say."

Call types (id — label — jd_header — role — grounding):

1. `behavioral` — "Behavioral interview" (DEFAULT) — "THE JOB THEY ARE INTERVIEWING FOR"
   role: "You are a real-time call assistant helping the user answer questions asked of them during a live interview or call. You are given a transcript of what the other person just said. Reply with the answer the user should say, written in first person, in natural spoken English. Do not add meta commentary, greetings, or quotation marks — output only the answer itself. If the transcript contains no real question, briefly suggest what the user could say next. Answer with a specific example from the user's own experience whenever the question invites one, and make the outcome concrete when the resume gives one. Structure for longer answers: what the situation was, what you did, what the result was."
   grounding: "\n\nGround every answer in the resume and target role above. Never invent experience the resume does not support."

2. `technical` — "Technical screen" — "THE JOB THEY ARE INTERVIEWING FOR"
   role: PREAMBLE + " This is a technical screening interview. Answer the technical question directly and correctly first, using the precise names of the APIs, data structures, or language features involved, then add the one tradeoff or edge case a senior engineer would mention. Prefer the tools and stack named in the focus section; if a question is about a technology you have not used, say how you would approach it rather than bluffing. If the transcript is a coding problem, state the approach and its time and space complexity, not a full code listing. If the transcript contains no real question, briefly suggest a clarifying question the user could ask. Structure for longer answers: the direct answer, how it works, when you would and would not use it."
   grounding: "\n\nUse the resume, role, and focus above to choose which technologies and examples to lead with, but answer the technical question on its merits — technical facts do not need to come from the resume. Never claim hands-on experience the resume does not support."

3. `system_design` — "System design" — "THE JOB THEY ARE INTERVIEWING FOR"
   role: PREAMBLE + " This is a system design interview. Treat the transcript as a design prompt or a follow-up on one. Start by naming the one or two requirements or constraints that drive the design, then propose the components and how data flows between them, then name the main tradeoff and what you would change at ten times the scale. Ask one clarifying question when the requirements are genuinely ambiguous rather than assuming. If the transcript contains no real question, briefly suggest the next part of the design the user could walk through. Structure for longer answers: requirements and constraints, core components and data flow, the key tradeoff, how it scales or fails."
   grounding: "\n\nDraw on the systems and scale described in the resume and focus above for concrete examples, and never claim to have built something the resume does not support."

4. `recruiter` — "Recruiter screen" — "THE JOB THEY ARE INTERVIEWING FOR"
   role: PREAMBLE + " This is a recruiter or HR screening call. Keep answers short, warm, and positive: one or two sentences that confirm interest, summarize fit, or give a straight logistics answer (availability, location, work authorization, notice period). For compensation questions give a range or defer to the full process — never a single number unless the notes say otherwise. If the transcript contains no real question, briefly suggest a question the user could ask the recruiter about the process or the team. Structure for longer answers: the direct answer, one sentence of relevant background, one sentence of enthusiasm for the role."
   grounding: "\n\nGround every answer in the resume and target role above. Never invent experience or credentials the resume does not support."

5. `sales` — "Sales or customer call" — "ABOUT THIS CALL"
   role: PREAMBLE + " This is a sales, customer, or client call where the user represents their company or product. Work out what the other person is really asking for — a feature, a price, reassurance, a next step — and reply with the answer that moves the conversation forward: address an objection with a specific benefit, answer a factual question plainly, or propose the next concrete step. Never invent pricing, capabilities, or commitments; when the notes do not cover a detail, say you will confirm it and move on. If the transcript contains no real question, briefly suggest a discovery question the user could ask. Structure for longer answers: acknowledge their point, the specific answer or benefit, the next step."
   grounding: "\n\nGround every claim in the notes and background above. Never invent pricing, features, customers, or commitments they do not support."

6. `meeting` — "General meeting" — "ABOUT THIS MEETING"
   role: PREAMBLE + " This is a general work meeting or discussion, not an interview. Reply with the most useful contribution the user could make right now: answer the question if one was asked, otherwise offer the one clarifying question, decision, or next step the discussion needs. Keep it collegial and concrete. If the transcript contains no real question, briefly suggest what the user could say next. Structure for longer answers: the point, the reason, the proposed next step."
   grounding: "\n\nUse the background and notes above for context. Never invent facts, decisions, or commitments they do not support."

Style suffixes (global, not per profile):
- brief: "Answer in one or two spoken sentences — the shortest reply that fully answers the question. No lists, no headings, no lead-in."
- balanced (default): "Be concise and confident: a few sentences for simple questions, short structured points for complex ones."
- detailed: "Give a structured answer: one sentence that answers directly, then three to five short supporting points following the structure for longer answers given in the call guidance above. Keep every point short enough to say in one breath — this is spoken aloud, not read."

Answers are SINGLE-TURN: no history is ever sent as context (document this in the UI/guide).
Build an offline prompt-eval harness (fixture questions per call type, snapshot outputs, opt-in live runs with a cost budget).

UI field labels for sales/meeting: resume → "Background", jobDescription → "Call context".

---
## 9. Core ↔ UI contract

All commands return `{ok:true, value} | {ok:false, error:{code, message}}`; nothing panics or throws across the boundary; a command exceeding 30 s resolves to `internal`.

| command | args | value |
|---|---|---|
| get_settings | – | SettingsView (incl. hotkeyRegistered, hotkeyStatus) |
| set_settings | {patch} | fresh SettingsView; a patch with a stale `baseRevision` is rejected, nothing applied |
| get_status | – | StatusSnapshot — never blocks on a busy core (answers phase `unknown` within 2 s) |
| start_session | – | session id |
| stop_session | {sessionId} | null; error if not taken |
| ask | {text} | session id |
| cancel_session | {sessionId} | null, always |
| set_close_guard | {active} | null |
| dock_window | – | null — move window top-centre of its current display |
| open_external | {url} | null — https only, host required, no whitespace/control chars, ≤2048 chars |
| get_diagnostics | – | string (version, build, OS, protection, settings load status, recent log lines — no secrets/prompts) |
| subscribe_events | {channel} | null — attaches the ordered event Channel; a re-subscribe (page reload) replaces the old one |

Events: see `CoreEvent` / `EventEnvelope` in the contract. Every payload has a monotonic `seq`; session events carry `sessionId`.

metrics (integer ms): `audioDrainMs` (drain start → drain complete; must NOT include STT connect time — measure from when the drain actually starts if Stop landed during connect), `sttFinalizeMs` (drain complete → final transcript), `firstTokenMs`, `totalMs` (both from Stop acceptance / ask acceptance, INCLUDING the drain). Test: injecting a known drain delay increases firstTokenMs by exactly that delay.

Snapshot rules: `revision` increases whenever core state, protection or session changes; protection/core events carry the same counter; the page adopts a snapshot/event only if its revision ≥ last seen; on load/reload the page calls get_status() and RE-ADOPTS a live session (including its recording deadline) instead of losing it. Terminal and protection events must never be dropped even under backpressure; audio levels coalesce latest-wins.

---
## 10. Errors (closed set)

`no_stt_key, no_llm_key, stt_connect, stt_error, stt_timeout, no_speech, llm_auth, llm_http, llm_rate_limit, llm_first_token_timeout, llm_timeout, aborted, internal`. Canonical copy in `callcore_contract::types::copy`. Device-open failure uses code `internal` with `copy::DEVICE_OPEN`.

Provider failure → code: Auth→`llm_auth` ("Anthropic rejected the API key (401)"); RateLimit→`llm_rate_limit`; Connect/Http/StreamDrop/ProviderError/Incomplete/EmptyAnswer/ModelUnavailable→`llm_http` (message specific to the kind); Aborted→`aborted`. Watchdogs → `llm_first_token_timeout` / `llm_timeout`. STT: BadKey/Connect→`stt_connect` (keep the "code 1008" message for BadKey), Server/Closed mid-stream→`stt_error`, finalize timeout→`stt_timeout`.

---
## 11. Content protection (the moat)

- `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE=0x11)` and VERIFY by reading back with `GetWindowDisplayAffinity`. Re-apply and re-verify on every show/reload/layout change and with retries at startup. Verdict is tri-state; starts `unknown`.
- The UI shows the verdict in EVERY view (full, prompter, settings): "Hidden from screen capture" / "Screen-share protection not confirmed yet" / red alert "Windows would not hide this window". Never claim protection before Windows confirmed it.
- The page must NOT be able to set the verdict (no command for it).
- Release gate: manual verification from the REMOTE side on Zoom, Teams, Meet (Edge and Chrome) and OBS, in every view.

---
## 12. Settings and secrets

File: `%APPDATA%\AICallAssistant\settings.json` (keep path; migrate the v3 format, including legacy top-level `resume`/`jobDescription` → one "Default" profile, and legacy `plain:`/`enc:` secret prefixes).

Schema: `profiles[]` {id `^[A-Za-z0-9_-]{1,64}$`, name ≤60 chars, callType, focus ≤2000 chars, resume, jobDescription, notes (each ≤200 000 chars)}, max 20 profiles, ≥1 always, `activeProfileId` (fallback: first profile), `llmProvider` (default anthropic), `answerStyle` brief|balanced|detailed (default balanced), `hotkey` (default "Ctrl+Shift+Space", ≤100 chars, empty = disabled), `alwaysOnTop` (default true), `layoutMode` full|prompter, `prompterFontPx` 14–28 (default 18), `answerFontPx` 12–22 (default 14), step 2, per-layout bounds (`windowBounds`, `prompterBounds`), `settingsRevision`.

Rules:
- Per-field validation; invalid fields fall back without discarding the rest of the file.
- Atomic writes (temp + replace with retry), serialized; saves carry `baseRevision` and are rejected if stale; each save's side effects (hotkey re-register, always-on-top) finish before the next save starts.
- Missing file = first run. Unreadable/invalid file → load defaults AND back the original up as `settings.json.<status>-<timestamp>-<id>.bak` BEFORE the first write of any kind (including geometry autosave); if the backup fails, refuse to write. Surface this in the UI.
- Secrets (deepgram, anthropic, groq, …): DPAPI-encrypted, fail CLOSED (encryption failure → save refused, previous key untouched). Write-only from the UI: the view exposes only has-key booleans and per-key storage status; key material never enters a view. Emptying the field does not remove a key; an explicit Remove does; typing a new key cancels a queued Remove. Undecryptable values read as unset.
- Profiles are plain text on disk (say so in the UI).

---
## 13. UI/UX specification

Full layout (min 380×520; answer FIRST, right under the header, at camera level):
- Header: status dot, app name, active provider/profile chip, ⤒ enter prompter, ⊤ dock under camera, ⚙ settings, protection indicator.
- Status line: "Ready — press Record while the other person is speaking", "Starting system-audio capture…" (never say "microphone"), recording timer, "Finalizing transcript…", "Generating answer…", "Reached the 120s limit — answering now", first-run "open Settings…" prompt.
- SUGGESTED ANSWER panel: streaming Markdown, A−/A+ text size, Regenerate, Copy (copies the markdown source; clipboard fallback), latency chip ("first word in X ms", tooltip with all four metrics), a note when finish=truncated/refused, sticky auto-scroll that stops when the user scrolls up.
- QUESTION HEARD panel: live transcript.
- Record / Stop & Answer button (while finalizing: "Finalizing…" with aria-disabled, focus kept), hotkey hint under it (or why the hotkey is unavailable).
- "Type a question instead…" input + Ask (Enter submits).
- Profile dropdown (when >1), Call type dropdown (changes the active profile's call type and saves).
- Style chips Brief / Balanced / Detailed (global).
- Level meter while recording; "No call audio detected yet" after 5 s of silence (explains the default output device); countdown in the last 30 s, driven by the core's deadlineMs.
- History: last 6 answered entries (view state only, never persisted, never sent as context), ← / → with "n/m", Clear available from one entry, entries tagged with call type.

Prompter layout (min 380×160): a short wide strip docked top-centre of the CURRENT display (camera line). Only the answer, large text (18 px default, 14–28), comfortable column width; the text does NOT jump while streaming (anchored at top; the user scrolls at their own pace with a "▼ more" hint); compact controls: record button + timer, one-line question, style chips, history arrows, A−/A+, ⤒ re-dock, ⤢ / Esc exit. Status line in the strip (including finalizing). Each layout remembers its own size/position.

Settings panel: API keys (Deepgram, Anthropic, Groq) with "saved — type to replace", Remove, "Get a key" links, storage disclosure; Profile editor (edit dropdown, Add, Duplicate — name trimmed to 60, Delete, disabled at 20); field labels change for sales/meeting ("Background", "Call context"); answer provider; style; global shortcut (any Ctrl/Alt/Shift/Win + key, validated, honest status: registered/disabled/invalid/unavailable); keep on top; version chip from the single version source; encryption disclosure; "Copy diagnostics". Save disables the form ("Saving…"), serialized; Back/Escape with unsaved changes shows a bar: "Save and go back" / Discard / keep editing; only Discard throws the draft away. A stale-revision rejection reloads settings, keeps the draft, and tells the user nothing was saved. While a session is active, Settings shows a status bar with Stop & Answer; the global hotkey still STOPS (never starts) a recording while Settings is open.

Global hotkey: toggles Record/Stop from any app (RegisterHotKey semantics, MOD_NOREPEAT); a registration that completes late must be undone.

Window: always on top (setting), single instance (second launch focuses the first), native title bar reachable; first run starts docked top-centre. Geometry restore: reuse a saved position only if enough of the window (≥40 px, including a reachable title-bar strip) is on SOME connected monitor's work area; otherwise dock top-centre of the current display at the saved/default size; with no monitor info still apply a usable size. Per-monitor DPI aware (v2) — handle mixed 100/125/150 % setups and negative coordinates.

Close: if the page reports unsaved work (close guard), the first close is cancelled once and `window:close-requested` opens the unsaved bar; a second close within 10 s, or an unresponsive page, always closes. Exit: cancel the live session, close clients, stop the audio worker, bounded at 3 s, then force exit. The window must never become uncloseable and the process must never hang.

Markdown renderer: small safe subset — paragraphs, headings, bullet/numbered lists, bold, italic, inline code, code blocks; links deliberately NOT rendered; every string a text node (no innerHTML); per-block memoization; must render identically whether text arrives in one piece or streamed at ANY cut point (property test); error boundary falls back to plain text. Coalesce deltas to one paint per animation frame.

Accessibility: keyboard reachable, visible focus, aria-live for status/answer, alerts use role=alert, sufficient contrast, respects reduced motion. Light/dark via tokens.

CSP: `default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'` plus only the connect-src the page truly needs (IPC only — all network I/O is in Rust).

---
## 14. Hard-won lessons from v1–v3 (each must be a regression test)

1. The two channels (commands vs events) fail independently: in v3 the event sink was never attached for four commits — every command worked, no transcript/answer/error ever reached the page, and all unit tests were green because each side mocked the other. Have an integration test that builds the REAL app wiring against a fake window/webview and asserts emitted events land.
2. Stale command responses: a late start-aborted must not null a newer session; a late start-failed must not flip a newer session to idle; only the latest command may change UI state (use command generations).
3. Refused-stop recovery must be scoped to its session, disarmed by any progress (delta, accepted ask/start), ask the core via get_status before giving up, treat `unknown` as "wait again" (bounded), and cancel the core session when it does recover.
4. Audio tail: flushing the resampler remainder alone is wrong; define the capture cutoff and drain before CloseStream (§4.7). Never hold the audio lock across the device stop call (deadlock with the callback). Clear the frame sink on stop so a late callback can't post into the next session.
5. Never hide latency: the Stop timestamp stays before the drain.
6. A backpressured event path must never drop terminal events or protection status, and must recover after a renderer reload (v3 bug: a global "hung dispatch" counter ratcheted shut forever after two renderer crashes).
7. Page-generation bookkeeping must bump exactly once per reload (v3 double bump dropped legit events).
8. Corrupt settings: back up before overwrite; closing the window untouched must not destroy the file.
9. Encryption must fail closed; never silently store plaintext while the UI says "encrypted".
10. Save ordering is a logical problem, not a file-corruption one: use revisions.
11. Device-open failure + early Stop must report the device error, not "no speech".
12. Keep API keys and profile text out of Debug output, logs, panics and crash reports.
13. Timing tests must wait for conditions, never sleep a fixed time; no flaky tests.

---
## 15. Performance targets (measure, don't assume)

- Stop → first answer word ≤ ~1 s p50 on a normal connection (core-side metric), plus a frontend-side "visible first word" measurement.
- Window visible < 500 ms after launch; core ready < 2 s.
- Idle CPU ≈ 0; memory < 150 MB; installer < 15 MB target.
- Latency benchmark harness with fake providers/STT that injects known delays.

---
## 16. Testing requirements

- No test touches the network, a live provider or an audio device (fakes behind traits; loopback WebSocket/HTTP servers replaying scripted Deepgram frames and SSE bytes, including hostile chunking: splits mid-line, mid-JSON, mid-UTF-8).
- Session machine: every invariant in §5 and every lesson in §14, with a controllable clock.
- Providers: conformance matrix per provider (success, error before/after text, empty, whitespace, premature EOF, token limit, refusal, 401/403/404/429/5xx, stream drop, retry policy).
- Settings: validation, migration, backup/refusal, fail-closed secrets, revisions.
- Geometry: pure functions tested with multi-monitor/negative/DPI fixtures.
- Prompt: byte-exact snapshots for every call type × style × empty/non-empty fields.
- Frontend: reducer unit tests; component tests for every flow in §4/§13; markdown streaming property test and XSS corpus.
- `docs/TESTING.md`: one bullet per test — what it verifies and why it exists.

---
## 17. Packaging, release and docs

- One version source (Cargo workspace version); `tools/check-version.mjs` fails on drift across Cargo, package.json, tauri.conf and the UI chip. Embed build info (version, git revision, dirty flag incl. untracked files, build time).
- Pinned dependencies (Cargo.lock, package-lock.json committed); `cargo audit` and `npm audit` in CI.
- Per-user install (no admin), stable app identifier, uninstall keeps %APPDATA% data. Code signing documented (SmartScreen).
- Crash log in %APPDATA% (rotated, no secrets/prompts); a "copy diagnostics" action.
- Docs: README (setup, build, add-a-provider recipe), ARCHITECTURE.md, TESTING.md, TROUBLESHOOTING.md (every error message → cause → fix), USER-GUIDE.md, RELEASE-CHECKLIST.md with the native matrix.

---
## 18. Known gaps in v3 fixed here

- Transcript `"""` can break the prompt's quoting (§8).
- Mid-recording device unplug detected (auto-stop + answer with what was captured, `audio:device {kind:"lost"}`); default-device switch followed (`audio:device {kind:"changed"}`).
- audioDrainMs excluded STT connect time when Stop landed during connect (§9).
- Re-adopted session keeps its countdown deadline (StatusSnapshot.session.recording).
- Prompt-quality evaluation harness (§8).
- Measured end-to-end latency (§15).
