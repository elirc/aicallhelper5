# Changelog

All notable changes to AI Call Assistant. Versions come from the Cargo workspace version (the one version source, checked by `tools/check-version.mjs`).

## 4.0.0 — unreleased

A ground-up rebuild of the v3.1.0 Python + pywebview app. Product behavior, UX, prompts and reliability rules carry over. The architecture, stack and test suite are new.

### Stack
- **Shell:** Tauri 2 (Rust) on WebView2, with an NSIS per-user installer (no admin), single instance, and the global-shortcut plugin.
- **Core:** Rust + tokio, split into crates behind ports (`crates/contract/src/ports.rs`):
  - `contract`: wire types exported to TypeScript with ts-rs
  - `prompt`
  - `llm`: reqwest streaming + a hand-written incremental SSE parser, one pooled client, pre-warm
  - `stt`: Deepgram over tokio-tungstenite
  - `audio`: direct WASAPI loopback polling through the `windows` crate, with an anti-aliased, phase-continuous 16 kHz resampler
  - `settings`: DPAPI secrets that fail closed
  - `session`: a single-owner actor
  - `shell`: Tauri-free shell logic
- **Frontend:** React 19 + TypeScript (strict) + Vite and plain CSS with design tokens. A pure reducer plus a side-effect controller, and a safe streaming Markdown renderer (text nodes only, no links).
- **IPC:** Tauri commands with a `{ok, value} | {ok:false, error}` envelope, plus one ordered event Channel with a monotonic `seq`. The CSP allows IPC only. All network I/O is in Rust.
- **Tooling:** `cargo fmt`/`clippy -D warnings`/`test`, `tsc --noEmit`, ESLint, Vitest + Testing Library, `cargo audit`, `npm audit`, one version source, and a GitHub Actions `windows-latest` pipeline that builds the installer.

### Carried over from v3 (unchanged behavior)
- Push-to-record flow: live Deepgram transcript, Stop & Answer, streamed answer under the camera.
- Six call types, three answer styles, and byte-identical prompts (spec §8, pinned by 54 snapshots plus a spec-conformance test that parses `SPEC.md`).
- Anthropic Claude Haiku 4.5 (default) and Groq GPT-OSS 120B (fastest) providers.
- Profiles (up to 20), prompter mode, docking, the global hotkey, and the 6-entry history (single-turn, never sent as context).
- Screen-capture exclusion (`WDA_EXCLUDEFROMCAPTURE`) with an honest tri-state indicator in every view.
- The same settings path, `%APPDATA%\AICallAssistant\settings.json`. v3 files migrate automatically: legacy top-level `resume`/`jobDescription` become a "Default" profile, snake_case aliases are accepted, `enc:` blobs are read, and `plain:` keys are re-encrypted and removed from disk on the first write.

### v3 gaps fixed (spec §18)
- **Triple quotes in the transcript can no longer break the prompt's quoting.** Runs of three or more `"` get U+2060 WORD JOINER between them, so a transcript can't close the `"""` section and inject instructions. Transcripts without `"""` produce exactly the old bytes.
- **Mid-recording device unplug is detected.** The recording auto-stops and answers with what was captured (`audio:device {kind:"lost"}`, "The system audio device was disconnected during recording. Answering with what was captured.").
- **A default-device switch is followed.** Capture moves to the new default output device (`audio:device {kind:"changed"}`).
- **Correct `audioDrainMs` when Stop lands during the Deepgram connect.** The drain waits for the socket, and `audioDrainMs` measures only the real drain. The latency clock still starts at Stop acceptance, so `firstTokenMs`/`totalMs` include the wait.
- **A re-adopted session keeps its countdown.** After a page reload, `get_status` returns the live session with its `recording.deadlineMs`, and the UI resumes the same deadline the core enforces.
- **Prompt-quality evaluation harness.** `cargo run -p callcore-prompt --example prompt_eval` builds 27 fixtures × 3 styles offline, and `eval::check_answer` scores real answers.
- **Measured end-to-end latency.** Every answer carries `audioDrainMs`, `sttFinalizeMs`, `firstTokenMs` and `totalMs` (latency chip and tooltip) plus a frontend "visible first word" measurement. A real-clock benchmark (`crates/session/examples/latency_bench.rs`) injects known delays and checks the metric definitions.

### Reliability fixes built in (lessons from v1–v3, each pinned by a regression test)
- An integration test drives the real app wiring against a real Tauri Channel, so "every command works but no event reaches the page" can't recur.
- Command generations: a late start/ask result can't null, flip or hijack a newer session, and it cancels only the session it created.
- Refused-stop recovery is scoped to its session, disarmed by progress, bounded, and treats `unknown` as "wait again".
- Capture cutoff: every pre-Stop sample (including the final partial frame) is drained before CloseStream, and nothing follows it. Cancel uses discard, never drain.
- The latency clock starts before the drain, so latency is never hidden.
- The event pump never drops terminal or protection events, has no ratcheting failure counter, and recovers after a renderer reload. Page-side `seq` bookkeeping resets exactly once per load.
- A corrupt or unreadable settings file is backed up before the first write of any kind. If the backup fails, all writes are refused. Closing untouched never destroys the file.
- DPAPI encryption fails closed and never silently stores plaintext.
- Saves are ordered by `settingsRevision`. A stale save is rejected and nothing is applied.
- A device-open failure plus an early Stop reports the device error, not "no speech".
- Keys and profile text are kept out of Debug output, logs, errors, panics and diagnostics.
- Timing tests wait for conditions (paused clock for timers, real time for sockets). No fixed sleeps.

### New in v4
- Direct WASAPI polling capture. cpal's event-mode loopback delivered only about 4 % of samples on real hardware. Silence is gap-filled while nothing plays.
- `get_status` never blocks on a busy core (it answers phase `unknown` within 2 s). Commands resolve to `internal` after 30 s instead of hanging.
- Startup shows the window first and builds the core afterwards. Readiness is reported explicitly (`core:ready` / `core:failed`) with actionable copy.
- Close guard: unsaved Settings changes cancel the first close once. A second close within 10 s, or an unresponsive page, always closes, and exit is bounded at 3 s.
- Honest hotkey status (registered / disabled / invalid / unavailable), and late OS registrations are undone.
- Geometry restore across multi-monitor, negative-coordinate and mixed-DPI setups, with per-layout bounds.
- Rotating log (`%APPDATA%\AICallAssistant\logs`) and a redacted **Copy diagnostics**.
- Embedded build info (version, git revision, dirty flag including untracked files, build time).
