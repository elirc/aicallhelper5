# Rules for build agents (read fully before starting)

Repo: `C:\Users\Owner\Desktop\aicallhelper5` (Windows 11, Git Bash + PowerShell available).
Product spec: `docs/SPEC.md` — read ALL of it. Contract (source of truth): `crates/contract/src/{types,ports,config,secret}.rs`.
Generated TS types: `src/generated/*.ts` (never hand-edit; regenerate with `cargo test -p callcore-contract`).

## Ownership (enforced)
- Edit ONLY the files/directories your brief assigns you. Several agents work in parallel in this same tree.
- Do NOT edit: `crates/contract/**`, the root `Cargo.toml`, `package.json`, `package-lock.json`, `tsconfig.json`, `vite.config.ts`, `src/generated/**`, `src/ipc/types.ts`, `src/app/view.ts`, `src/app/testing.ts`, `docs/SPEC.md`, this file.
- Pinned public APIs (the `lib.rs` stubs that say "PUBLIC API PINNED", `src/ipc/types.ts`, `src/app/view.ts`) must keep their signatures. You may ADD items. If you truly need a pinned signature or a contract type changed, finish everything else, then describe the exact change in your final report under "CONTRACT REQUESTS" (don't make it yourself).
- You may add dependencies only to YOUR crate's `Cargo.toml`, preferring `{ workspace = true }` entries. Never run `npm install`/`npm ci` (already done; ask in your report if you need a package).
- Do not `git commit`, `git add`, `git stash` or `git checkout` anything — the orchestrator commits.

## Building / testing
- Rust crates under `crates/`: ALWAYS `export CARGO_TARGET_DIR=target-core` (Git Bash) before cargo commands, so you don't block the Tauri shell build in `target/`. Cargo may print "Blocking waiting for file lock" while another agent builds — that is normal, just wait.
- `src-tauri` uses the default `target/` dir.
- Gates for a Rust crate: `cargo fmt -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`. All must pass.
- Frontend gates: `npx tsc --noEmit`, `npx vitest run <your paths>`, `npx eslint <your paths>` (once eslint config exists).
- Background shell tasks on this machine get killed after ~10 min. Run builds in the foreground with a long timeout (up to 600000 ms); if a build is cut off, just re-run it (the cache resumes).
- `cmd | tail` reports the PIPE's exit code. Use `cmd > log 2>&1; echo "exit $?"` or `${PIPESTATUS[0]}`.
- If a test looks flaky, run it at least twice. Never weaken a test to make it pass.

## Test rules (spec §16)
- No test touches the network (except 127.0.0.1 loopback servers you start in-test), a live provider or an audio device.
- Timing tests wait for conditions; never sleep a fixed wall-clock time. For timers use `tokio::time::pause()` + `advance`. Gotcha: paused clock + REAL loopback sockets race tokio's auto-advance on Windows — don't mix them; in socket tests use real time with generous bounds or explicit synchronization.
- Keep secrets and profile/prompt text out of Debug, logs, errors and panics — add tests for that where relevant.
- For every test you write, add a bullet to `docs/testing/<your-area>.md`: `- \`test_name\` — what it verifies — why it exists`.

## Reference
An older, different-contract Tauri build lives at `C:\Users\Owner\Desktop\aicallhelper`. You MAY look at it for Windows API / crate-version specifics (e.g. `windows` 0.58 feature names, cpal loopback, DPAPI calls). Write your own code to THIS spec; do not copy its architecture.
Crate versions available offline in the cargo cache: tokio 1.53, reqwest 0.12, tokio-tungstenite 0.24, cpal 0.15, windows 0.58, tauri 2.11, thiserror 2, ts-rs 10, tempfile 3. Network is available but prefer these.

## Final report (your last message)
1. What you built (files), 2. public API summary, 3. exact gate commands run + results (pass counts), 4. known gaps / TODOs, 5. CONTRACT REQUESTS (if any).
