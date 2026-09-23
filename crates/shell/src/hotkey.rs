//! Global hotkey: parsing/normalization and the registration state machine
//! (spec §12, §13: "any Ctrl/Alt/Shift/Win + key, validated, honest status";
//! "a registration that completes late must be undone").

use callcore_contract::{HotkeyStatus, HOTKEY_MAX};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

/// A validated hotkey: at least one modifier and exactly one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: Modifiers,
    /// Canonical key name, as accepted by `global-hotkey`'s parser
    /// ("A", "7", "F13", "Space", "ArrowUp", "Comma", …).
    pub key: &'static str,
}

impl Hotkey {
    /// Normalized user-facing form, e.g. "Ctrl+Shift+Space".
    pub fn display(&self) -> String {
        self.join("Win")
    }

    /// Accelerator string for tauri-plugin-global-shortcut
    /// (`global-hotkey` spells the Windows key "Super").
    pub fn accelerator(&self) -> String {
        self.join("Super")
    }

    fn join(&self, win: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mods.ctrl {
            parts.push("Ctrl");
        }
        if self.mods.alt {
            parts.push("Alt");
        }
        if self.mods.shift {
            parts.push("Shift");
        }
        if self.mods.win {
            parts.push(win);
        }
        parts.push(self.key);
        parts.join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyParse {
    /// Empty string: the user turned the shortcut off.
    Disabled,
    Valid(Hotkey),
    /// Not a Ctrl/Alt/Shift/Win + key combination. The string is a short
    /// reason (never echoes more than the ≤100-char input).
    Invalid(String),
}

const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const FKEYS: [&str; 24] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
    "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

/// Map one (non-modifier) token to its canonical key name.
fn key_name(token: &str) -> Option<&'static str> {
    let up = token.to_ascii_uppercase();
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_uppercase() {
            return Some(LETTERS[(c - b'A') as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[(c - b'0') as usize]);
        }
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<usize>().ok()) {
        if (1..=24).contains(&n) && up == format!("F{n}") {
            return Some(FKEYS[n - 1]);
        }
    }
    let name = match up.as_str() {
        "SPACE" | "SPACEBAR" => "Space",
        "ENTER" | "RETURN" => "Enter",
        "TAB" => "Tab",
        "UP" | "ARROWUP" => "ArrowUp",
        "DOWN" | "ARROWDOWN" => "ArrowDown",
        "LEFT" | "ARROWLEFT" => "ArrowLeft",
        "RIGHT" | "ARROWRIGHT" => "ArrowRight",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" | "PGUP" => "PageUp",
        "PAGEDOWN" | "PGDN" => "PageDown",
        "INSERT" | "INS" => "Insert",
        "DELETE" | "DEL" => "Delete",
        "BACKSPACE" => "Backspace",
        "ESCAPE" | "ESC" => "Escape",
        "COMMA" | "," => "Comma",
        "PERIOD" | "." => "Period",
        "MINUS" | "-" => "Minus",
        "EQUAL" | "EQUALS" | "=" => "Equal",
        "SEMICOLON" | ";" => "Semicolon",
        "SLASH" | "/" => "Slash",
        "BACKSLASH" | "\\" => "Backslash",
        "QUOTE" | "'" => "Quote",
        "BACKQUOTE" | "BACKTICK" | "`" => "Backquote",
        "BRACKETLEFT" | "[" => "BracketLeft",
        "BRACKETRIGHT" | "]" => "BracketRight",
        _ => return None,
    };
    Some(name)
}

/// Every canonical key name `parse` can produce (for glue-side tests that
/// check the plugin accepts them all).
pub fn all_key_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    v.extend(LETTERS);
    v.extend(DIGITS);
    v.extend(FKEYS);
    for t in [
        "space",
        "enter",
        "tab",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "insert",
        "delete",
        "backspace",
        "escape",
        ",",
        ".",
        "-",
        "=",
        ";",
        "/",
        "\\",
        "'",
        "`",
        "[",
        "]",
    ] {
        v.push(key_name(t).expect("listed key parses"));
    }
    v
}

/// Parse "Ctrl+Shift+Space"-style strings. Modifiers are case-insensitive
/// (Ctrl/Control, Alt, Shift, Win/Super/Meta/Cmd); tokens are trimmed.
pub fn parse(input: &str) -> HotkeyParse {
    let s = input.trim();
    if s.is_empty() {
        return HotkeyParse::Disabled;
    }
    if input.chars().count() > HOTKEY_MAX {
        return HotkeyParse::Invalid(format!("longer than {HOTKEY_MAX} characters"));
    }
    let mut mods = Modifiers::default();
    let mut key: Option<&'static str> = None;
    for raw in s.split('+') {
        let tok = raw.trim();
        if tok.is_empty() {
            return HotkeyParse::Invalid("empty part between '+' signs".into());
        }
        match tok.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "option" => mods.alt = true,
            "shift" => mods.shift = true,
            "win" | "windows" | "super" | "meta" | "cmd" | "command" => mods.win = true,
            _ => match key_name(tok) {
                Some(k) => {
                    if key.is_some() {
                        return HotkeyParse::Invalid("more than one key".into());
                    }
                    key = Some(k);
                }
                None => return HotkeyParse::Invalid("unsupported key".into()),
            },
        }
    }
    let Some(key) = key else {
        return HotkeyParse::Invalid("no key after the modifiers".into());
    };
    if !mods.any() {
        return HotkeyParse::Invalid("needs Ctrl, Alt, Shift or Win".into());
    }
    HotkeyParse::Valid(Hotkey { mods, key })
}

/// What the settings view reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyReport {
    pub status: HotkeyStatus,
    pub message: Option<String>,
}

impl HotkeyReport {
    pub fn registered(&self) -> bool {
        self.status == HotkeyStatus::Registered
    }
    pub fn disabled() -> Self {
        Self {
            status: HotkeyStatus::Disabled,
            message: Some(msg_disabled()),
        }
    }
    pub fn registered_ok() -> Self {
        Self {
            status: HotkeyStatus::Registered,
            message: None,
        }
    }
    pub fn invalid(input: &str) -> Self {
        Self {
            status: HotkeyStatus::Invalid,
            message: Some(msg_invalid(input)),
        }
    }
    pub fn unavailable(display: &str) -> Self {
        Self {
            status: HotkeyStatus::Unavailable,
            message: Some(msg_taken(display)),
        }
    }
    /// While the OS registration is in flight.
    pub fn registering(display: &str) -> Self {
        Self {
            status: HotkeyStatus::Unavailable,
            message: Some(format!("Registering {display}…")),
        }
    }
    /// Before the first registration attempt (core still starting).
    pub fn pending() -> Self {
        Self {
            status: HotkeyStatus::Unavailable,
            message: Some(
                "The global shortcut is not registered yet — the app is still starting.".into(),
            ),
        }
    }
}

pub fn msg_disabled() -> String {
    "The global shortcut is turned off. Set one in Settings to Record/Stop from any app.".into()
}

pub fn msg_invalid(input: &str) -> String {
    let shown: String = input.trim().chars().take(HOTKEY_MAX).collect();
    format!(
        "\"{shown}\" is not a valid shortcut. Use Ctrl, Alt, Shift or Win plus one key, e.g. Ctrl+Shift+Space."
    )
}

pub fn msg_taken(display: &str) -> String {
    format!("{display} is already taken by another app. Choose a different shortcut in Settings.")
}

// ───────────────────────────── registration state machine ─────────────────────────────

/// What the glue must do for one requested hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub generation: u64,
    /// Unregister this accelerator first (the one currently registered).
    pub unregister: Option<String>,
    /// Then register this accelerator, and report back via `complete`.
    pub register: Option<String>,
    /// Display form of what is being registered (for messages).
    pub display: Option<String>,
    /// Set when no registration is needed (Disabled / Invalid): the final
    /// report, already applied.
    pub immediate: Option<HotkeyReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// The newest request finished; publish this report.
    Applied(HotkeyReport),
    /// A stale registration succeeded after a newer request replaced it:
    /// unregister this accelerator (a ghost key must never survive).
    Undo(String),
    /// A stale registration succeeded and happens to be exactly what the
    /// newest request wants while the newest one failed — keep it.
    Adopted(HotkeyReport),
    /// Stale and failed: nothing to do.
    Ignore,
}

/// Generation-counted registrar. Pure; the glue performs the OS calls.
#[derive(Debug, Clone)]
pub struct Registrar {
    generation: u64,
    /// Accelerator the newest request wants registered (None = nothing).
    desired: Option<String>,
    desired_display: Option<String>,
    /// Accelerator the OS currently has registered for us.
    registered: Option<String>,
    report: HotkeyReport,
}

impl Default for Registrar {
    fn default() -> Self {
        Self::new()
    }
}

impl Registrar {
    pub fn new() -> Self {
        Self {
            generation: 0,
            desired: None,
            desired_display: None,
            registered: None,
            report: HotkeyReport::pending(),
        }
    }

    pub fn report(&self) -> &HotkeyReport {
        &self.report
    }

    pub fn registered_accelerator(&self) -> Option<&str> {
        self.registered.as_deref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Start applying `hotkey` (the raw settings string). The caller must
    /// perform `plan.unregister` and then `plan.register` (if any) and call
    /// [`Registrar::complete`] with the result.
    pub fn begin(&mut self, hotkey: &str) -> Plan {
        self.generation += 1;
        let unregister = self.registered.take();
        match parse(hotkey) {
            HotkeyParse::Disabled => {
                self.desired = None;
                self.desired_display = None;
                self.report = HotkeyReport::disabled();
                Plan {
                    generation: self.generation,
                    unregister,
                    register: None,
                    display: None,
                    immediate: Some(self.report.clone()),
                }
            }
            HotkeyParse::Invalid(_) => {
                self.desired = None;
                self.desired_display = None;
                self.report = HotkeyReport::invalid(hotkey);
                Plan {
                    generation: self.generation,
                    unregister,
                    register: None,
                    display: None,
                    immediate: Some(self.report.clone()),
                }
            }
            HotkeyParse::Valid(hk) => {
                let accel = hk.accelerator();
                self.desired = Some(accel.clone());
                self.desired_display = Some(hk.display());
                // Honest while in flight: not registered (yet).
                self.report = HotkeyReport::registering(&hk.display());
                Plan {
                    generation: self.generation,
                    unregister,
                    register: Some(accel),
                    display: Some(hk.display()),
                    immediate: None,
                }
            }
        }
    }

    /// Report the result of registering `accelerator` for `generation`.
    pub fn complete(&mut self, generation: u64, accelerator: &str, ok: bool) -> Completion {
        if generation == self.generation {
            let display = self
                .desired_display
                .clone()
                .unwrap_or_else(|| accelerator.to_string());
            // A failure while an adopted stale registration already holds the
            // very same combo still means "registered".
            let already_ours = self.registered.as_deref() == Some(accelerator);
            self.report = if ok || already_ours {
                self.registered = Some(accelerator.to_string());
                HotkeyReport::registered_ok()
            } else {
                HotkeyReport::unavailable(&display)
            };
            return Completion::Applied(self.report.clone());
        }
        if !ok {
            return Completion::Ignore;
        }
        let newest_wants_it = self.desired.as_deref() == Some(accelerator);
        if newest_wants_it && self.registered.is_none() && !self.report.registered() {
            self.registered = Some(accelerator.to_string());
            self.report = HotkeyReport::registered_ok();
            return Completion::Adopted(self.report.clone());
        }
        if self.registered.as_deref() == Some(accelerator) {
            // Already ours through the newest generation; undoing would kill it.
            return Completion::Ignore;
        }
        Completion::Undo(accelerator.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(s: &str) -> Hotkey {
        match parse(s) {
            HotkeyParse::Valid(h) => h,
            other => panic!("{s:?} -> {other:?}"),
        }
    }

    #[test]
    fn parses_the_default() {
        let h = valid("Ctrl+Shift+Space");
        assert_eq!(h.display(), "Ctrl+Shift+Space");
        assert_eq!(h.accelerator(), "Ctrl+Shift+Space");
    }

    #[test]
    fn modifiers_are_case_insensitive_and_normalized_in_order() {
        assert_eq!(valid("shift + control + a").display(), "Ctrl+Shift+A");
        assert_eq!(valid("META+alt+f13").display(), "Alt+Win+F13");
        assert_eq!(valid("super+z").accelerator(), "Super+Z");
        assert_eq!(valid("Win+Z").display(), "Win+Z");
    }

    #[test]
    fn key_families() {
        assert_eq!(valid("Ctrl+7").key, "7");
        assert_eq!(valid("Ctrl+F24").key, "F24");
        assert_eq!(valid("Alt+up").key, "ArrowUp");
        assert_eq!(valid("Alt+ArrowLeft").key, "ArrowLeft");
        assert_eq!(valid("Ctrl+Enter").key, "Enter");
        assert_eq!(valid("Ctrl+Tab").key, "Tab");
        assert_eq!(valid("Ctrl+,").key, "Comma");
        assert_eq!(valid("Ctrl+Slash").key, "Slash");
        assert_eq!(valid("Ctrl+`").key, "Backquote");
    }

    #[test]
    fn empty_is_disabled() {
        assert_eq!(parse(""), HotkeyParse::Disabled);
        assert_eq!(parse("   "), HotkeyParse::Disabled);
    }

    #[test]
    fn invalid_inputs() {
        for s in [
            "Space",       // no modifier
            "Ctrl+Shift",  // no key
            "Ctrl+A+B",    // two keys
            "Ctrl++A",     // empty token
            "Ctrl+F25",    // out of range
            "Ctrl+F0",     //
            "Ctrl+F01",    // non-canonical number
            "Ctrl+Banana", // unknown
            "Hyper+A",     // unknown modifier
            "Ctrl+Shift+", // trailing plus
        ] {
            assert!(
                matches!(parse(s), HotkeyParse::Invalid(_)),
                "{s:?} should be invalid"
            );
        }
        let long = format!("Ctrl+{}", "A".repeat(100));
        assert!(matches!(parse(&long), HotkeyParse::Invalid(_)));
    }

    #[test]
    fn exactly_100_chars_is_accepted_by_the_length_rule() {
        let s = format!("Ctrl+A{}", " ".repeat(94));
        assert_eq!(s.chars().count(), 100);
        assert!(matches!(parse(&s), HotkeyParse::Valid(_)));
    }

    #[test]
    fn messages() {
        assert_eq!(
            msg_taken("Ctrl+Shift+Space"),
            "Ctrl+Shift+Space is already taken by another app. Choose a different shortcut in Settings."
        );
        assert!(msg_invalid("Ctrl+Nope").contains("\"Ctrl+Nope\""));
        assert!(HotkeyReport::registered_ok().message.is_none());
        assert!(HotkeyReport::registered_ok().registered());
    }

    #[test]
    fn all_key_names_are_distinct_and_parse_back() {
        let names = all_key_names();
        for n in &names {
            assert_eq!(valid(&format!("Ctrl+{n}")).key, *n);
        }
        let mut dedup = names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len());
    }

    // ── registrar ──

    #[test]
    fn happy_path_registers() {
        let mut r = Registrar::new();
        let p = r.begin("Ctrl+Shift+Space");
        assert_eq!(p.unregister, None);
        assert_eq!(p.register.as_deref(), Some("Ctrl+Shift+Space"));
        assert_eq!(
            r.complete(p.generation, "Ctrl+Shift+Space", true),
            Completion::Applied(HotkeyReport::registered_ok())
        );
        assert_eq!(r.registered_accelerator(), Some("Ctrl+Shift+Space"));
    }

    #[test]
    fn change_unregisters_the_previous_one() {
        let mut r = Registrar::new();
        let p = r.begin("Ctrl+Shift+Space");
        r.complete(p.generation, p.register.as_deref().unwrap(), true);
        let p2 = r.begin("Alt+F9");
        assert_eq!(p2.unregister.as_deref(), Some("Ctrl+Shift+Space"));
        assert_eq!(p2.register.as_deref(), Some("Alt+F9"));
    }

    #[test]
    fn taken_hotkey_reports_unavailable() {
        let mut r = Registrar::new();
        let p = r.begin("ctrl+shift+space");
        match r.complete(p.generation, "Ctrl+Shift+Space", false) {
            Completion::Applied(rep) => {
                assert_eq!(rep.status, HotkeyStatus::Unavailable);
                assert_eq!(rep.message.unwrap(), msg_taken("Ctrl+Shift+Space"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r.registered_accelerator(), None);
    }

    #[test]
    fn disabled_and_invalid_are_immediate_and_unregister() {
        let mut r = Registrar::new();
        let p = r.begin("Ctrl+Q");
        r.complete(p.generation, "Ctrl+Q", true);
        let p = r.begin("");
        assert_eq!(p.unregister.as_deref(), Some("Ctrl+Q"));
        assert_eq!(p.immediate.as_ref().unwrap().status, HotkeyStatus::Disabled);
        assert_eq!(r.report().status, HotkeyStatus::Disabled);
        let p = r.begin("Ctrl+Nope");
        assert_eq!(p.unregister, None);
        assert_eq!(p.immediate.unwrap().status, HotkeyStatus::Invalid);
    }

    #[test]
    fn late_success_after_a_newer_request_is_undone() {
        let mut r = Registrar::new();
        let old = r.begin("Ctrl+Q");
        let new = r.begin("Alt+W");
        assert_eq!(
            r.complete(new.generation, "Alt+W", true),
            Completion::Applied(HotkeyReport::registered_ok())
        );
        // The old registration finally lands: it must be undone.
        assert_eq!(
            r.complete(old.generation, "Ctrl+Q", true),
            Completion::Undo("Ctrl+Q".into())
        );
        assert_eq!(r.registered_accelerator(), Some("Alt+W"));
    }

    #[test]
    fn late_success_after_disable_is_undone() {
        let mut r = Registrar::new();
        let old = r.begin("Ctrl+Q");
        r.begin("");
        assert_eq!(
            r.complete(old.generation, "Ctrl+Q", true),
            Completion::Undo("Ctrl+Q".into())
        );
        assert_eq!(r.report().status, HotkeyStatus::Disabled);
    }

    #[test]
    fn late_failure_is_ignored() {
        let mut r = Registrar::new();
        let old = r.begin("Ctrl+Q");
        let new = r.begin("Alt+W");
        r.complete(new.generation, "Alt+W", true);
        assert_eq!(
            r.complete(old.generation, "Ctrl+Q", false),
            Completion::Ignore
        );
        assert!(r.report().registered());
    }

    #[test]
    fn late_success_of_the_same_combo_is_adopted_when_the_newer_failed() {
        let mut r = Registrar::new();
        let old = r.begin("Ctrl+Q");
        let new = r.begin("Ctrl+Q");
        // Newer fails because the old one (still in flight) grabbed it.
        r.complete(new.generation, "Ctrl+Q", false);
        assert!(matches!(
            r.complete(old.generation, "Ctrl+Q", true),
            Completion::Adopted(_)
        ));
        assert!(r.report().registered());
        assert_eq!(r.registered_accelerator(), Some("Ctrl+Q"));
    }

    #[test]
    fn in_flight_registration_is_not_reported_as_registered() {
        let mut r = Registrar::new();
        r.begin("Ctrl+Q");
        assert!(!r.report().registered());
        assert_eq!(r.report().message.as_deref(), Some("Registering Ctrl+Q…"));
    }

    #[test]
    fn late_success_before_newer_completes_is_undone() {
        let mut r = Registrar::new();
        let old = r.begin("Ctrl+Q");
        let _new = r.begin("Alt+W"); // still in flight
        assert_eq!(
            r.complete(old.generation, "Ctrl+Q", true),
            Completion::Undo("Ctrl+Q".into())
        );
    }
}
