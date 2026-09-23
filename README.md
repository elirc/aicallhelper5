# AI Call Assistant v4

AI Call Assistant is a push-to-record call copilot for Windows. During a video call you press **Record** while the other person is talking. The app captures **system output audio** (WASAPI loopback of the default playback device, never the microphone) and streams it to Deepgram for a live transcript. When you press **Stop & Answer**, it streams a suggested reply from an LLM: what you could say next, in the first person, in spoken English, grounded in your active profile (resume, job description, focus, notes, call type). The answer appears at the top centre of the screen, just under the webcam, so you read it while looking at the camera. The window is **excluded from screen capture**, so it does not show up in Zoom, Teams, Meet or OBS shares. There is no account, no backend of ours and no telemetry: you bring your own API keys.

v4 is a ground-up rebuild of the Python/pywebview v3.1.0 on Tauri 2 (Rust core) with a React/TypeScript front end. See [docs/CHANGELOG.md](docs/CHANGELOG.md).

## Features

- **Record → Stop & Answer.** Live transcript while recording, and the answer streams word by word. The design target is about 1 s from Stop to the first answer word. The latency chip shows the measured value.
- **Type a question instead.** Ask answers a typed question directly, with no audio and no Deepgram key needed. **Regenerate** re-asks the question you are viewing and adds the result as a new history entry.
- **Profiles and call types.** Up to 20 profiles, each with a call type: behavioral interview, technical screen, system design, recruiter screen, sales or customer call, or general meeting. Each call type has its own prompt.
- **Answer styles.** Brief, Balanced or Detailed. This is a global setting, and you can switch it mid-call.
- **Prompter mode.** A short, wide strip docked at the camera line, showing only the answer in large text. The text doesn't jump while it streams.
- **Screen-share protection** with an honest indicator in every view: "Hidden from screen capture", "Screen-share protection not confirmed yet" or a red alert.
- **Global hotkey** (default `Ctrl+Shift+Space`) toggles Record/Stop from any app.
- **Device handling.** If the playback device is unplugged mid-recording, the app answers with what it captured. If the default device changes, capture follows it.
- **Safe by construction.** Keys are encrypted with DPAPI and the encryption fails closed. Corrupt settings files are backed up before anything is written. Answers are single-turn.

## Requirements

- Windows 10 version 2004 (build 19041) or later, or Windows 11. Screen-capture exclusion (`WDA_EXCLUDEFROMCAPTURE`) needs 2004+.
- Microsoft Edge **WebView2** runtime. It ships with Windows 11. On Windows 10, the installer downloads it if it's missing (Tauri's default bootstrapper mode, so that install needs internet).
- A **Deepgram** API key (speech-to-text, needed to record).
- An **Anthropic** API key (the default provider, Claude Haiku 4.5) **or** a **Groq** API key (the "fastest" preset, `openai/gpt-oss-120b`). You choose the provider in Settings.

To build from source you also need a stable Rust toolchain (MSRV 1.80, MSVC target), Node.js 22 and npm.

## Quick start (development)

```bash
npm ci                 # install the pinned JS toolchain (package-lock.json)
npx tauri dev          # runs `npm run dev` (Vite on http://localhost:5173) + the Rust shell
npx tauri build        # release build + NSIS per-user installer
```

`npx tauri build` writes the installer to `target/release/bundle/nsis/` (the workspace-level `target/` directory). The installer is per-user (`installMode: currentUser`), so it needs no admin rights.

`npm run dev` on its own serves the UI in a plain browser against a scripted fake core (`src/ipc/fake.ts`, demo mode). You can click through the whole flow there without Rust, keys or audio.

## Repository layout

| Path | Crate / package | Responsibility |
|---|---|---|
| `crates/contract` | `callcore-contract` | Source of truth: every core ↔ UI type (`types.rs`), the ports (traits) every external dependency sits behind (`ports.rs`), all timeouts and limits (`config.rs`), and the redacting `Secret`. Exported to TypeScript with ts-rs. |
| `crates/prompt` | `callcore-prompt` | Byte-exact prompt construction (spec §8), the `"""` fix, and the offline prompt-eval harness (`examples/prompt_eval.rs`). |
| `crates/llm` | `callcore-llm` | Answer providers (Anthropic, Groq), the provider registry, the shared pooled HTTP client, the incremental SSE parser, failure copy and pre-warm. |
| `crates/stt` | `callcore-stt` | Deepgram streaming WebSocket client: sender/reader tasks, KeepAlive, strict Results parsing, transcript accumulator and close classification. |
| `crates/audio` | `callcore-audio` | WASAPI loopback capture on one dedicated worker thread (polling backend), DSP (downmix, anti-aliased 16 kHz resampler, 2048-sample frames, RMS), device-loss and default-device-change detection. |
| `crates/settings` | `callcore-settings` | `settings.json` store: per-field validation, v3 migration, atomic revision-checked writes, backup-before-first-write, and DPAPI secrets that fail closed. |
| `crates/session` | `callcore-session` | The session actor: one live session, supersede/cancel, capture cutoff and drain, STT finalize, answer streaming with watchdogs and retry-once, and metrics. |
| `crates/shell` | `callcore-shell` | Pure, Tauri-free shell logic: ordered event pump, status revision hub, geometry, hotkey parsing and registrar, URL validation, close guard, diagnostics, build info and the command-boundary guards. |
| `src-tauri` | `aicallassistant` | Thin Tauri 2 glue: window, content protection, commands, logging, platform calls, startup and shutdown. |
| `src/` | npm package | React 19 UI: `ipc/` (Tauri adapter and fake core), `state/` (pure reducer and selectors), `app/` (controller, `AppProvider`, pinned `view.ts` seam), `components/`, `markdown/` (safe streaming renderer), `styles/`, and `generated/` (ts-rs output, never hand-edit). |
| `tools/check-version.mjs` | | Fails on version drift between Cargo, `package.json`, `package-lock.json` and `tauri.conf.json`. |
| `docs/` | | [SPEC](docs/SPEC.md), [ARCHITECTURE](docs/ARCHITECTURE.md), [TESTING](docs/TESTING.md), [TROUBLESHOOTING](docs/TROUBLESHOOTING.md), [USER-GUIDE](docs/USER-GUIDE.md), [RELEASE-CHECKLIST](docs/RELEASE-CHECKLIST.md), [CHANGELOG](docs/CHANGELOG.md). |

## Gates

CI runs these on `windows-latest` (`.github/workflows/ci.yml`). Run them locally before pushing:

```bash
node tools/check-version.mjs                        # one version source (Cargo workspace version)
cargo test -p callcore-contract                     # also regenerates src/generated/*.ts (ts-rs)
git diff --exit-code -- src/generated               # generated bindings must be committed
cargo fmt --all -- --check
npm run build                                       # tsc --noEmit + vite build (tauri-build needs dist/)
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npx tsc --noEmit
npx eslint src
npx vitest run
cargo audit                                         # CI installs cargo-audit
npm audit --omit=dev
```

The ts-rs export directory is set in `.cargo/config.toml` (`TS_RS_EXPORT_DIR = src/generated`), so any `cargo test -p callcore-contract` rewrites the bindings. After changing a contract type, run it and commit the diff.

When several builds run in the same tree, point the core crates at their own target directory so they don't block the Tauri build: `export CARGO_TARGET_DIR=target-core` (Git Bash) before `cargo test -p <crate>`.

### Latency benchmark

```bash
export CARGO_TARGET_DIR=target-core
cargo run -p callcore-session --example latency_bench --release
cargo run -p callcore-session --example latency_bench --release -- --cycles 50 --drain 40 --finalize 150 --first-token 300
```

The benchmark drives the **real** session actor through N record → stop → answer cycles against in-process fakes that inject known delays (drain, STT finalize, first token, per-delta). It runs on the real clock and prints p50/p90/min/max of `audioDrainMs`, `sttFinalizeMs`, `firstTokenMs` and `totalMs` taken from the `llm:done` metrics. It then runs two checks:

- **Check 1 (pass/fail):** the p50 session overhead, `firstTokenMs − audioDrainMs − sttFinalizeMs − the provider fake's measured wait`, must be within ±15 ms (the default). This proves the latency clock starts at Stop acceptance, before the drain.
- **Check 2 (warning only):** p50 `firstTokenMs` is compared with the *configured* drain + finalize + first-token. On a loaded machine the fakes oversleep, so this check only warns.

The exit code is 1 if a cycle fails or check 1 fails. Flags: `--cycles --drain --finalize --first-token --delta --deltas --record --tolerance`. See [docs/TESTING.md](docs/TESTING.md#latency-benchmark).

### Other runnable examples

- `cargo run -p callcore-prompt --example prompt_eval`: builds every eval fixture × style offline, prints a size table and writes the prompts to `crates/prompt/eval/out/` (gitignored).
- `cargo run -p callcore-audio --example capture_probe`: manual QA on real hardware. It captures the default playback device for 3 s through the full worker and DSP path. Play audio while it runs.
- `cargo run -p callcore-audio --example raw_probe`: manual QA. It drives the WASAPI backend directly for 3 s and counts packets at the device rate.

## Add a provider

The session, retry, metrics and UI are provider-agnostic. To add, for example, an OpenAI-compatible provider:

1. **One module in `crates/llm/src/`**, say `acme.rs`, modelled on `groq.rs` (the template for OpenAI-compatible APIs):
   - Add constants `ID`, `DISPLAY_NAME`, `KEY_ID`, `MODEL` and a `pub(crate) const META: ProviderMeta` (brand name, model, the alternative to suggest when the model is gone, and a `model_unavailable(status, body)` predicate).
   - Add a struct holding the shared `reqwest::Client`, its origin and a `Throttle`, and implement `callcore_contract::ports::AnswerProvider`:
     - `build_request` is pure and builds the body ONCE. The key goes in a `HeaderValue::Secret`.
     - `stream` calls `http::run_stream(&self.client, &META, req, deltas, handler)` with a `StreamHandler` that emits text deltas and reports the provider's real terminal signal through `terminal()`.
     - `prewarm` calls `http::prewarm(...)`.
   - Declare the module in `lib.rs` (`pub mod acme;`).
2. **One line in the registry.** Add `Arc::new(AcmeProvider::new(client.clone(), ACME_ORIGIN))` to the provider list in `Registry::with_origins` (`crates/llm/src/lib.rs`), which `Registry::new` calls. The first entry stays the default. Its `ProviderInfo` (`{id, displayName, keyId, model}`) then reaches the settings view automatically. Settings validates `llmProvider` against the registry, and the key list gains a row for the new `keyId`.
3. **One `key_meta` entry in settings.** In `crates/settings/src/lib.rs` `key_meta()`, map the new key id to its label and "Get a key" URL (https). Without it, the key row shows the raw id and no link.
4. Add a conformance test file like `crates/llm/tests/groq.rs` (loopback server, hostile chunking, status matrix, retry bytes) and bullets in `docs/TESTING.md`.

Error mapping, retry-once, the watchdogs and metrics need no changes.

## Privacy

- **No telemetry, no account, no backend of ours.** Network traffic goes only to Deepgram (`wss://api.deepgram.com`) and to the answer provider you selected (`https://api.anthropic.com` or `https://api.groq.com`). This includes a keyless pre-warm `GET /v1/models` to open a pooled connection. All network I/O happens in Rust. The page's CSP allows only IPC.
- **API keys** are encrypted with Windows DPAPI (current-user scope) in `%APPDATA%\AICallAssistant\settings.json`. If encryption fails, the save is refused and your previous key is kept. Keys never enter the UI (only "saved" status does), logs, errors or diagnostics.
- **Profiles** (resume, job description, focus, notes) are stored as **plain text** in the same file. The Settings screen says so.
- **Single-turn.** Each answer is sent only the current transcript (or typed question) plus your active profile. Earlier questions and answers are never sent as context. History is view state only and is never written to disk.
- **Logs** (`%APPDATA%\AICallAssistant\logs\aica.log`, rotated) hold codes, phases and timings, never keys, transcripts or profile text. "Copy diagnostics" runs a second redaction pass.
