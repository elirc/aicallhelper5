# Troubleshooting

This file maps every user-facing message in the app to its cause and fix, followed by common symptoms. Messages are quoted exactly as the code emits them. `{…}` marks a value filled in at runtime, and `[…]` marks an optional part. The error **code** (from the closed set in spec §10) is what the UI keys its behavior on. When a message appears in the red session-error box, the three codes `no_stt_key`, `no_llm_key` and `llm_auth` also offer an **Open Settings** button.

## Logs and diagnostics

| What | Where |
|---|---|
| Settings file | `%APPDATA%\AICallAssistant\settings.json` (the Settings screen shows the exact path) |
| Backups of a bad settings file | Next to it: `settings.json.corrupt-<yyyyMMddTHHmmss>-<id>.bak` or `settings.json.unreadable-…bak` |
| Log file | `%APPDATA%\AICallAssistant\logs\aica.log`, rotated at 1 MB into `aica.1.log` and `aica.2.log` (3 files kept). It holds codes, phases, timings and startup milestones (`settings loaded`, `window shown`, `core built`, `core ready`, `event channel attached`, `shutdown`), never keys, transcripts or profile text. Panics are logged as one redacted line. |
| Diagnostics | Settings → About → **Copy diagnostics**. Copies version, git revision and dirty flag, build time, OS version, protection verdict, core state and error, settings load status (and whether writes are blocked), hotkey status and message, event-pump counters, and the last 200 log lines. The whole text is redacted again (anything key-like becomes `[redacted]`). Paste it into a bug report. |

If `%APPDATA%` is not set, the app falls back to `%USERPROFILE%\AICallAssistant`.

## Symptoms

### "No call audio detected yet — make sure the call is playing through your default output device."
The app records **system output** (what you hear) from the Windows **default playback device**, never the microphone. This hint appears after 5 s of recording with no audible level (RMS below 0.01).
- Make sure the call's audio goes to the device set as default under *Settings → System → Sound → Output*. If Zoom or Teams output to a headset that isn't the Windows default, the app hears nothing. Either set that headset as default, or set the call app's speaker to "Same as system".
- Check that the call isn't muted and the other person is actually talking. Silence is expected between questions.
- If the default device changes mid-recording, the app follows it and shows "The default output device changed — now capturing the new device."
- Exclusive-mode apps (some DAWs, ASIO drivers) can block shared-mode loopback. Close them.
- Manual check: `cargo run -p callcore-audio --example capture_probe` (from source) captures 3 s and prints the peak RMS. Play audio while it runs.

### The answer is slow
Open the latency chip ("first word in X ms"). Its tooltip shows all the metrics:
- **Audio drain** is the time to deliver the last captured audio after Stop. It is normally small. Values near 2000 ms mean the drain hit its 2 s bound (a stuck audio driver).
- **Transcript finalize** is Deepgram flushing the final transcript. High values mean network latency to Deepgram. The limit is 5 s, after which you get `stt_timeout`.
- **First word (core)** is Stop → first answer token, and it *includes* the two above. What's left after subtracting them is the provider's time to first token (network + model).
- **Visible first word** is the frontend's click → first painted word.

Fixes:
- Switch the provider to **Groq GPT-OSS 120B (fastest)** in Settings.
- Use **Brief** style.
- Keep profiles focused. Very long resumes and notes make every prompt bigger, although Anthropic caches the profile prefix.
- Check your network: VPNs and proxies add round trips.
- The first answer after launch pays the TLS handshake. Record, Ask and Stop pre-warm the connection, so later answers reuse it.

### The window is visible in a screen share
Check the protection indicator in the window header, the prompter strip or the Settings screen:
- **"Hidden from screen capture"**: Windows confirmed `WDA_EXCLUDEFROMCAPTURE` by reading it back.
- **"Screen-share protection not confirmed yet"**: the app is still verifying. It retries at startup for about 4.4 s and again on focus, reload and layout change.
- **Red "Windows would not hide this window — it may be visible in screen shares"**: Windows refused or the read-back did not match. **Don't share your screen with the app open.**

Causes and fixes:
- Windows older than 10 version 2004 doesn't support exclusion. Update Windows.
- Some remote-desktop, virtual-machine or older display-driver setups ignore display affinity. Test from the remote side (see [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md)).
- Capture tools that grab the whole physical display through unusual paths (hardware capture cards, phone cameras) can't be blocked by any software.
- Focusing the window, reloading, or switching layout re-applies protection. If it stays red, copy diagnostics (the `protection:` line) and report it.

### The global hotkey doesn't work
Settings → *Shortcut and window* shows the honest status:
- **"Registered — {hotkey} toggles recording from any app."**: working. While Settings is open, the hotkey can only **stop** a recording, never start one.
- **"Disabled — no global shortcut is set."**: the field is empty. Enter one, for example `Ctrl+Shift+Space`.
- **"Not registered — the saved shortcut is not valid."**: it needs Ctrl, Alt, Shift or Win plus exactly one key.
- **"Not registered — Windows would not give the app this shortcut (another app may be using it)."**: another app owns the combination. Pick a different one.

Some full-screen games and elevated (admin) windows swallow global hotkeys. Click back into a normal window.

### SmartScreen: "Windows protected your PC"
Unsigned or newly signed installers have no SmartScreen reputation yet. Click **More info → Run anyway** only if you got the installer from the official source. Release builds should be Authenticode-signed with a timestamp (see [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md#code-signing-and-smartscreen)). Reputation builds up over downloads, and EV certificates get it faster.

### The app window didn't appear, or a second launch does nothing
Only one instance runs. A second launch focuses the existing window, which may be on another monitor or minimized. A saved position is reused only if its title bar is reachable on a connected monitor. Otherwise the window docks top-centre of the current display. Use ⊤ (**Dock under camera**) to bring it back under the webcam.

### The page never finishes "Starting…"
The heavy core (audio worker, clients, session actor) builds after the window appears, usually within 2 s. Commands wait up to 25 s. After that they report "The app is still starting. Try again in a moment." If it never becomes ready, check `aica.log` for `core failed to start`, then restart.

## Messages

### Session and recording (codes from the session core)

| Message | Code | Cause | Fix |
|---|---|---|---|
| Add your Deepgram API key in Settings to record. | `no_stt_key` | Record pressed with no usable Deepgram key (missing, blank or undecryptable). | Settings → API keys → Deepgram → paste the key → Save. Ask still works without it. |
| Add an API key for the selected answer provider in Settings. | `no_llm_key` | No usable key for the **selected** provider (Anthropic or Groq). | Add that key, or switch the answer provider to one you have a key for. |
| No speech detected in the recording. Make sure call audio is playing. | `no_speech` | Deepgram returned an empty or whitespace transcript. The LLM is never called. | See "No call audio detected yet" above. Record while the other person is actually speaking. |
| Could not open the system audio device. Check that a default output device exists, then try again. | `internal` | The WASAPI loopback of the default playback device couldn't be opened or started (no output device, the device was disabled, the audio service isn't running, or start took more than 10 s). The details are in the log only. | Make sure a playback device is enabled and set as default. Plug in speakers or headphones if there are none. Restart the Windows Audio service if needed. |
| The system audio device was disconnected during recording. Answering with what was captured. | notice (`audio:device lost`) | The playback device vanished mid-recording (unplug, Bluetooth drop, driver reset). The recording stops and answers with the audio it has. | Reconnect the device, or pick another default, before the next Record. |
| The default output device changed — now capturing the new device. | notice (`audio:device changed`) | Windows' default playback device changed mid-recording. Capture followed it. | Nothing to do. Make sure the call audio is on the new device. |
| Reached the 120s limit — answering now | status line | The 120 s recording cap was hit. The recording stops and answers automatically. "Auto-stop in N s" shows during the last 30 s. | Stop sooner. Long questions are fine, but 120 s is the maximum. |
| That recording is no longer active. | `internal` / notice | Stop reached the core for a session that already ended (cap, device loss, error or supersede). The page then checks the core (up to 5 polls) and returns to idle. | Nothing to do. Press Record again. |
| Type a question first. | `internal` / notice | Ask with an empty or whitespace question. A live recording is not affected. | Type a question. |
| The session core is shutting down. | `internal` | A command arrived while the app was exiting. | Nothing to do. Restart the app if this appears without closing it. |
| Could not read the saved API keys. Try again. | `internal` | The background key read failed unexpectedly. | Retry. If it repeats, re-enter the keys and copy diagnostics. |
| Could not build the answer request. Copy diagnostics from Settings and report it. | `internal` | Building the prompt or request panicked (a bug). | Copy diagnostics and report it. Try a different profile in the meantime. |
| Cancelled. | `aborted` | The session was cancelled. Not shown as an error. | None. |

### Speech-to-text (Deepgram)

| Message | Code | Cause | Fix |
|---|---|---|---|
| Could not connect to Deepgram. Check the API key and your network. | `stt_connect` | DNS, TLS, refused or handshake failure, a non-auth HTTP rejection, or no socket within 5 s. | Check internet access, firewall or proxy rules for `wss://api.deepgram.com`, and the key. |
| Deepgram rejected the API key (HTTP {401\|403}). Check the key in Settings. | `stt_connect` | Deepgram refused the key during the WebSocket handshake. | Create or copy a valid key at console.deepgram.com and save it. Check project balance and permissions. |
| Deepgram closed the connection (code {1008\|4001\|4003\|4008}[: {reason}]) | `stt_connect` | Auth-class close: bad, revoked or out-of-credit key. | Fix the key or the Deepgram account. |
| Deepgram closed the connection (code {other}[: {reason}]) | `stt_error` | The server closed the stream with an error mid-recording. | Retry. If the code repeats, check Deepgram's status page. |
| The Deepgram API key contains characters that are not allowed. Re-enter it in Settings. | `stt_connect` | The stored key can't be sent as a header (control characters). | Paste the key again. |
| Deepgram ended the stream unexpectedly | `stt_error` | The server ended the stream (normal close, or the TCP stream ended) before the recording finished. | Retry. |
| Lost the connection to Deepgram mid-stream ({detail}). | `stt_error` | Transport error while recording (network drop, protocol error, oversized frame). | Check the network and retry. |
| Lost the connection to Deepgram while sending audio. | `stt_error` | Writing audio to the socket failed. | Check the network and retry. |
| The Deepgram connection is closed. | `stt_error` | The app tried to send on a socket that had already died. | Retry. |
| Deepgram closed the connection before the transcript was final. Try again. | `stt_error` | The event stream ended, or a flush arrived, before the recording finished. | Retry. |
| Deepgram did not finish the transcript within 5 seconds. Try again. | `stt_timeout` | No final transcript within 5 s after the end-of-audio marker. | Retry. Usually a slow or unstable connection to Deepgram. |

### Answer providers (Anthropic `Anthropic` / Groq `Groq`)

`{name}` is `Anthropic` or `Groq`. `{snippet}` is the provider's own error text, cut to 200 characters, with control characters stripped and any key replaced by `***`.

| Message | Code | Cause | Fix |
|---|---|---|---|
| {name} rejected the API key ({401\|403}). Check the key in Settings. | `llm_auth` | Invalid, revoked or unauthorized key. | Create a new key (console.anthropic.com/settings/keys or console.groq.com/keys) and save it. |
| The {name} API key contains characters that can't be sent. Re-enter it in Settings. | `llm_auth` | The key can't be used as an HTTP header value. | Paste the key again. |
| {name} is rate-limiting or overloaded ({429\|529})[: {snippet}]. Wait a few seconds and try again. | `llm_rate_limit` | HTTP 429/529, or an in-stream `rate_limit_error`/`overloaded_error` (Anthropic). | Wait and retry (Regenerate). Check the account's rate limits and credits, or switch provider. |
| Could not connect to {name}[ (timed out)]. Check your internet connection and try again. | `llm_http` | No response byte arrived (DNS, TLS, refused, reset, 10 s connect timeout). Already retried once automatically with the identical request. | Check the network, VPN, proxy or firewall for `api.anthropic.com` / `api.groq.com`. |
| {name} returned an error ({status})[: {snippet}] | `llm_http` | Any other non-2xx status (e.g. 400, 500, 502). | Retry. For a 400, the snippet explains the reason (e.g. billing). |
| The model claude-haiku-4-5 is no longer available — switch the answer provider to Groq in Settings or install the latest version. | `llm_http` | Anthropic answered 404 for the model. | Switch provider in Settings or update the app. |
| The model openai/gpt-oss-120b is no longer available — switch the answer provider to Claude in Settings or install the latest version. | `llm_http` | Groq answered 404, or 400 with `model_decommissioned` / `model_not_found` / "does not exist". | Switch the provider to Claude or update the app. |
| Anthropic reported an error mid-answer[: {message}] | `llm_http` | An in-stream `error` event (not rate-limit or overload). | Retry. |
| Groq reported an error mid-answer[: {message}] | `llm_http` | A `{"error": …}` payload in the stream. | Retry. |
| The {name} answer stream ended before it finished. Try again. | `llm_http` | The stream ended without the provider's terminal signal (`message_stop`, or `finish_reason`/`[DONE]`). Text already shown stays. | Regenerate. |
| The connection to {name} dropped while the answer was streaming[ (no data for 75 seconds)]. Try again. | `llm_http` | The connection broke after the response started. Never retried once text has been shown. | Regenerate. Check network stability. |
| {name} returned an empty answer. Try again or rephrase the question. | `llm_http` | The provider finished with no text, or whitespace only. | Rephrase, or Regenerate. |
| The answer provider returned an empty answer. Try again, or switch the answer provider in Settings. | `llm_http` | Session-level guard for the same case. | Same as above. |
| The answer provider failed. Try again. | (mapped code) | Fallback when a provider failure carries no message. | Retry. Copy diagnostics if it repeats. |
| The answer didn't start streaming within 10 seconds. Try again, or switch the answer provider in Settings. | `llm_first_token_timeout` | No first token within 10 s of sending the request. | Retry, or switch provider (Groq is usually faster). |
| The answer took longer than 60 seconds and was stopped. | `llm_timeout` | The whole answer took more than 60 s. | Use Brief style, or retry. |
| The answer was cut off at the length limit. | note | finish = truncated (1024-token cap or context window). | Use Brief or Balanced, or ask a narrower question. |
| The model declined to answer this one. | note | finish = refused. | Rephrase the question. |

### Settings (saving)

All of these are `internal` and mean **nothing was saved**. The Settings form keeps your draft.

| Message | Cause | Fix |
|---|---|---|
| Settings changed elsewhere — nothing was saved. Your edits are still here; review them and save again. *(core)* / Settings changed elsewhere — nothing was saved. Review and save again. *(Settings screen)* / Settings were changed elsewhere, so nothing was saved. Your edits are still here — review them and save again. *(quick controls)* | The save's `baseRevision` was stale because another save landed first (e.g. a style chip or font click). Settings were reloaded. | Review and press Save again. |
| Windows could not encrypt the API key (DPAPI) — nothing was saved and your previous key is unchanged. Try again; if it keeps failing, sign out of Windows and back in. | DPAPI `CryptProtectData` failed. Encryption fails closed. | Retry, or sign out and back in. Roaming or corrupted profile keys can cause this. |
| API keys must be plain ASCII with no spaces — check for smart quotes or stray characters. Nothing was saved. | The pasted key contains spaces, smart quotes or non-ASCII characters. | Copy the key again from the provider console. |
| That API key is too long. Nothing was saved. | Over 4096 characters. | Paste only the key. |
| Unknown API key "{id}". Nothing was saved. | Internal mismatch between the page and the core. | Restart the app. |
| Unknown answer provider. Nothing was saved. | The selected provider isn't in this build. | Pick a listed provider. |
| The selected profile doesn't exist. Nothing was saved. | The active profile id isn't in the list. | Reopen Settings and choose a profile. |
| Keep at least one profile. Nothing was saved. / At most 20 profiles are allowed. Nothing was saved. | Profile count outside 1–20. | Add or delete profiles accordingly. |
| A profile has an invalid id. Nothing was saved. / Two profiles share the same id. Nothing was saved. | Corrupted profile list (ids must match `^[A-Za-z0-9_-]{1,64}$` and be unique). | Restart the app. Report it if it repeats. |
| Every profile needs a name. Nothing was saved. *(the Settings screen blocks this earlier with "Every profile needs a name.")* | Blank profile name. | Enter a name. |
| Profile "{name}": {the name\|focus\|the resume\|the job description\|notes} is longer than {60\|2000\|200000} characters. Nothing was saved. | A field is over its limit. | Shorten it. |
| The hotkey is too long (max 100 characters). Nothing was saved. | Over 100 characters. | Enter a real shortcut. |
| Prompter text size must be an even size from 14 to 28. Nothing was saved. / Answer text size must be an even size from 12 to 22. Nothing was saved. | Font size off the step-2 grid. | Use A−/A+. |
| Couldn't save settings to {path} ({error}). Nothing was saved. | The atomic write failed (disk full, permissions, file locked by antivirus or sync software). | Free disk space, check folder permissions, and exclude the folder from aggressive sync or AV. Retry. |
| Settings can't be saved: the settings file that failed to load couldn't be backed up first, so it was left untouched. Nothing was saved. Move or fix {path} (or free disk space / check folder permissions), then restart the app. | The file was corrupt or unreadable **and** the required backup failed. All writes are now refused (including window-position saves) so the original is never lost. The Settings screen also shows "Settings can't be saved: the original file couldn't be backed up, so the app won't overwrite it." | Move `settings.json` somewhere safe (or fix it), free disk space or fix permissions, then restart. |
| Settings have not loaded yet — try again in a moment. | A quick control (style, font, call type) was used before settings loaded. | Wait a moment. |
| Saving failed. Try again. | The save failed without a message. | Retry. Copy diagnostics if it repeats. |
| Fix the global shortcut first. | The hotkey field is invalid, so Save is blocked. | See the hint under the field. |
| Keep the shortcut under 100 characters. / Use a shortcut like Ctrl+Shift+Space: modifiers and one key joined by +. / Add at least one modifier: Ctrl, Alt, Shift or Win. / Use exactly one non-modifier key, e.g. Ctrl+Shift+Space. | Client-side shortcut validation. | Follow the hint. |

### Settings (loading) — banner at the top of Settings

| Message | Cause | Fix |
|---|---|---|
| Your settings file isn't valid, so defaults are loaded. The original will be backed up next to {path} before anything is saved. | `settings.json` isn't a JSON object (hand edit, truncated write, disk error). | Nothing is lost. Before the first write of any kind (a save, or the window position saved when you move or close the window), the original is backed up next to it. Fix the JSON by hand, or re-enter settings. |
| Your settings file couldn't be opened ({error}), so defaults are loaded. It will be backed up next to {path} before anything is saved. | IO error reading the file (locked, permissions, it's a folder). | Close whatever locks it (AV or sync) and restart. |
| Your settings file couldn't be loaded, so defaults were used. The original was backed up to {backup path}. | The backup succeeded on the first write. | Recover values from the `.bak` file if needed. |
| stored key couldn't be read — enter it again | A key exists but can't be decrypted: the file came from another Windows user or PC, or it's a legacy plaintext key DPAPI couldn't encrypt. The key reads as unset and is kept on disk. | Paste the key again and save, or press Remove. |

Invalid individual fields (a bad font size, an unknown provider, a malformed profile) never trigger a banner. They quietly fall back to defaults and the rest of the file is kept.

### App core, window and links

| Message | Code | Cause | Fix |
|---|---|---|---|
| The app core failed to start. Restart the app; if it keeps happening, copy diagnostics from Settings. | `internal` | Startup of the heavy core failed. The status line shows the specific reason (e.g. "The settings file could not be loaded." or "Could not start the audio worker."). | Restart. If it repeats, copy diagnostics (the `core error:` line) and check `aica.log`. |
| The app is still starting. Try again in a moment. | `internal` | A command waited 25 s for the core to finish starting. | Wait, or restart if it persists. |
| The app took too long to respond (30 s). Try again. / The app core did not respond in time. | `internal` | A command exceeded the 30 s command timeout (core side or page side). | Retry. Copy diagnostics if it repeats. |
| Something went wrong inside the app. Try again; if it keeps happening, copy diagnostics from Settings. | `internal` | A command panicked (caught at the boundary). | Retry and report it with diagnostics. |
| The app core sent an unexpected response. / The app core returned an error. | `internal` | The page got a malformed reply, or an IPC error with no message. | Restart. Report it with diagnostics. |
| The window is not ready yet. | `internal` | Dock or open-link was used before the window finished initializing. | Retry. |
| Could not move the window. | `internal` | Docking failed: Windows reported no current or primary monitor, or the window size/position call failed. | Move the window by hand, or retry. |
| That link can't be opened: {it is empty \| it is longer than 2048 characters \| it contains spaces or control characters \| only https links are allowed \| it contains a user name or password \| it has no host \| the host name is not valid \| the port is not valid}. | `internal` | `open_external` accepts only clean `https://host/...` URLs. | None needed: the built-in "Get a key" links are valid. |
| Windows could not open the link. | `internal` | `ShellExecuteW` failed (no default browser). | Set a default browser. |
| Could not copy to the clipboard. / Copy failed / Couldn't collect diagnostics. | notice | WebView2 denied clipboard access and the fallback failed, or diagnostics couldn't be collected. | Retry. Select the text by hand. |

### Hotkey status messages (header hint and Settings)

| Message | Meaning / fix |
|---|---|
| {hotkey} toggles recording from any app | Registered. |
| Global shortcut is off — set one in Settings / The global shortcut is turned off. Set one in Settings to Record/Stop from any app. | Empty hotkey. Set one if you want it. |
| "{text}" is not a valid shortcut. Use Ctrl, Alt, Shift or Win plus one key, e.g. Ctrl+Shift+Space. / The global shortcut in Settings is not valid | Fix the shortcut. |
| {hotkey} is already taken by another app. Choose a different shortcut in Settings. / The global shortcut is in use by another app — pick a different one in Settings | Another app registered it first. Pick a different one. Saving Settings retries the registration. |
| Registering {hotkey}… | The OS registration is in flight (it can take up to 2 s when the UI thread is busy). |
| The global shortcut is not registered yet — the app is still starting. | Registration happens right after the core is ready. |

### Status line (not errors)

| Text | Meaning |
|---|---|
| Starting… | The core is still starting. Controls are disabled. |
| Add your API keys — open Settings to get started | First run: a Deepgram key or the selected provider's key is missing. |
| Ready — press Record while the other person is speaking | Idle. |
| Starting system-audio capture… | Record pressed. Opening the device and the Deepgram socket in parallel. |
| Recording — m:ss | Capturing. Levels show in the meter. |
| Finalizing transcript… | Stop accepted. Draining audio and waiting for Deepgram's final transcript. The button reads "Finalizing…". |
| Generating answer… | Streaming the answer. |
| Reached the 120s limit — answering now | Auto-stopped by the cap. |
