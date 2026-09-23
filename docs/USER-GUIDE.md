# User guide

AI Call Assistant listens to the other person on your call and suggests what to say back. Press **Record** while they talk, then **Stop & Answer**. The suggested answer streams in at the top of the screen, right under your webcam, so you can read it while looking at the camera. Other people in the call can't see the window when you share your screen, as long as the protection indicator says so (see [Screen-share protection](#screen-share-protection)).

## 1. First run: add your keys

The app uses your own accounts:

| Key | What it's for | Where to get it |
|---|---|---|
| **Deepgram** | Live transcription of the call audio. Needed to **record**. | https://console.deepgram.com/ |
| **Anthropic** | Answers from *Claude Haiku 4.5 (recommended)*, the default. | https://console.anthropic.com/settings/keys |
| **Groq** | Answers from *Groq GPT-OSS 120B (fastest)*. | https://console.groq.com/keys |

You need Deepgram plus **one** of Anthropic or Groq, whichever answer provider you select.

1. Open **Settings** with ⚙ in the header, or with the *Open Settings* button when the status line says "Add your API keys — open Settings to get started".
2. Under **API keys**, paste each key into its field. **Get a key** opens the provider's page in your browser.
3. Under **Answers**, pick the **Answer provider** that matches your key.
4. Press **Save**. The form is disabled and the button reads "Saving…" until the save finishes.

Your keys are write-only. Once saved, the field shows *saved — type to replace* with the status "Saved (encrypted)". Typing a new key replaces the old one when you save. Clearing the field does **not** delete a key. Use **Remove**, which you can undo before saving, and typing a new key cancels a pending Remove. If a key shows "stored key couldn't be read — enter it again", the settings file came from another Windows account or PC. Paste the key again.

## 2. Profiles and call types

A **profile** tells the assistant who you are and what the call is about. Settings → **Profiles**:

- **Editing** picks which profile you're editing. **Add**, **Duplicate** (the copy is named "<name> copy") and **Delete** manage the list. You can have 1 to 20 profiles.
- **Use for answers** makes the profile you're editing the active one. The button reads **Active profile** when it already is.
- Fields:
  - **Name** (up to 60 characters)
  - **Call type**
  - **Focus**: up to 2000 characters, for what matters most in this call, such as the stack to emphasize or topics to steer toward
  - **Resume** and **Job description**, labelled **Background** and **Call context** for sales and meeting calls
  - **Notes**: facts you want the answers to use, such as pricing, availability or salary range

Only the non-empty fields are sent. When any of them is filled in, answers are told to stick to what you provided and never invent experience or facts.

**Call types** change how answers are shaped:

| Call type | What the answers do |
|---|---|
| **Behavioral interview** (default) | A specific example from your experience, with a concrete outcome. Longer answers follow situation → what you did → result. |
| **Technical screen** | The direct, correct technical answer first (precise API or language names), then one tradeoff or edge case. For coding problems: approach plus time and space complexity, not a code listing. |
| **System design** | The driving requirements, then components and data flow, then the key tradeoff and what changes at 10× scale. Asks a clarifying question when the prompt is ambiguous. |
| **Recruiter screen** | Short, warm, positive, with straight logistics answers. For compensation: a range, or deferring to the process, unless your notes say otherwise. |
| **Sales or customer call** | Addresses what they're really asking (objection, price, next step). Never invents pricing, features or commitments. |
| **General meeting** | The most useful contribution right now: an answer, a clarifying question, a decision or a next step. |

On the main screen, the **profile dropdown** switches the active profile (it appears only when you have more than one). The **call type dropdown** changes the active profile's call type and saves it right away.

## 3. Recording and answering

1. Join your call. Make sure you **hear** the other person through your Windows **default playback device**. The app records system output audio, never your microphone.
2. When they start asking something, press **Record**, or the global hotkey (default `Ctrl+Shift+Space`). The status line shows "Starting system-audio capture…", then "Recording — 0:05". A level meter moves while there's sound, and the live transcript appears under **QUESTION HEARD**.
3. When they finish, press **Stop & Answer** (or the hotkey again). The status line shows "Finalizing transcript…" while the last audio is transcribed (the button reads "Finalizing…"), then "Generating answer…" while the answer streams into **SUGGESTED ANSWER**.
4. Read it aloud in your own words.

Good to know:
- A recording can last at most **120 seconds**. An "Auto-stop in N s" countdown appears during the last 30 s. At the limit the app answers automatically ("Reached the 120s limit — answering now").
- If the status says "No call audio detected yet…", the app isn't hearing anything on the default playback device. See [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
- If you unplug the playback device mid-recording, the app answers with what it captured so far. If Windows switches the default device, capture follows it.
- Pressing **Record** while an answer is still streaming starts a new question and drops the old answer.
- There is no separate Cancel button. Starting a new Record or Ask replaces the current one.

**Type a question instead.** Type into "Type a question instead…" and press **Enter** or **Ask**. You get an answer without recording, and no Deepgram key is needed.

**Regenerate** asks the question you're viewing again. The new answer is added as a **new** history entry, and the old one is kept.

**The answer panel:**
- **A− / A+** change the text size (12–22 px).
- **Copy** copies the answer's Markdown source.
- The panel follows new text as it streams until you scroll up. Scroll back to the bottom to resume following.
- The chip "first word in X ms" shows how long it took from Stop to the first word. Hover it for the breakdown (audio drain, transcript finalize, total, visible first word).
- A note appears when the answer was cut off at the length limit, or when the model declined to answer.

## 4. Answer styles

The **Brief / Balanced / Detailed** chips on the main screen (and the *Answer style* setting) apply to every profile. Changing style mid-recording applies to the answer for that recording.

- **Brief**: one or two spoken sentences, no lists.
- **Balanced** (default): a few sentences for simple questions, short structured points for complex ones.
- **Detailed**: one direct sentence, then three to five short supporting points.

## 5. Prompter mode and docking

- **⤒ Enter prompter mode** turns the window into a short, wide strip showing only the answer in large text (14–28 px, default 18). The strip keeps compact controls:
  - Record/Stop with timer, the one-line question, style chips and history arrows
  - A−/A+
  - **⤒** re-docks the strip under the camera
  - **⤢** (or **Esc**) exits to the full layout
- The prompter text stays anchored at the top while it streams, so it doesn't jump. When there's more than fits, a **▼ more** hint appears. Click it or scroll at your own pace.
- **⊤ Dock under camera** (full layout) moves the window to the top centre of the monitor it's on, right under a typical laptop or monitor webcam.
- Each layout remembers its own size and position. On first run the window starts docked top-centre. If a saved position is no longer reachable (for example, a monitor was unplugged), the window docks top-centre of the current display instead.
- **Keep window on top** (Settings → *Shortcut and window*) is on by default.

## 6. Global hotkey

The hotkey toggles **Record / Stop & Answer** from any app, so you don't have to click the window. The default is `Ctrl+Shift+Space`. To change it, go to Settings → *Shortcut and window* → **Global shortcut**. Use any of Ctrl, Alt, Shift or Win plus one key, for example `Ctrl+Alt+R` or `Win+F9`. Leave the field empty to turn the hotkey off.

The line under the field tells you honestly whether Windows registered it. If another app already uses the combination, pick a different one. While Settings is open, the hotkey can only **stop** a recording (so you never start one by accident). A Stop & Answer bar also appears at the top of Settings while a session is active.

## 7. History

The last **6** answers stay on screen. Use **← / →** to move between them ("n/m"). Each entry is tagged with its call type. **Clear** empties the history.

History is **never saved to disk and never sent as context.** Every answer is **single-turn**: the provider receives only the current question (transcript or typed text) and your active profile, never earlier questions or answers. If a follow-up depends on an earlier exchange, include that context in your question, or put it in the profile's notes.

## 8. Screen-share protection

The window asks Windows to exclude it from screen capture, and the app **verifies** that Windows accepted the request. The indicator appears in every view (the full header, the prompter strip and Settings):

| Indicator | Meaning | What to do |
|---|---|---|
| ● **Hidden from screen capture** | Windows confirmed the window is excluded from capture. | Share as usual. |
| ◌ **Screen-share protection not confirmed yet** | The app is still checking (normal for a few seconds after launch). | Wait for it to turn to "Hidden…" before sharing. |
| ▲ **Windows would not hide this window — it may be visible in screen shares** (red) | Windows refused the request, or it couldn't be confirmed. | **Don't share your screen** with the app open. See [TROUBLESHOOTING.md](TROUBLESHOOTING.md). |

The app never claims protection until Windows confirms it. Protection needs Windows 10 version 2004 or later. It can't stop a camera pointed at your screen or hardware capture devices.

## 9. Leaving Settings with unsaved changes

**← Back** or **Esc** with unsaved changes shows a bar: **Save and go back**, **Discard** or **Keep editing**. Only **Discard** throws your changes away. If you try to close the window with unsaved Settings changes, the same bar appears. Closing again within 10 seconds closes the app anyway.

If Settings reports "Settings changed elsewhere — nothing was saved", another save (for example a style chip) landed first. Your edits are still in the form. Review them and press Save again.

## 10. Privacy

- No account, no telemetry, no server of ours. Audio goes only to Deepgram, and questions go only to the answer provider you selected.
- API keys are encrypted with Windows DPAPI for your Windows user account. **Profiles are stored as plain text** in `%APPDATA%\AICallAssistant\settings.json`.
- Answers are single-turn. History stays in memory and is gone when you close the app.
- Logs (`%APPDATA%\AICallAssistant\logs`) contain no keys, transcripts or profile text. **Copy diagnostics** (Settings → About) produces a redacted report you can share when asking for help.
- Uninstalling the app keeps your settings folder. Delete `%APPDATA%\AICallAssistant` yourself to remove everything.
