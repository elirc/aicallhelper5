//! On-disk schema, per-field validation on load, and v3 migration.
//!
//! # v4 file (`settings.json`, camelCase, pretty JSON)
//!
//! ```json
//! {
//!   "version": 4,
//!   "settingsRevision": 7,
//!   "profiles": [{ "id": "default", "name": "Default", "callType": "behavioral",
//!                  "focus": "", "resume": "", "jobDescription": "", "notes": "" }],
//!   "activeProfileId": "default",
//!   "llmProvider": "anthropic",
//!   "answerStyle": "balanced",
//!   "hotkey": "Ctrl+Shift+Space",
//!   "alwaysOnTop": true,
//!   "layoutMode": "full",
//!   "prompterFontPx": 18,
//!   "answerFontPx": 14,
//!   "windowBounds": { "x": 0, "y": 0, "width": 900, "height": 700 },
//!   "prompterBounds": null,
//!   "secrets": { "deepgram": "dpapi:<base64>" }
//! }
//! ```
//!
//! # Load rules (per field — one bad field never costs the rest)
//!
//! | field | rule | fallback |
//! |---|---|---|
//! | `settingsRevision` | non-negative integer | 0 |
//! | `profiles` | array; each entry validated individually (see below) | one "Default" profile |
//! | `activeProfileId` | string naming a loaded profile | first profile |
//! | `llmProvider` | a known provider id | "anthropic" |
//! | `answerStyle` | brief / balanced / detailed | balanced |
//! | `hotkey` | string of ≤100 chars (kept verbatim; "" = disabled) | "Ctrl+Shift+Space" |
//! | `alwaysOnTop` | bool | true |
//! | `layoutMode` | full / prompter | full |
//! | `prompterFontPx` / `answerFontPx` | number: clamped to range, then snapped DOWN to the step-2 grid from the minimum | 18 / 14 |
//! | `windowBounds` / `prompterBounds` | `{x,y,width,height}` integers, 0 < width,height < 100000, x/y in i32 | none |
//!
//! Profiles: non-object entries and entries whose `id` fails
//! `^[A-Za-z0-9_-]{1,64}$` are dropped; duplicate ids keep the first; the name
//! is trimmed and truncated to 60 chars (chars, not bytes; empty → "Untitled");
//! `focus` is truncated to 2000 chars and `resume`/`jobDescription`/`notes` to
//! 200 000; an unknown or missing `callType` becomes behavioral; only the first
//! 20 valid profiles are kept; zero valid profiles → one "Default" profile.
//!
//! # Accepted legacy (v3 and older) shapes
//!
//! The v3 Python app wrote the SAME path. Every shape below is covered by a test.
//!
//! 1. **No profiles, top-level `resume` / `jobDescription`** (or snake_case
//!    `job_description`) → one profile `{id:"default", name:"Default"}` holding
//!    that text. If a valid `profiles` list exists, top-level text is a v3
//!    write-only mirror and is ignored.
//! 2. **v3 profile objects**: same camelCase keys as v4; `job_description` and
//!    `call_type` are accepted as snake_case aliases (camelCase wins).
//! 3. **Secrets** may appear in (highest precedence first) the `secrets`
//!    object, an `apiKeys` object, or top-level `<id>ApiKey` fields
//!    (`deepgramApiKey`, `anthropicApiKey`, `groqApiKey`, …). Values:
//!    * `dpapi:<base64>` — DPAPI blob (v4 format).
//!    * `enc:<base64>` — DPAPI blob written by v3 (no entropy; the keystore
//!      falls back to no-entropy on unprotect).
//!    * `plain:<base64-of-utf8>` (v3) or `plain:<key>` (older) — plaintext.
//!      The payload is base64-decoded when it decodes to printable ASCII,
//!      otherwise taken verbatim. It is DPAPI-encrypted in memory at load and
//!      reaches disk encrypted on the first successful write of any kind; the
//!      plaintext is gone from the file afterwards. If DPAPI cannot encrypt it,
//!      it reads as unset (`Unreadable`, fail closed) and is preserved as-is.
//!    * An UNPREFIXED string is accepted as plaintext only in the legacy
//!      top-level `<id>ApiKey` fields (the oldest builds stored raw keys there);
//!      inside `secrets`/`apiKeys` an unknown prefix is invalid.
//!    * Undecryptable blobs read as unset (`KeyStorage::Unreadable`) and are
//!      preserved verbatim on write (the file may move back to its own user).
//!      Invalid values (bad base64, unknown prefix, non-string) read as
//!      `Unreadable` and are dropped on the next write.
//! 4. `version` is ignored on load (v3 had none); v4 always writes `4`.
//! 5. Unknown top-level keys (including the legacy fields above) are not
//!    written back.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use callcore_contract::{
    AnswerStyle, Bounds, CallType, LayoutMode, Profile, Secret, ANSWER_FONT_DEFAULT,
    ANSWER_FONT_MAX, ANSWER_FONT_MIN, DEFAULT_HOTKEY, FOCUS_MAX, FONT_STEP, HOTKEY_MAX,
    MAX_PROFILES, PROFILE_NAME_MAX, PROFILE_TEXT_MAX, PROMPTER_FONT_DEFAULT, PROMPTER_FONT_MAX,
    PROMPTER_FONT_MIN,
};
use serde::Serialize;
use serde_json::{Map, Value};

pub(crate) const SCHEMA_VERSION: u32 = 4;
pub(crate) const DEFAULT_PROFILE_ID: &str = "default";
pub(crate) const DEFAULT_PROFILE_NAME: &str = "Default";
pub(crate) const UNTITLED_PROFILE_NAME: &str = "Untitled";
pub(crate) const BOUNDS_LIMIT: u32 = 100_000;

/// A secret as held in memory. Never `Debug`-prints material.
#[derive(Clone)]
pub(crate) enum StoredSecret {
    /// DPAPI blob. `readable` = unprotect succeeded when it was loaded/set.
    Blob { blob: Vec<u8>, readable: bool },
    /// Legacy plaintext that DPAPI could not encrypt. Reads as unset.
    Plain(Secret),
    /// Unparseable value. Reads as unset; dropped on the next write.
    Invalid,
}

impl std::fmt::Debug for StoredSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StoredSecret::Blob { readable: true, .. } => "Blob(encrypted)",
            StoredSecret::Blob {
                readable: false, ..
            } => "Blob(unreadable)",
            StoredSecret::Plain(_) => "Plain(***)",
            StoredSecret::Invalid => "Invalid",
        })
    }
}

/// A secret value as parsed from the file, before the keystore sees it.
pub(crate) enum RawSecret {
    Blob(Vec<u8>),
    Plain(Secret),
    Invalid,
}

/// Everything the file holds, validated.
#[derive(Clone)]
pub(crate) struct Data {
    pub revision: u64,
    pub profiles: Vec<Profile>,
    pub active_profile_id: String,
    pub llm_provider: String,
    pub answer_style: AnswerStyle,
    pub hotkey: String,
    pub always_on_top: bool,
    pub layout_mode: LayoutMode,
    pub prompter_font_px: u8,
    pub answer_font_px: u8,
    pub window_bounds: Option<Bounds>,
    pub prompter_bounds: Option<Bounds>,
    pub secrets: BTreeMap<String, StoredSecret>,
}

impl Data {
    pub(crate) fn defaults(default_provider: &str) -> Self {
        Data {
            revision: 0,
            profiles: vec![blank_profile(DEFAULT_PROFILE_ID, DEFAULT_PROFILE_NAME)],
            active_profile_id: DEFAULT_PROFILE_ID.to_string(),
            llm_provider: default_provider.to_string(),
            answer_style: AnswerStyle::Balanced,
            hotkey: DEFAULT_HOTKEY.to_string(),
            always_on_top: true,
            layout_mode: LayoutMode::Full,
            prompter_font_px: PROMPTER_FONT_DEFAULT,
            answer_font_px: ANSWER_FONT_DEFAULT,
            window_bounds: None,
            prompter_bounds: None,
            secrets: BTreeMap::new(),
        }
    }

    pub(crate) fn active_profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile_id)
            .or_else(|| self.profiles.first())
            .expect("at least one profile is an invariant")
    }
}

pub(crate) fn blank_profile(id: &str, name: &str) -> Profile {
    Profile {
        id: id.to_string(),
        name: name.to_string(),
        call_type: CallType::Behavioral,
        focus: String::new(),
        resume: String::new(),
        job_description: String::new(),
        notes: String::new(),
    }
}

pub(crate) fn valid_profile_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

pub(crate) fn valid_bounds(b: &Bounds) -> bool {
    b.width > 0 && b.width < BOUNDS_LIMIT && b.height > 0 && b.height < BOUNDS_LIMIT
}

/// Clamp to `[min,max]`, then snap down to the step grid anchored at `min`.
pub(crate) fn snap_font(v: f64, min: u8, max: u8) -> u8 {
    let c = v.clamp(f64::from(min), f64::from(max));
    let steps = ((c - f64::from(min)) / f64::from(FONT_STEP)).floor() as u8;
    min + steps * FONT_STEP
}

pub(crate) fn font_on_grid(v: u8, min: u8, max: u8) -> bool {
    (min..=max).contains(&v) && (v - min) % FONT_STEP == 0
}

// ───────────────────────────── load ─────────────────────────────

/// Parse a validated [`Data`] (secrets still raw) out of a JSON object.
/// `known_provider` decides `llmProvider` validity.
pub(crate) fn parse(
    obj: &Map<String, Value>,
    default_provider: &str,
    known_provider: &dyn Fn(&str) -> bool,
) -> (Data, BTreeMap<String, RawSecret>) {
    let mut d = Data::defaults(default_provider);

    if let Some(r) = obj.get("settingsRevision").and_then(Value::as_u64) {
        d.revision = r;
    }

    let profiles = parse_profiles(obj.get("profiles"));
    d.profiles = if profiles.is_empty() {
        let mut p = blank_profile(DEFAULT_PROFILE_ID, DEFAULT_PROFILE_NAME);
        p.resume = text_field(obj, "resume", "resume", PROFILE_TEXT_MAX);
        p.job_description = text_field(obj, "jobDescription", "job_description", PROFILE_TEXT_MAX);
        vec![p]
    } else {
        profiles
    };

    d.active_profile_id = match obj.get("activeProfileId").and_then(Value::as_str) {
        Some(id) if d.profiles.iter().any(|p| p.id == id) => id.to_string(),
        _ => d.profiles[0].id.clone(),
    };

    if let Some(p) = obj.get("llmProvider").and_then(Value::as_str) {
        if known_provider(p) {
            d.llm_provider = p.to_string();
        }
    }
    if let Some(s) = obj.get("answerStyle").and_then(Value::as_str) {
        if matches!(s, "brief" | "balanced" | "detailed") {
            d.answer_style = AnswerStyle::from_id_lossy(s);
        }
    }
    if let Some(h) = obj.get("hotkey").and_then(Value::as_str) {
        if h.chars().count() <= HOTKEY_MAX {
            d.hotkey = h.to_string();
        }
    }
    if let Some(b) = obj.get("alwaysOnTop").and_then(Value::as_bool) {
        d.always_on_top = b;
    }
    match obj.get("layoutMode").and_then(Value::as_str) {
        Some("full") => d.layout_mode = LayoutMode::Full,
        Some("prompter") => d.layout_mode = LayoutMode::Prompter,
        _ => {}
    }
    if let Some(v) = obj
        .get("prompterFontPx")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
    {
        d.prompter_font_px = snap_font(v, PROMPTER_FONT_MIN, PROMPTER_FONT_MAX);
    }
    if let Some(v) = obj
        .get("answerFontPx")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
    {
        d.answer_font_px = snap_font(v, ANSWER_FONT_MIN, ANSWER_FONT_MAX);
    }
    d.window_bounds = obj.get("windowBounds").and_then(parse_bounds);
    d.prompter_bounds = obj.get("prompterBounds").and_then(parse_bounds);

    (d, parse_secrets(obj))
}

fn get_alias<'a>(obj: &'a Map<String, Value>, camel: &str, snake: &str) -> Option<&'a Value> {
    obj.get(camel).or_else(|| obj.get(snake))
}

fn text_field(obj: &Map<String, Value>, camel: &str, snake: &str, max: usize) -> String {
    get_alias(obj, camel, snake)
        .and_then(Value::as_str)
        .map(|s| truncate_chars(s, max))
        .unwrap_or_default()
}

fn parse_profiles(v: Option<&Value>) -> Vec<Profile> {
    let Some(items) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Profile> = Vec::new();
    for item in items {
        if out.len() >= MAX_PROFILES {
            break;
        }
        let Some(o) = item.as_object() else { continue };
        let Some(id) = o.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !valid_profile_id(id) || out.iter().any(|p| p.id == id) {
            continue;
        }
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let name = truncate_chars(name, PROFILE_NAME_MAX);
        let name = name.trim_end();
        out.push(Profile {
            id: id.to_string(),
            name: if name.is_empty() {
                UNTITLED_PROFILE_NAME.to_string()
            } else {
                name.to_string()
            },
            call_type: get_alias(o, "callType", "call_type")
                .and_then(Value::as_str)
                .map(CallType::from_id_lossy)
                .unwrap_or_default(),
            focus: text_field(o, "focus", "focus", FOCUS_MAX),
            resume: text_field(o, "resume", "resume", PROFILE_TEXT_MAX),
            job_description: text_field(o, "jobDescription", "job_description", PROFILE_TEXT_MAX),
            notes: text_field(o, "notes", "notes", PROFILE_TEXT_MAX),
        });
    }
    out
}

fn parse_bounds(v: &Value) -> Option<Bounds> {
    let o = v.as_object()?;
    let int = |k: &str| o.get(k).and_then(Value::as_i64);
    let b = Bounds {
        x: i32::try_from(int("x")?).ok()?,
        y: i32::try_from(int("y")?).ok()?,
        width: u32::try_from(int("width")?).ok()?,
        height: u32::try_from(int("height")?).ok()?,
    };
    valid_bounds(&b).then_some(b)
}

fn parse_secrets(obj: &Map<String, Value>) -> BTreeMap<String, RawSecret> {
    let mut out = BTreeMap::new();
    // Lowest precedence first; later sources overwrite.
    for (k, v) in obj {
        if let Some(id) = k.strip_suffix("ApiKey") {
            if !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                if let Some(s) = parse_secret_value(v, true) {
                    out.insert(id.to_string(), s);
                }
            }
        }
    }
    for section in ["apiKeys", "secrets"] {
        if let Some(map) = obj.get(section).and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(s) = parse_secret_value(v, false) {
                    out.insert(k.clone(), s);
                }
            }
        }
    }
    out
}

/// `None` = unset (null / empty string).
pub(crate) fn parse_secret_value(v: &Value, allow_unprefixed_plain: bool) -> Option<RawSecret> {
    let s = match v {
        Value::Null => return None,
        Value::String(s) => s.trim(),
        _ => return Some(RawSecret::Invalid),
    };
    if s.is_empty() {
        return None;
    }
    let blob_payload = s.strip_prefix("dpapi:").or_else(|| s.strip_prefix("enc:"));
    if let Some(p) = blob_payload {
        return Some(match B64.decode(p.trim()) {
            Ok(b) if !b.is_empty() => RawSecret::Blob(b),
            _ => RawSecret::Invalid,
        });
    }
    if let Some(p) = s.strip_prefix("plain:") {
        let p = p.trim();
        if p.is_empty() {
            return None;
        }
        let decoded = B64
            .decode(p)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|t| !t.is_empty() && t.bytes().all(|c| c.is_ascii_graphic()));
        return Some(RawSecret::Plain(Secret::new(
            decoded.unwrap_or_else(|| p.to_string()),
        )));
    }
    if allow_unprefixed_plain {
        return Some(RawSecret::Plain(Secret::new(s)));
    }
    Some(RawSecret::Invalid)
}

// ───────────────────────────── write ─────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OnDisk<'a> {
    version: u32,
    settings_revision: u64,
    profiles: &'a [Profile],
    active_profile_id: &'a str,
    llm_provider: &'a str,
    answer_style: AnswerStyle,
    hotkey: &'a str,
    always_on_top: bool,
    layout_mode: LayoutMode,
    prompter_font_px: u8,
    answer_font_px: u8,
    window_bounds: Option<Bounds>,
    prompter_bounds: Option<Bounds>,
    secrets: BTreeMap<&'a str, String>,
}

/// Pretty JSON bytes for `d`. Blobs → `dpapi:`; unencryptable legacy plain
/// stays `plain:` (it was already on disk); invalid values are dropped.
pub(crate) fn to_json(d: &Data) -> Vec<u8> {
    let secrets = d
        .secrets
        .iter()
        .filter_map(|(k, v)| {
            let s = match v {
                StoredSecret::Blob { blob, .. } => format!("dpapi:{}", B64.encode(blob)),
                StoredSecret::Plain(p) => format!("plain:{}", B64.encode(p.expose())),
                StoredSecret::Invalid => return None,
            };
            Some((k.as_str(), s))
        })
        .collect();
    let disk = OnDisk {
        version: SCHEMA_VERSION,
        settings_revision: d.revision,
        profiles: &d.profiles,
        active_profile_id: &d.active_profile_id,
        llm_provider: &d.llm_provider,
        answer_style: d.answer_style,
        hotkey: &d.hotkey,
        always_on_top: d.always_on_top,
        layout_mode: d.layout_mode,
        prompter_font_px: d.prompter_font_px,
        answer_font_px: d.answer_font_px,
        window_bounds: d.window_bounds,
        prompter_bounds: d.prompter_bounds,
        secrets,
    };
    let mut out = serde_json::to_vec_pretty(&disk).expect("settings always serialize");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("ab", 5), "ab");
        assert_eq!(truncate_chars("😀😀😀", 2), "😀😀");
    }

    #[test]
    fn snap_font_clamps_and_floors_to_grid() {
        assert_eq!(snap_font(15.0, 14, 28), 14);
        assert_eq!(snap_font(17.0, 14, 28), 16);
        assert_eq!(snap_font(99.0, 14, 28), 28);
        assert_eq!(snap_font(-5.0, 14, 28), 14);
        assert_eq!(snap_font(21.0, 12, 22), 20);
        assert_eq!(snap_font(22.0, 12, 22), 22);
        assert_eq!(snap_font(19.9, 14, 28), 18);
    }

    #[test]
    fn profile_id_regex() {
        assert!(valid_profile_id("a"));
        assert!(valid_profile_id("A-b_9"));
        assert!(valid_profile_id(&"x".repeat(64)));
        assert!(!valid_profile_id(&"x".repeat(65)));
        assert!(!valid_profile_id(""));
        assert!(!valid_profile_id("a b"));
        assert!(!valid_profile_id("é"));
        assert!(!valid_profile_id("a/b"));
    }

    #[test]
    fn plain_prefix_decodes_base64_or_takes_verbatim() {
        let dec = |s: &str| match parse_secret_value(&Value::String(s.into()), false) {
            Some(RawSecret::Plain(p)) => p.expose().to_string(),
            _ => panic!("not plain"),
        };
        assert_eq!(
            dec(&format!("plain:{}", B64.encode("sk-ant-abc"))),
            "sk-ant-abc"
        );
        assert_eq!(dec("plain:sk-ant-api03-xyz"), "sk-ant-api03-xyz");
        // 40 hex chars are valid base64 but decode to binary -> verbatim.
        let hex = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(dec(&format!("plain:{hex}")), hex);
    }

    #[test]
    fn secret_value_edge_cases() {
        assert!(parse_secret_value(&Value::Null, false).is_none());
        assert!(parse_secret_value(&Value::String("  ".into()), false).is_none());
        assert!(parse_secret_value(&Value::String("plain:".into()), false).is_none());
        assert!(matches!(
            parse_secret_value(&Value::Bool(true), false),
            Some(RawSecret::Invalid)
        ));
        assert!(matches!(
            parse_secret_value(&Value::String("dpapi:!!!".into()), false),
            Some(RawSecret::Invalid)
        ));
        assert!(matches!(
            parse_secret_value(&Value::String("rawkey".into()), false),
            Some(RawSecret::Invalid)
        ));
        assert!(matches!(
            parse_secret_value(&Value::String("rawkey".into()), true),
            Some(RawSecret::Plain(_))
        ));
        assert!(matches!(
            parse_secret_value(&Value::String("enc:AQID".into()), false),
            Some(RawSecret::Blob(_))
        ));
    }
}
