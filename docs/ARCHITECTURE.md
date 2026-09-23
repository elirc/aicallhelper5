# Architecture

This document describes the system as built. The product rules live in [SPEC.md](SPEC.md). The contract (types, ports, limits) lives in `crates/contract/src/`, which is the source of truth for everything below.

## 1. System map

```
                                   Windows
  default render device ──WASAPI loopback (polling, 10 ms)──┐
                                                            ▼
 ┌──────────────────────── Rust process (Tauri 2) ──────────────────────────────────────────┐
 │                                                                                          │
 │  "audio-wasapi" thread ── raw chunks ──► "audio-worker" thread                           │
 │  (per stream, MTA COM,                  (owns device + DSP: downmix → 16 kHz FIR          │
 │   gap-fills silence)                     resampler → 2048-sample frames + RMS)            │
 │                                                  │ AudioMsg (unbounded FrameSink)        │
 │                                                  ▼                                       │
 │   tokio runtime ┌──────────────────────── callcore-session ───────────────────────────┐   │
 │                 │ audio lane task ◄── AudioOp (Start / Drain / Discard, FIFO)         │   │
 │                 │ forwarder task ── frames ──►┐                                       │   │
 │                 │                             ▼                                       │   │
 │   commands ────►│ SessionHandle ──Cmd──► SESSION ACTOR (single owner of all state) ◄──┼── deadlines
 │   (invoke)      │                         │   ▲ Tagged{gen, TaskEvent}                │   │ (cap/finalize/
 │                 │   writer task ◄─frames──┤   │                                       │   │  first-token/total)
 │                 │   reader task ──────────┼───┘ ◄── SttEvent ◄── callcore-stt ◄── wss://api.deepgram.com
 │                 │   stream task ──────────┘ ◄── deltas ◄── callcore-llm ◄── https://api.anthropic.com
 │                 │                                                          / api.groq.com
 │                 └───────────────┬──────────────────────────────────────────────────────┘ │
 │                                 │ CoreEvent (EventSink::emit, never blocks)              │
 │                                 ▼                                                        │
 │   "event-pump" thread ── EventEnvelope{seq,…} ──► Tauri Channel ──────────────┐           │
 │   StatusHub (revision) ◄── session status watch / protection / core state     │           │
 │   SettingsStore (%APPDATA%\AICallAssistant\settings.json, DPAPI)              │           │
 │   Platform: window, SetWindowDisplayAffinity, hotkey, ShellExecuteW           │           │
 └───────────────────────────────────────────────────────────────────────────────┼───────────┘
                                                                                 ▼
 ┌────────────── WebView2 page (React 19) ──────────────┐
 │ ipc/tauri.ts ─► controller.ts (commands, seq dedupe,  │
 │                 rAF delta coalescing, recovery)       │
 │              ─► reducer (pure) ─► selectView ─► AppView│
 │              ─► components via useApp()               │
 └───────────────────────────────────────────────────────┘
```

Two independent channels connect the page and the core. **Commands** are Tauri `invoke` calls (request/response, `{ok, value} | {ok:false, error}`). **Events** go through one Tauri `Channel` attached by `subscribe_events` (core → page, ordered, `seq`-numbered). They fail independently, so `src-tauri/tests/wiring.rs` runs the real wiring end to end (§14.1).

## 2. Crates and responsibilities

| Crate | Owns | Key public items |
|---|---|---|
| `callcore-contract` | Wire types, ports, limits | `types::*` (ts-rs exported), `ports::*`, `config::*`, `Secret`, `copy::*` |
| `callcore-prompt` | Prompt bytes (§8), `"""` fix, eval harness | `build_prompt`, `PromptInput`, `neutralize_triple_quotes`, `eval::check_answer` |
| `callcore-llm` | Providers, registry, HTTP client, SSE | `Registry::{new, with_origins, infos}`, `AnthropicProvider`, `GroqProvider`, `http_client()`, `sse::SseParser` |
| `callcore-stt` | Deepgram socket | `DeepgramConnector::{new, with_url}`, `TranscriptAccumulator`, `parse_results` |
| `callcore-audio` | Loopback capture + DSP | `LoopbackSource::{new, with_backend, shutdown}`, `dsp::*` |
| `callcore-settings` | Settings file + secrets | `SettingsStore::{open, view, apply_patch, save_bounds, bounds}`, `DpapiKeystore`, `default_dir()` |
| `callcore-session` | The session machine | `SessionHandle::{spawn, start_session, stop_session, ask, cancel_session, status, status_watch, shutdown}` |
| `callcore-shell` | Tauri-free shell logic | `events::EventPump`, `status::StatusHub`, `geometry::restore`, `hotkey::{parse, Registrar}`, `url::validate_external`, `close_guard::CloseGuard`, `diagnostics::render`, `commands::{guarded, CoreGate}` |
| `src-tauri` | Glue | `app::{AppCtx, build_core, <command fns>}`, `commands.rs` (`#[tauri::command]` one-liners), `platform.rs`, `protection.rs`, `logging.rs` |

The Rust types are exported to `src/generated/*.ts` by `cargo test -p callcore-contract` (ts-rs, with `TS_RS_EXPORT_DIR` set in `.cargo/config.toml`). `src/ipc/types.ts` hand-writes the one shape ts-rs can't express (`CmdResult<T>`) and the `CoreApi` interface.

## 3. Ports

Every external dependency of the session machine sits behind a trait in `crates/contract/src/ports.rs`:

| Port | Production implementation | Test fake(s) |
|---|---|---|
| `AudioSource` (`start`, `stop_discard`, `stop_and_drain`) | `callcore_audio::LoopbackSource` over `wasapi::WasapiBackend` | `crates/session/tests/support` `FakeAudio`; `callcore_audio::fake::FakeBackend` (worker level); `BenchAudio` (latency bench) |
| `SttConnector` / `SttSender` | `callcore_stt::DeepgramConnector` (+ its `DeepgramSender`) | `FakeStt`; loopback WS server in `crates/stt/tests/support` |
| `AnswerProvider` + `ProviderRegistry` | `AnthropicProvider`, `GroqProvider`, `callcore_llm::Registry` | `FakeProvider`; loopback HTTP server in `crates/llm/tests/support` |
| `SettingsReader` | `callcore_settings::SettingsStore` | `FakeSettings` |
| `Keystore` | `callcore_settings::DpapiKeystore` (CryptProtectData, entropy `AICallAssistant.v4`, no-entropy fallback on unprotect for v3 blobs) | `FakeKeystore` (reversible XOR, can fail) |
| `EventSink` | `callcore_shell::events::EventPump` | `RecordingSink` |
| `Clock` (epoch ms for `deadlineMs`) | `SystemClock` | `FakeClock` (driven by tokio's paused clock) |
| `DisplayAffinity` | `src-tauri` `protection::HwndAffinity` (behind `Platform::protect_once`) | fake `Platform` in `wiring.rs` |

Durations and timers use `tokio::time`, never the `Clock` port, so tests can pause and advance time.

## 4. Threading

| Thread / task | Kind | Does |
|---|---|---|
| Tauri main (UI) thread | OS | Window events, webview. Window getters and setters hop here. `protect_on_ui_thread` runs here at startup. |
| tokio runtime (Tauri's async runtime) | pool | Commands, the session actor and all its tasks, STT sender/reader, provider streams, protection verify loop, debounced geometry save. |
| **session actor** | task | Single owner of the live-session slot, phase, flags, command tickets and deadlines. Processes, one at a time: `Cmd`s from `SessionHandle`, `Tagged` reports from child tasks, and its earliest deadline (`select!`, biased toward task reports). Publishes `SessionStatus` on a `watch` channel. |
| **audio lane** | task | Issues every `AudioSource` call in the order the actor queued it, so a superseded session's discard always reaches the device before the next start. Bounds: start 10 s, discard 3 s, drain = drain timeout + 0.5 s. |
| per-session child tasks | tasks | `Connect` (STT connect under the 5 s timeout), `Forwarder` (relays capture messages; on drain completion relays the rest, sends `DrainDone`, drops the receiver), `Writer` (frames then CloseStream, FIFO), `Reader` (STT events), `Stream` (one provider attempt). Each report carries the session's generation, and the actor drops stale ones. Aborted through `TaskSet` on every exit path. `GuardedSender` aborts the socket on drop. |
| **audio-worker** | OS | `LoopbackSource`: owns every device object and all DSP state. One FIFO of commands plus captured chunks gives an explicit order. Polls the default render device every 1 s. |
| audio-wasapi | OS, one per open stream | MTA COM thread. Polls `IAudioCaptureClient` every 10 ms (200 ms engine buffer) and fills packet-less gaps with wall-clock silence. Dropping the stream joins it after reading every buffered packet. |
| Deepgram sender / reader | tasks (stt crate) | Sender owns the write half: audio, CloseStream, KeepAlive every 8 s while idle. Reader parses frames as hostile input and emits exactly one terminal event. |
| **event-pump** | OS | Delivers `EventEnvelope`s in queue order to the attached Channel (see §5). |
| blocking pool | `spawn_blocking` | DPAPI key reads (`SettingsReader::get_secret`), settings reads/writes, platform calls (always-on-top, layout, dock, hotkey register/unregister, `ShellExecuteW`, protection apply/verify). |
| exit-watchdog | OS | Started at exit. Forces `process::exit(0)` after `SHUTDOWN_BUDGET` + 2 s. |

The audio callback side does minimal work: it converts the chunk and sends it down the worker's channel, with no lock. A panic in chunk processing is caught and drops only that chunk. The only audio lock is the sink slot, and no device call ever happens while it is held (§14.4).

## 5. Event pump, `seq` and `revision`

**Pump** (`crates/shell/src/events.rs`):
- `emit` never blocks: a short mutex section and no I/O. The session actor and window callbacks call it.
- Every accepted event gets a strictly increasing `seq`, per process. Attach, detach and delivery failure never reset it.
- One worker thread delivers in queue order to the attached `Delivery`, which in production is the page's Tauri `Channel`.
- Backpressure is a soft bound: 1 024 events while attached, 4 096 while detached. Over it, only `audio:level` is dropped. `audio:level` also coalesces latest-wins per session in place, keeping its seq. Must-deliver events (`llm:done`, `session:error`, `protection:*`, `core:*`, `window:close-requested`, `session:autostopped`, `audio:device`) and ordered content (`stt:partial`, `llm:delta`) are kept. A 50 000-event hard cap exists only so a page that never re-attaches can't grow memory forever. Past it, the oldest non-must-deliver events go.
- `subscribe_events` replaces the target (page reload). A delivery failure detaches only the target that failed and puts the envelope back at the head, so the next attach flushes it. There is deliberately **no** global failure counter that could ratchet shut (§14.6).

**Page side** (`src/app/controller.ts`, `src/state/reducer.ts`): envelopes with `seq <= lastSeq` are dropped, side effects included. `lastSeq` is 0 once per page load and is never reset by snapshots or `core:ready` (§14.7). Session events for an id that isn't current are dropped (§5.3). Events that arrive before a start/ask reply are buffered and replayed only if the reply adopts that id.

**Revision** (`crates/shell/src/status.rs`, `StatusHub`): one counter bumps on every actual change of core state, protection verdict or session status. The actor's `status_watch` is mirrored into the hub by a watcher task. `core:ready`, `core:failed`, `protection:ok` and `protection:failed` carry the revision returned by the setter. The page adopts a snapshot or event only if its revision is ≥ the last one it saw. `get_status` answers within `STATUS_TIMEOUT` (2 s). If the session can't be read in time, it answers with phase `unknown` at the *current* revision, which is not a change. On load the page subscribes first, then calls `get_status` and re-adopts a live session, including its `recording.deadlineMs`.

## 6. Startup sequence

From `src-tauri/src/lib.rs` `run()` / `setup()`:

1. Logging: `%APPDATA%\AICallAssistant\logs\aica.log`, rotating 1 MB × 3, plus an in-memory 200-line tail. The panic hook writes a redacted line.
2. Build info (`AICA_GIT_REV`, `AICA_GIT_DIRTY` incl. untracked files, `AICA_BUILD_TIME` from `build.rs`).
3. Provider registry and `SettingsStore::open`. This is cheap and happens before the window, because geometry and always-on-top come from the file. A panic here leaves the store absent, and the core later fails with an actionable error.
4. Tauri builder: single-instance (a second launch focuses the first and re-applies protection), global-shortcut (on press, emits `hotkey:toggle`), commands, window-event handler.
5. `setup`: restore geometry (`geometry::restore`), build the window **hidden** at those bounds, apply `WDA_EXCLUDEFROMCAPTURE` on the UI thread, then **show** and focus it (logged as `window shown` with ms since launch).
6. Spawn `verify_protection` with retries at 0/100/300/1000/3000 ms. The verdict stays `unknown` until Windows' read-back confirms `0x11`, or the retries run out and it becomes `unprotected`.
7. Spawn the heavy core, wrapped in `catch_panic`: start the audio worker thread (`LoopbackSource::new` on the blocking pool), create the Deepgram connector, then `build_core` (session actor with the pump as sink, status watcher). Then `mark_core_ready` → `core:ready`, apply the hotkey, and emit `settings:changed`. On failure: `mark_core_failed` → `core:failed`.
8. Commands that need the core wait on `CoreGate` for up to `CORE_READY_WAIT` (25 s) while it is starting (after that, `copy::CORE_STARTING`). Once it has failed they return `copy::CORE_FAILED` immediately. `get_status`, `cancel_session`, `set_close_guard`, `subscribe_events`, `dock_window`, `open_external` and `get_diagnostics` don't need the core.

Exit (`RunEvent::Exit`): cancel the live session (discard), stop the actor, abort the watcher and stop the audio worker. All of it is bounded by `SHUTDOWN_BUDGET` (3 s), and the watchdog forces exit if anything still hangs.

## 7. Stop → answer timeline and metrics

```
 user presses Stop
   │  stop_session(id) ──► actor: accepted only for the live, not-yet-stopping record session
   ├─ t0 = accepted_at  ◄─────────────── latency clock starts HERE (before the drain)
   │  phase → finalizing, cap cleared, provider.prewarm()
   │  (if the STT socket is still connecting: wait for it — bounded by the 5 s connect timeout)
   ├─ t1 = drain_started ◄─── audio lane, immediately before AudioSource::stop_and_drain
   │  worker: stop device input, flush resampler + final partial frame into the sink (≤ 2 s)
   │  forwarder relays every drained frame, then DrainDone
   ├─ t2 = drain_done   ◄─── actor on DrainDone: capture cutoff (later frames rejected),
   │                          CloseStream queued BEHIND every frame, 5 s finalize deadline armed
   │  Deepgram flushes, closes 1000 → SttEvent::Flushed{transcript}
   ├─ t3 = flushed_at   ◄─── actor: STT done (writer/reader aborted); empty → no_speech (no LLM call)
   │  build_prompt + build_request ONCE, 10 s first-token + 60 s total deadlines armed, stream task spawned
   │  (Connect failure before any delta → one retry with the same bytes, same deadlines)
   ├─ t4 = first_delta_at ◄── actor on the first non-empty delta (first-token deadline disarmed)
   │  llm:delta … (page coalesces to one paint per animation frame)
   └─ t5 = stream Ok with the provider's real terminal signal → llm:done{…, metrics}

 audioDrainMs  = t2 − t1      (excludes any STT-connect wait: t1 is when the drain really starts)
 sttFinalizeMs = t3 − t2
 firstTokenMs  = t4 − t0      (includes the drain; = totalMs if no delta)
 totalMs       = t5 − t0
 visibleFirstWordMs (frontend only) = Stop/Ask click → first painted answer text (performance.now())
```

For **Ask**, t0 is ask acceptance, the session starts directly in `answering`, and `audioDrainMs = sttFinalizeMs = 0`. For the 120 s cap and for device loss, the same stop path runs. The cap emits `session:autostopped` first. Device loss emits `audio:device{lost}` instead.

Checks: `crates/session/tests/metrics.rs` (paused clock; injecting a drain delay D grows `firstTokenMs` by exactly D) and the real-clock benchmark `crates/session/examples/latency_bench.rs`.

## 8. Content protection

- `Platform::protect_once` calls `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE = 0x11)` and then `GetWindowDisplayAffinity`. The verdict is `Protected` **only** when the read-back is exactly `0x11` (`protection::verdict_from_readback`). An apply error still verifies, because the read-back is the source of truth. A failed read is `Unprotected`.
- When it runs:
  - Before the first show.
  - After the show, with retries at 0/100/300/1000/3000 ms.
  - On every `subscribe_events` (page load or reload; always published).
  - On window focus, on a second-instance launch, and after every layout switch (retries at 0/100/300 ms; published only on change).
- The `StatusHub` starts `unknown`. Intermediate failures within one retry series are not published. The final verdict goes out as `protection:ok` or `protection:failed` with the hub revision, and it is a must-deliver event.
- There is **no command** that sets the verdict. The page only renders it: `ProtectionBadge` in every view (full header, prompter strip, settings). `src-tauri/capabilities/default.json` grants the page no plugin or core window permissions.

## 9. Settings persistence

- File: `%APPDATA%\AICallAssistant\settings.json` (v3's path), pretty camelCase JSON with `"version": 4`. The schema, load rules and every accepted legacy shape are documented at the top of `crates/settings/src/schema.rs`.
- **Load** validates per field: an invalid field falls back alone and the rest of the file is kept. A missing file is `first_run`. An unreadable file (IO error) or a corrupt one (not a JSON object) loads defaults and records a `SettingsLoadIssue`.
- **Backup before the first write of any kind** (a patch *or* a geometry autosave): `settings.json.<corrupt|unreadable>-<yyyyMMddTHHmmss>-<random6>.bak`. If the backup fails, `writesBlocked` is set and every later write is refused, even if the disk recovers. Merely opening and viewing never touches the file. Note that closing saves window geometry, which counts as a write and therefore backs up first.
- **Writes** are serialized behind one mutex and atomic (temp file + fsync + rename with retry). `apply_patch` requires `baseRevision == settingsRevision`. A stale patch is rejected and nothing is applied. Each successful patch bumps the revision by exactly 1. Geometry saves (`save_bounds`) don't bump it, so an open Settings form stays valid.
- **Side effects** run under `AppCtx.settings_lock`: the next save starts only after this one's hotkey re-register, always-on-top change and layout switch have finished.
- **Secrets** are write-only from the UI (`SecretAction::Set | Remove`). They are DPAPI-encrypted as `dpapi:<base64>`. A DPAPI failure refuses the whole patch and the previous key is untouched. A blank Set is a no-op. Legacy `plain:` keys are encrypted in memory at load and reach disk encrypted on the first write. Undecryptable blobs read as unset (`KeyStorage::Unreadable`) and are preserved verbatim. The view carries only `KeyStatus { id, label, hasKey, storage, getKeyUrl }`.
- **Geometry** is saved per layout (`windowBounds`, `prompterBounds`), debounced 500 ms after move/resize, synchronously on close, and immediately after Dock. A saved position is reused only if a ≥ 40 px-wide, ≥ 16 px-tall title strip is on some monitor's work area. Otherwise the window docks top-centre of the current monitor.

## 10. Frontend architecture

```
main.tsx ─ pickApi(): Tauri → createTauriApi() | plain browser → createFakeApi({demo:true})
App ─ <AppProvider api> ─ <Root/>
AppProvider: useReducer(reducer) + createController({api, dispatch}) + 250 ms tick while recording
             value = { view: selectView(state, now), actions: controller.actions }
Root: settings screen | FullLayout | PrompterLayout   (components read only useApp())
```

- **`src/ipc/`**: `tauri.ts` is the real `CoreApi`. It never rejects: thrown errors and a 30 s client-side timeout map to `internal`, responses are shape-checked, unknown error codes are coerced to `internal`, and `subscribe_events` is retried up to 3 times. `fake.ts` is a scriptable fake core with a demo mode.
- **`src/state/`**: `reducer(state, action)` is pure, and every time value arrives on the action. It handles the session phase machine, command generations (§14.2), seq and stale-id filtering, buffering before the reply, revision adoption, history (6 entries, view-only) and notices. `selectView(state, now)` derives the pinned `AppView`, including the status line copy (`copy.ts`), silence detection (5 s below RMS 0.01), button enablement and the hotkey hint.
- **`src/app/controller.ts`** owns every side effect:
  - commands with generations (a late result that lost the race cancels only the session it created);
  - event intake (seq dedupe; `llm:delta` coalesced to one dispatch per animation frame; any other event flushes buffered deltas first);
  - hotkey handling (it only STOPS while Settings is open);
  - refused-stop recovery (§14.3: scoped to the session, disarmed by progress, `unknown` means wait, bounded at 5 polls, ends with `cancel_session`);
  - serialized settings saves (`baseRevision` read when each save runs; a stale rejection refetches settings and keeps the draft);
  - close-guard sync.
- **`src/app/view.ts`** is the pinned seam. Components never touch the reducer or the IPC.
- **`src/components/`**: full layout (header, answer panel with sticky auto-scroll and latency chip, question, controls, history), prompter strip (anchored text, "▼ more"), settings (keys, profile editor, answers, shortcut/window, about, unsaved-changes bar). `ProtectionBadge` appears in every view.
- **`src/markdown/`**: a safe subset renderer. Every string is a text node (no `innerHTML`), links are not rendered, blocks are memoized, output is identical whether streamed or whole (property-tested), and an error boundary falls back to plain text.
- CSP (`tauri.conf.json`): `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'none'; connect-src ipc: http://ipc.localhost`. Navigation away from the app origin is blocked (`url::is_app_origin`).

## 11. Decisions log

- **Tauri 2 instead of Electron or pywebview.** WebView2 is already on Windows 11, so the installer and memory stay small (targets: installer < 15 MB, < 150 MB RAM). A Rust core gives real threads for audio, and `windows`-crate access for `SetWindowDisplayAffinity`, DPAPI and WASAPI. Tauri's command + Channel model maps directly onto the two-channel contract.
- **ts-rs generated types.** The Rust contract is the single source of truth. The TS types are generated, CI fails if `src/generated` drifts, and no hand-maintained duplicate can go stale. `CmdResult<T>` is the one hand-written TS type, because ts-rs can't express it.
- **Per-crate split behind ports.** Each external dependency (device, socket, HTTP provider, settings, keystore, window) is a trait. The session rules can then be tested with fakes on a paused clock (71 deterministic session tests). Each area has its own crate and test suite, and several agents could build them in parallel against pinned APIs. `callcore-shell` keeps the shell's logic Tauri-free, so it is unit-testable. `src-tauri` is glue plus one integration test that drives the real wiring.
- **One actor, deadlines instead of timer tasks.** All session state lives in one task. Timers are `Option<Instant>` fields that the select loop sleeps on, so dropping the session state clears every timer on every exit path (invariant 9). Child tasks report tagged with a generation, so stale results are dropped by construction (invariants 2 and 3).
- **Direct WASAPI polling instead of cpal's event mode.** cpal 0.15 opens loopback event-driven. On real hardware that delivered about 0.13 s of audio out of 3 s, because loopback clients can't rely on the buffer event. `callcore-audio` therefore talks to WASAPI directly through the `windows` crate. It polls every 10 ms on a dedicated MTA thread and fills packet-less gaps with wall-clock silence (loopback sends nothing while nothing plays, and Deepgram must see time pass). Dropping a stream joins the thread only after reading every buffered packet, which is what the capture cutoff relies on.
- **Drain before CloseStream (capture cutoff).** v3 flushed only the resampler remainder and lost the audio tail. v4 stops input, delivers every pre-Stop sample including the final partial frame, and only then queues CloseStream behind all audio. Frames after the cutoff are rejected.
- **The `"""` fix.** The user message quotes the transcript between `"""` lines, so a transcript containing `"""` could close the quote and inject instructions. `neutralize_triple_quotes` inserts U+2060 WORD JOINER between adjacent quotes in every run of 3 or more `"`. Without `"""` the bytes are exactly the spec format. The change is lossless (strip U+2060 to recover the text), and the delimiters sit on their own lines, so edge quotes can't merge with them.
- **Supersede and cancel are silent.** A superseded or cancelled session emits nothing further, not even a terminal event. The page started the new command or the cancel and has already moved on, and every other session still ends with exactly one `llm:done` XOR `session:error`.
- **Latency clock at Stop acceptance.** The user waits through the drain and the STT flush, so those count. `audioDrainMs` starts when the drain actually starts, so a Stop during connect doesn't hide the connect wait inside it or attribute it wrongly (§18).
- **App manifest embedded by the linker.** `src-tauri/windows-app-manifest.xml` declares Common Controls v6 and **per-monitor DPI v2** up front (mixed 100/125/150 % setups). `build.rs` embeds it with `/MANIFEST:EMBED` for every binary, test binaries included. tauri-build's default embeds the manifest only into the app, which makes the `tauri::test` integration tests fail to load.
- **No `panic = "abort"` in release.** A panic inside one session task must only fail that answer. Command handlers and the core build are wrapped in `catch_panic`.
