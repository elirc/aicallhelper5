# Release checklist

Copy this list into the release PR or issue and tick every box. A release ships only when **all automated gates are green and every native check has passed**. Content protection is the product's moat, so any failure in §2.1 blocks the release.

Record for the release: version `____`, git revision `____`, installer file name and SHA-256 `____`, tester `____`, date `____`, Windows builds tested `____`.

## 1. Automated gates

Run on a clean checkout of the release commit, where `git status` shows nothing. The build embeds a dirty flag that includes untracked files, and it appears in diagnostics.

- [ ] `node tools/check-version.mjs`: all sources at the release version (Cargo workspace, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, every crate inherits `version.workspace`).
- [ ] `npm ci`
- [ ] `cargo test -p callcore-contract` then `git diff --exit-code -- src/generated` (the TS bindings are current).
- [ ] `cargo fmt --all -- --check`
- [ ] `npm run build` (`tsc --noEmit` + `vite build`)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`: all green, **run twice**. A test that fails only sometimes blocks the release until it is fixed.
- [ ] `npx tsc --noEmit`
- [ ] `npx eslint src`
- [ ] `npx vitest run`
- [ ] `cargo audit`: no unaddressed advisories.
- [ ] `npm audit --omit=dev`: no production advisories.
- [ ] `cargo run -p callcore-session --example latency_bench --release` exits 0 (check 1, the session-overhead check, passes). Check 2 should also PASS on an idle machine; a WARN there means the machine was busy, so rerun with nothing else running. Paste the table into the release notes.
- [ ] `npx tauri build` produces `target/release/bundle/nsis/AI Call Assistant_<version>_x64-setup.exe`. Record the size (target < 15 MB).
- [ ] The CI run for the release commit is green (`check` + `bundle` jobs).
- [ ] `docs/CHANGELOG.md` has an entry for this version.

## 2. Native manual matrix

Use the **signed installer** from §4 on real hardware, not `tauri dev`. Before starting:
- In the app, confirm the version chip in Settings matches the release and shows no `dirty` revision.
- Settings → **Copy diagnostics**, and keep the text with the test record.

### 2.1 Content protection, verified from the REMOTE side

For every row, share the screen (or the whole display) from the test PC and look at the **other participant's view**, on a second machine or phone. Check all three app views: **Full** layout, **Prompter** strip and **Settings** screen. In each case the app window must be **invisible** in the remote view (you should see a black or empty area, or the content behind it, depending on the app). The local indicator must read **"Hidden from screen capture"** in that view.

| Capture path | Full | Prompter | Settings |
|---|---|---|---|
| Zoom desktop, share entire screen | [ ] | [ ] | [ ] |
| Zoom desktop, share window (another app, with ours on top) | [ ] | [ ] | [ ] |
| Microsoft Teams (new Teams), share screen | [ ] | [ ] | [ ] |
| Google Meet in **Edge**, present entire screen | [ ] | [ ] | [ ] |
| Google Meet in **Chrome**, present entire screen | [ ] | [ ] | [ ] |
| OBS Studio **Display Capture** (full screen), check the recording/preview | [ ] | [ ] | [ ] |
| OBS Studio **Window Capture** targeting our window (Windows 10+ capture method) | [ ] | [ ] | [ ] |

Also check:
- [ ] The indicator never shows "Hidden from screen capture" before Windows confirmed it. Right after launch it reads "Screen-share protection not confirmed yet", then turns to "Hidden…" within about 5 s.
- [ ] Protection holds after: switching Full → Prompter → Full, opening and closing Settings, minimizing and restoring, moving to another monitor, a page reload (Ctrl+R in a debug build only), and a second launch (which focuses the first instance).
- [ ] Negative test on an unsupported setup, if available (Windows 10 older than 2004, or an RDP session where affinity is ignored): the red "Windows would not hide this window — it may be visible in screen shares" alert shows in all three views.

### 2.2 Windows, monitors and DPI

- [ ] First run (no settings file): the window appears docked top-centre of the primary display, in under 500 ms after launch (check `window shown` ms in `aica.log`). The core is ready in under 2 s (`core built`).
- [ ] Multi-monitor: move the window to a secondary monitor (including one left of or above the primary, which has negative coordinates), quit and relaunch. It reopens there.
- [ ] Mixed DPI (100 % + 125 %/150 %): Dock (⊤) on each monitor centres correctly under the camera line. The size is sensible after moving between monitors. Prompter mode docks on the **current** monitor.
- [ ] Each layout remembers its own size and position across restarts.
- [ ] Monitor unplug: save a position on monitor 2, unplug it, relaunch. The window docks top-centre of the remaining display at the saved size, with a reachable title bar.
- [ ] Always on top works, and toggling *Keep window on top* applies immediately.
- [ ] Idle CPU ≈ 0 % and memory < 150 MB (Task Manager, app idle for 1 min after an answer).

### 2.3 Audio devices

- [ ] Normal: record 10–20 s of a YouTube clip or the call. The transcript is correct, the level meter moves, and the answer arrives.
- [ ] The final words spoken right before Stop appear in the final transcript (audio tail / capture cutoff).
- [ ] Silence: record with nothing playing. After 5 s, "No call audio detected yet…" appears. Stop gives "No speech detected in the recording…" with no LLM call (no provider usage).
- [ ] Device unplug mid-recording (USB headset or DAC): the notice "The system audio device was disconnected during recording. Answering with what was captured." appears and an answer arrives for the captured audio.
- [ ] Bluetooth headset: record through it. Then power it off mid-recording (device lost), and switch between A2DP and hands-free profiles if applicable.
- [ ] Default-device switch mid-recording (change the default output in Windows Sound settings): "The default output device changed — now capturing the new device." appears and capture continues on the new device.
- [ ] No output device at all (disable every playback device): Record gives "Could not open the system audio device…" immediately, and the app stays usable afterwards.
- [ ] The 120 s cap: the countdown appears in the last 30 s, and at 0 "Reached the 120s limit — answering now" appears and it answers.
- [ ] The global hotkey starts and stops recording while another app (the call app) has focus. While Settings is open it only stops.

### 2.4 Close guard and exit

- [ ] Edit Settings without saving, click the window's X: the unsaved bar appears and the window stays open. Click X again within 10 s: the app closes.
- [ ] With no unsaved changes, X closes immediately.
- [ ] Close during a recording and during a streaming answer: the process exits within about 3 s (check Task Manager, no leftover `aicallassistant.exe`).
- [ ] Alt+F4 behaves the same as X.

### 2.5 Settings file robustness

- [ ] Corrupt file: quit, replace `%APPDATA%\AICallAssistant\settings.json` with `{not json`, launch. The banner says the file isn't valid and defaults are loaded. Quit without changing anything. Closing saves the window geometry, which is a write, so a `settings.json.corrupt-<stamp>-<id>.bak` holding the **original bytes** must exist before `settings.json` is rewritten. Launch again: the banner names the backup, and exactly one `.bak` exists.
- [ ] Unreadable file (e.g. deny read in file permissions, or replace the file with a folder of the same name): an "couldn't be opened" banner. If the backup can't be made, Save is refused with the "Settings can't be saved…" alert and the original is untouched.
- [ ] Hand-edited file with one bad field (e.g. `"answerFontPx": 99`): the other fields survive, and the bad one falls back or clamps.
- [ ] Settings save round trip: change a profile field, the provider and a key, then Save. "Saving…" disables the form, then the values persist across a restart. (The stale-revision rejection is hard to trigger by hand and is covered by the settings, AppProvider and wiring tests.)

### 2.6 Install, upgrade and uninstall

- [ ] Clean install on a Windows 10 22H2 VM and a Windows 11 machine with a **standard (non-admin)** user. The per-user install needs no UAC prompt. The Start-menu shortcut launches the app.
- [ ] Windows 10 without WebView2 (fresh VM): the installer fetches the WebView2 runtime, then the app runs.
- [ ] **Upgrade over v3.1.0 data**: on a machine where v3.1.0 was used (its data in `%APPDATA%\AICallAssistant\settings.json`, including profiles and `enc:`/`plain:` keys), install v4:
  - [ ] Profiles, the active profile, style, hotkey and fonts carry over. A legacy top-level `resume`/`jobDescription` becomes one "Default" profile.
  - [ ] Keys show "Saved (encrypted)" and work (record and answer) without re-entry.
  - [ ] After the first save (or any window move), the file is `"version": 4`, keys are `dpapi:`, and no `plain:` value or plaintext key remains in the file.
  - [ ] The v3 program (a separate Python-based install) still uninstalls cleanly on its own and doesn't delete the shared settings file. Note the result.
- [ ] Upgrade v4.x → this version over an existing install: settings are preserved and the version chip updates.
- [ ] Uninstall (Settings → Apps): the app is removed, and `%APPDATA%\AICallAssistant` (settings and logs) **remains**. Repeat with the uninstaller's "delete application data" option checked, if shown. That option removes Tauri's app-identifier folders (`com.aicallassistant.app`, WebView2 data) and must still keep `%APPDATA%\AICallAssistant`.
- [ ] Reinstall after uninstall: the previous settings load.

### 2.7 Budgeted live-provider smoke test (≤ $0.05 total)

This is the only step that uses real provider accounts. Use test keys with low limits, a **short** profile (a few lines of resume, empty notes) and **Brief** or **Balanced** style.

Budget, at the time of writing (check current prices before running):
- Claude Haiku 4.5 is listed at $1 per million input tokens and $5 per million output tokens. A short-profile prompt is about 1–2 k tokens and a Brief/Balanced answer about 100–300 tokens, so roughly $0.002–0.004 per answer.
- Groq `openai/gpt-oss-120b` is cheaper per token.
- Deepgram nova-3 streaming costs on the order of a cent per minute of audio.

Plan: **5 recorded answers of about 15 s each on Anthropic, 3 typed questions on Groq, 1 bad-key check per provider**. That comes to roughly $0.02–0.04 in total. Stop early if a provider dashboard shows more than expected.

- [ ] Anthropic: 5 × (Record about 15 s of a clip with a clear question → Stop & Answer). Each answer streams, finishes, and shows a latency chip.
- [ ] Record each run's **first word (core)** from the chip tooltip: `____ ____ ____ ____ ____` ms. The **p50 (median) must be ≤ ~1000 ms** on a normal connection. Also note audio drain, transcript finalize and visible first word for the median run.
- [ ] Groq: switch provider, ask 3 typed questions. They answer, and first-word times are noted.
- [ ] Wrong Anthropic key → "Anthropic rejected the API key (401). Check the key in Settings." with an Open Settings button. Wrong Deepgram key → a Deepgram key or code-1008 message. Restore the real keys afterwards.
- [ ] Pull the network cable (or Wi-Fi off) and Ask → "Could not connect to {provider}…" appears promptly (after one automatic retry). Reconnect.
- [ ] Check the provider dashboards: total spend ≤ $0.05. Remove or rotate the test keys if they were temporary.
- [ ] `aica.log` and Copy diagnostics contain no key, transcript or profile text (search for a distinctive word from the question and for the key prefix).

## 3. Accessibility and UI sanity (quick)

- [ ] Keyboard only: Tab reaches every control, focus is visible, Enter in the ask box submits, and Esc exits prompter mode and Settings.
- [ ] Light and dark Windows themes are both readable.
- [ ] No text anywhere says "microphone".

## 4. Code signing and SmartScreen

Unsigned installers trigger SmartScreen ("Windows protected your PC") and look untrustworthy. Sign **both** the app executable and the NSIS installer with an Authenticode certificate and an RFC 3161 **timestamp**, so signatures stay valid after the certificate expires.

- Configure Tauri to sign during `tauri build`. In `src-tauri/tauri.conf.json` → `bundle.windows`, set either:
  - `certificateThumbprint` (certificate in the user's cert store), `digestAlgorithm: "sha256"` and `timestampUrl` (e.g. `http://timestamp.digicert.com`); or
  - a `signCommand` for cloud or HSM signing (Azure Trusted Signing, a hardware token), which receives the file path as `%1`.
- Manual equivalent for each binary:

  ```
  signtool sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /sha1 <CERT_THUMBPRINT> "AI Call Assistant_<version>_x64-setup.exe"
  signtool verify /pa /v "AI Call Assistant_<version>_x64-setup.exe"
  ```

- [ ] `signtool verify /pa /v` passes for the installer **and** for the installed `aicallassistant.exe`, and shows a timestamp.
- [ ] File Properties → Digital Signatures shows the publisher name and a timestamp.
- [ ] In CI, keep the certificate or signing credentials in repository secrets. Note: `TAURI_SIGNING_PRIVATE_KEY` (mentioned in `.github/workflows/ci.yml`) is Tauri's **updater** signature key, not Authenticode. Code signing needs the certificate/`signCommand` configuration above.
- SmartScreen reputation is tied to the certificate (and the file hash). A new OV certificate still shows warnings until enough users have downloaded signed builds. EV certificates, or Microsoft's Trusted Signing, generally establish reputation faster. Keep the same certificate across releases so reputation carries over, and submit the installer to Microsoft (https://www.microsoft.com/wdsi/filesubmission) if it is falsely flagged.
- [ ] Download the signed installer through a browser on a clean machine and record the SmartScreen behavior in the release notes (no prompt, or "More info → Run anyway").

## 5. Publish

- [ ] Tag `v<version>` on the release commit, attach the signed installer and its SHA-256.
- [ ] Release notes: the CHANGELOG entry, the latency bench table, the live p50, the tested Windows builds, and known issues.
