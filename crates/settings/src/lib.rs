//! Settings store. PUBLIC API PINNED (the shell wires exactly these).
//!
//! * [`schema`] — the on-disk format, per-field load validation and every
//!   accepted v3/legacy shape (documented there).
//! * Writes are serialized behind one mutex, atomic (temp + fsync + rename
//!   with retry), and revision-checked (`apply_patch`).
//! * A corrupt/unreadable file loads as defaults and is backed up BEFORE the
//!   first write of any kind; if that backup fails, every write is refused.
//! * Secrets are DPAPI-encrypted and fail CLOSED; the view carries only
//!   `KeyStatus` (never key material).

mod fsio;
mod keystore;
mod schema;

#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

use callcore_contract::ports::{AnswerConfig, Keystore, SecretReadError, SettingsReader};
use callcore_contract::{
    AppError, Bounds, BuildInfo, HotkeyStatus, KeyStatus, KeyStorage, LayoutMode, Profile,
    ProviderInfo, Secret, SecretAction, SettingsLoadIssue, SettingsLoadStatus, SettingsPatch,
    SettingsView, ANSWER_FONT_MAX, ANSWER_FONT_MIN, FOCUS_MAX, HOTKEY_MAX, MAX_PROFILES,
    PROFILE_NAME_MAX, PROFILE_TEXT_MAX, PROMPTER_FONT_MAX, PROMPTER_FONT_MIN,
};

pub use keystore::{DpapiKeystore, DPAPI_ENTROPY};
use schema::{Data, RawSecret, StoredSecret};

/// File name inside the settings directory.
pub const SETTINGS_FILE: &str = "settings.json";
/// Secret id of the speech-to-text key.
pub const DEEPGRAM_KEY_ID: &str = "deepgram";
/// Longest API key accepted by a save.
pub const SECRET_MAX_CHARS: usize = 4096;

/// Shown when a save's `baseRevision` is stale.
pub const STALE_REVISION_MESSAGE: &str =
    "Settings changed elsewhere — nothing was saved. Your edits are still here; review them and save again.";
/// Shown when DPAPI refuses to encrypt a key.
pub const ENCRYPT_FAILED_MESSAGE: &str = "Windows could not encrypt the API key (DPAPI) — nothing was saved and your previous key is unchanged. Try again; if it keeps failing, sign out of Windows and back in.";

/// `%APPDATA%\AICallAssistant` (falls back to `<home>\AICallAssistant`).
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("AICallAssistant")
}

/// Label + where to get a key, for a secret id.
fn key_meta(id: &str) -> (String, String) {
    match id {
        DEEPGRAM_KEY_ID => ("Deepgram".into(), "https://console.deepgram.com/".into()),
        "anthropic" => (
            "Anthropic".into(),
            "https://console.anthropic.com/settings/keys".into(),
        ),
        "groq" => ("Groq".into(), "https://console.groq.com/keys".into()),
        other => (other.to_string(), String::new()),
    }
}

/// What still has to happen before the first write after a bad load.
enum PendingBackup {
    /// Corrupt: the exact bytes we read.
    Bytes(Vec<u8>),
    /// Unreadable: re-read the file at backup time.
    Reread,
}

struct WriteState {
    pending: Option<PendingBackup>,
    blocked: bool,
}

/// Thread-safe (internally synchronized). All writes are serialized, atomic
/// and revision-checked.
pub struct SettingsStore {
    path: PathBuf,
    keystore: Arc<dyn Keystore>,
    providers: Vec<ProviderInfo>,
    load_status: SettingsLoadStatus,
    data: RwLock<Data>,
    issue: RwLock<Option<SettingsLoadIssue>>,
    writer: Mutex<WriteState>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn replace<T>(l: &RwLock<T>, v: T) {
    *l.write().unwrap_or_else(|e| e.into_inner()) = v;
}

fn wipe(mut v: Vec<u8>) {
    for b in v.iter_mut() {
        // SAFETY: writing a zero into our own initialized byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

fn status_tag(s: SettingsLoadStatus) -> &'static str {
    match s {
        SettingsLoadStatus::Unreadable => "unreadable",
        SettingsLoadStatus::Corrupt => "corrupt",
        SettingsLoadStatus::Ok => "ok",
        SettingsLoadStatus::FirstRun => "firstrun",
    }
}

impl SettingsStore {
    /// Load `<dir>/settings.json` (missing = first run; corrupt/unreadable =
    /// defaults + pending backup). `providers` is the LLM registry's info list
    /// (drives the `keys` list and llmProvider validation).
    pub fn open(dir: &Path, keystore: Arc<dyn Keystore>, providers: Vec<ProviderInfo>) -> Self {
        let path = dir.join(SETTINGS_FILE);
        let default_provider =
            if providers.is_empty() || providers.iter().any(|p| p.id == "anthropic") {
                "anthropic".to_string()
            } else {
                providers[0].id.clone()
            };
        let known = |id: &str| providers.iter().any(|p| p.id == id) || id == default_provider;

        let (load_status, data, raw_secrets, pending, issue) = match fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (
                SettingsLoadStatus::FirstRun,
                Data::defaults(&default_provider),
                Default::default(),
                None,
                None,
            ),
            Err(e) => {
                tracing::warn!(error = %e, "settings file unreadable; loading defaults");
                let issue = SettingsLoadIssue {
                    status: SettingsLoadStatus::Unreadable,
                    backup_path: None,
                    writes_blocked: false,
                    message: format!(
                        "Your settings file couldn't be opened ({e}), so defaults are loaded. It will be backed up next to {} before anything is saved.",
                        path.display()
                    ),
                };
                (
                    SettingsLoadStatus::Unreadable,
                    Data::defaults(&default_provider),
                    Default::default(),
                    Some(PendingBackup::Reread),
                    Some(issue),
                )
            }
            Ok(bytes) => {
                let body = bytes
                    .strip_prefix(b"\xEF\xBB\xBF".as_slice())
                    .unwrap_or(&bytes);
                match serde_json::from_slice::<serde_json::Value>(body) {
                    Ok(serde_json::Value::Object(obj)) => {
                        let (d, raw) = schema::parse(&obj, &default_provider, &known);
                        (SettingsLoadStatus::Ok, d, raw, None, None)
                    }
                    _ => {
                        tracing::warn!("settings file is not a JSON object; loading defaults");
                        let issue = SettingsLoadIssue {
                            status: SettingsLoadStatus::Corrupt,
                            backup_path: None,
                            writes_blocked: false,
                            message: format!(
                                "Your settings file isn't valid, so defaults are loaded. The original will be backed up next to {} before anything is saved.",
                                path.display()
                            ),
                        };
                        (
                            SettingsLoadStatus::Corrupt,
                            Data::defaults(&default_provider),
                            Default::default(),
                            Some(PendingBackup::Bytes(bytes)),
                            Some(issue),
                        )
                    }
                }
            }
        };

        let mut data = data;
        for (id, raw) in raw_secrets {
            let stored = match raw {
                RawSecret::Invalid => StoredSecret::Invalid,
                RawSecret::Blob(blob) => {
                    let readable = match keystore.unprotect(&blob) {
                        Ok(plain) => {
                            wipe(plain);
                            true
                        }
                        Err(_) => false,
                    };
                    StoredSecret::Blob { blob, readable }
                }
                // Legacy plaintext: encrypt in memory now; disk follows on
                // the first successful write.
                RawSecret::Plain(p) => match keystore.protect(p.expose().as_bytes()) {
                    Ok(blob) => StoredSecret::Blob {
                        blob,
                        readable: true,
                    },
                    Err(_) => StoredSecret::Plain(p),
                },
            };
            data.secrets.insert(id, stored);
        }

        SettingsStore {
            path,
            keystore,
            providers,
            load_status,
            data: RwLock::new(data),
            issue: RwLock::new(issue),
            writer: Mutex::new(WriteState {
                pending,
                blocked: false,
            }),
        }
    }

    /// How the file loaded at startup (for diagnostics).
    pub fn load_status(&self) -> SettingsLoadStatus {
        self.load_status
    }

    /// The load issue (corrupt/unreadable file), with backup progress.
    pub fn load_issue(&self) -> Option<SettingsLoadIssue> {
        read(&self.issue).clone()
    }

    /// Current `settingsRevision`.
    pub fn revision(&self) -> u64 {
        read(&self.data).revision
    }

    /// The view. Hotkey fields are placeholders (`Disabled`/false/None) and
    /// `build` is `BuildInfo` default-ish — the shell overwrites both.
    pub fn view(&self) -> SettingsView {
        let d = read(&self.data);
        SettingsView {
            settings_revision: d.revision,
            profiles: d.profiles.clone(),
            active_profile_id: d.active_profile_id.clone(),
            llm_provider: d.llm_provider.clone(),
            answer_style: d.answer_style,
            hotkey: d.hotkey.clone(),
            always_on_top: d.always_on_top,
            layout_mode: d.layout_mode,
            prompter_font_px: d.prompter_font_px,
            answer_font_px: d.answer_font_px,
            keys: self.key_statuses(&d),
            providers: self.providers.clone(),
            hotkey_registered: false,
            hotkey_status: HotkeyStatus::Disabled,
            hotkey_message: None,
            load_issue: read(&self.issue).clone(),
            settings_path: self.path.display().to_string(),
            build: BuildInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                git_revision: "unknown".into(),
                dirty: false,
                build_time: "unknown".into(),
            },
        }
    }

    /// Secret ids the app knows: deepgram, then each provider's key id.
    fn known_key_ids(&self) -> Vec<String> {
        let mut ids = vec![DEEPGRAM_KEY_ID.to_string()];
        for p in &self.providers {
            if !ids.contains(&p.key_id) {
                ids.push(p.key_id.clone());
            }
        }
        ids
    }

    fn key_statuses(&self, d: &Data) -> Vec<KeyStatus> {
        self.known_key_ids()
            .into_iter()
            .map(|id| {
                let (has_key, storage) = match d.secrets.get(&id) {
                    None => (false, KeyStorage::Unset),
                    Some(StoredSecret::Blob { readable: true, .. }) => {
                        (true, KeyStorage::Encrypted)
                    }
                    Some(_) => (false, KeyStorage::Unreadable),
                };
                let (label, get_key_url) = key_meta(&id);
                KeyStatus {
                    id,
                    label,
                    has_key,
                    storage,
                    get_key_url,
                }
            })
            .collect()
    }

    /// Validate + apply. Stale `base_revision` -> Err, nothing applied.
    /// Secret encryption failure -> Err, nothing applied (fail closed).
    pub fn apply_patch(&self, patch: SettingsPatch) -> Result<SettingsView, AppError> {
        let mut w = lock(&self.writer);
        self.check_blocked(&w)?;
        let mut next = read(&self.data).clone();
        if patch.base_revision != next.revision {
            return Err(AppError::internal(STALE_REVISION_MESSAGE));
        }
        self.apply_fields(&mut next, &patch)?;
        self.apply_secrets(&mut next, &patch)?;
        next.revision = next.revision.saturating_add(1);
        self.persist(&mut w, &mut next)?;
        replace(&self.data, next);
        drop(w);
        Ok(self.view())
    }

    fn apply_fields(&self, next: &mut Data, patch: &SettingsPatch) -> Result<(), AppError> {
        if let Some(profiles) = &patch.profiles {
            next.profiles = validate_profiles(profiles)?;
            if !next.profiles.iter().any(|p| p.id == next.active_profile_id) {
                next.active_profile_id = next.profiles[0].id.clone();
            }
        }
        if let Some(id) = &patch.active_profile_id {
            if !next.profiles.iter().any(|p| &p.id == id) {
                return Err(AppError::internal(
                    "The selected profile doesn't exist. Nothing was saved.",
                ));
            }
            next.active_profile_id = id.clone();
        }
        if let Some(p) = &patch.llm_provider {
            if !self.providers.iter().any(|i| &i.id == p) {
                return Err(AppError::internal(
                    "Unknown answer provider. Nothing was saved.",
                ));
            }
            next.llm_provider = p.clone();
        }
        if let Some(s) = patch.answer_style {
            next.answer_style = s;
        }
        if let Some(h) = &patch.hotkey {
            if h.chars().count() > HOTKEY_MAX {
                return Err(AppError::internal(format!(
                    "The hotkey is too long (max {HOTKEY_MAX} characters). Nothing was saved."
                )));
            }
            next.hotkey = h.clone();
        }
        if let Some(b) = patch.always_on_top {
            next.always_on_top = b;
        }
        if let Some(m) = patch.layout_mode {
            next.layout_mode = m;
        }
        if let Some(v) = patch.prompter_font_px {
            if !schema::font_on_grid(v, PROMPTER_FONT_MIN, PROMPTER_FONT_MAX) {
                return Err(AppError::internal(format!(
                    "Prompter text size must be an even size from {PROMPTER_FONT_MIN} to {PROMPTER_FONT_MAX}. Nothing was saved."
                )));
            }
            next.prompter_font_px = v;
        }
        if let Some(v) = patch.answer_font_px {
            if !schema::font_on_grid(v, ANSWER_FONT_MIN, ANSWER_FONT_MAX) {
                return Err(AppError::internal(format!(
                    "Answer text size must be an even size from {ANSWER_FONT_MIN} to {ANSWER_FONT_MAX}. Nothing was saved."
                )));
            }
            next.answer_font_px = v;
        }
        Ok(())
    }

    fn apply_secrets(&self, next: &mut Data, patch: &SettingsPatch) -> Result<(), AppError> {
        let Some(changes) = &patch.secrets else {
            return Ok(());
        };
        let known = self.known_key_ids();
        // Validate everything before encrypting anything.
        for c in changes {
            if !known.contains(&c.key_id) {
                return Err(AppError::internal(format!(
                    "Unknown API key \"{}\". Nothing was saved.",
                    c.key_id
                )));
            }
            if let SecretAction::Set { value } = &c.action {
                let v = value.trim();
                if v.chars().count() > SECRET_MAX_CHARS {
                    return Err(AppError::internal(
                        "That API key is too long. Nothing was saved.",
                    ));
                }
                if !v.bytes().all(|b| b.is_ascii_graphic()) {
                    return Err(AppError::internal(
                        "API keys must be plain ASCII with no spaces — check for smart quotes or stray characters. Nothing was saved.",
                    ));
                }
            }
        }
        for c in changes {
            match &c.action {
                SecretAction::Set { value } => {
                    let v = value.trim();
                    if v.is_empty() {
                        continue; // emptying the field never removes a key
                    }
                    let blob = self.keystore.protect(v.as_bytes()).map_err(|_| {
                        tracing::warn!(key_id = %c.key_id, "DPAPI protect failed; save refused");
                        AppError::internal(ENCRYPT_FAILED_MESSAGE)
                    })?;
                    next.secrets.insert(
                        c.key_id.clone(),
                        StoredSecret::Blob {
                            blob,
                            readable: true,
                        },
                    );
                }
                SecretAction::Remove => {
                    next.secrets.remove(&c.key_id);
                }
            }
        }
        Ok(())
    }

    pub fn bounds(&self, layout: LayoutMode) -> Option<Bounds> {
        let d = read(&self.data);
        match layout {
            LayoutMode::Full => d.window_bounds,
            LayoutMode::Prompter => d.prompter_bounds,
        }
    }

    /// Geometry autosave. Obeys the backup-before-first-write rule; does NOT
    /// bump `settingsRevision`.
    pub fn save_bounds(&self, layout: LayoutMode, bounds: Bounds) -> Result<(), AppError> {
        if !schema::valid_bounds(&bounds) {
            return Err(AppError::internal(
                "Window size out of range; geometry not saved.",
            ));
        }
        let mut w = lock(&self.writer);
        self.check_blocked(&w)?;
        let mut next = read(&self.data).clone();
        match layout {
            LayoutMode::Full => next.window_bounds = Some(bounds),
            LayoutMode::Prompter => next.prompter_bounds = Some(bounds),
        }
        self.persist(&mut w, &mut next)?;
        replace(&self.data, next);
        Ok(())
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    fn blocked_error(&self) -> AppError {
        AppError::internal(format!(
            "Settings can't be saved: the settings file that failed to load couldn't be backed up first, so it was left untouched. Nothing was saved. Move or fix {} (or free disk space / check folder permissions), then restart the app.",
            self.path.display()
        ))
    }

    fn check_blocked(&self, w: &WriteState) -> Result<(), AppError> {
        if w.blocked {
            Err(self.blocked_error())
        } else {
            Ok(())
        }
    }

    /// Backup (if pending) → migrate plaintext → atomic write. Under the writer lock.
    fn persist(&self, w: &mut WriteState, next: &mut Data) -> Result<(), AppError> {
        if let Some(p) = w.pending.take() {
            let tag = status_tag(self.load_status);
            let res = match &p {
                PendingBackup::Bytes(b) => fsio::write_backup(&self.path, tag, b),
                PendingBackup::Reread => {
                    fs::read(&self.path).and_then(|b| fsio::write_backup(&self.path, tag, &b))
                }
            };
            let mut issue = lock_issue(&self.issue);
            match res {
                Ok(bak) => {
                    tracing::info!(backup = %bak.display(), "backed up bad settings file");
                    if let Some(i) = issue.as_mut() {
                        i.backup_path = Some(bak.display().to_string());
                        i.message = format!(
                            "Your settings file couldn't be loaded, so defaults were used. The original was backed up to {}.",
                            bak.display()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "settings backup failed; writes blocked");
                    w.blocked = true;
                    let err = self.blocked_error();
                    if let Some(i) = issue.as_mut() {
                        i.writes_blocked = true;
                        i.message = err.message.clone();
                    }
                    return Err(err);
                }
            }
        }
        // Legacy plaintext that DPAPI refused at load: try again now.
        for v in next.secrets.values_mut() {
            if let StoredSecret::Plain(p) = v {
                if let Ok(blob) = self.keystore.protect(p.expose().as_bytes()) {
                    *v = StoredSecret::Blob {
                        blob,
                        readable: true,
                    };
                }
            }
        }
        let bytes = schema::to_json(next);
        fsio::atomic_write(&self.path, &bytes).map_err(|e| {
            tracing::warn!(error = %e, "settings write failed");
            AppError::internal(format!(
                "Couldn't save settings to {} ({e}). Nothing was saved.",
                self.path.display()
            ))
        })
    }
}

fn lock_issue(
    l: &RwLock<Option<SettingsLoadIssue>>,
) -> std::sync::RwLockWriteGuard<'_, Option<SettingsLoadIssue>> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// Strict save-time validation (only profile names are normalized: trimmed).
fn validate_profiles(profiles: &[Profile]) -> Result<Vec<Profile>, AppError> {
    let nothing = "Nothing was saved.";
    if profiles.is_empty() {
        return Err(AppError::internal(format!(
            "Keep at least one profile. {nothing}"
        )));
    }
    if profiles.len() > MAX_PROFILES {
        return Err(AppError::internal(format!(
            "At most {MAX_PROFILES} profiles are allowed. {nothing}"
        )));
    }
    let mut out: Vec<Profile> = Vec::with_capacity(profiles.len());
    for p in profiles {
        if !schema::valid_profile_id(&p.id) {
            return Err(AppError::internal(format!(
                "A profile has an invalid id. {nothing}"
            )));
        }
        if out.iter().any(|o| o.id == p.id) {
            return Err(AppError::internal(format!(
                "Two profiles share the same id. {nothing}"
            )));
        }
        let name = p.name.trim();
        if name.is_empty() {
            return Err(AppError::internal(format!(
                "Every profile needs a name. {nothing}"
            )));
        }
        let too_long = |label: &str, max: usize| {
            AppError::internal(format!(
                "Profile \"{}\": {label} is longer than {max} characters. {nothing}",
                schema::truncate_chars(name, PROFILE_NAME_MAX)
            ))
        };
        if name.chars().count() > PROFILE_NAME_MAX {
            return Err(too_long("the name", PROFILE_NAME_MAX));
        }
        if p.focus.chars().count() > FOCUS_MAX {
            return Err(too_long("focus", FOCUS_MAX));
        }
        for (label, text) in [
            ("the resume", &p.resume),
            ("the job description", &p.job_description),
            ("notes", &p.notes),
        ] {
            if text.chars().count() > PROFILE_TEXT_MAX {
                return Err(too_long(label, PROFILE_TEXT_MAX));
            }
        }
        let mut p = p.clone();
        p.name = name.to_string();
        out.push(p);
    }
    Ok(out)
}

impl SettingsReader for SettingsStore {
    fn answer_config(&self) -> AnswerConfig {
        let d = read(&self.data);
        let profile = d.active_profile().clone();
        AnswerConfig {
            call_type: profile.call_type,
            profile,
            style: d.answer_style,
            provider_id: d.llm_provider.clone(),
        }
    }

    fn get_secret(&self, key_id: &str) -> Result<Option<Secret>, SecretReadError> {
        let blob = match read(&self.data).secrets.get(key_id) {
            Some(StoredSecret::Blob {
                blob,
                readable: true,
            }) => blob.clone(),
            _ => return Ok(None),
        };
        let plain = match self.keystore.unprotect(&blob) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        match String::from_utf8(plain) {
            Ok(s) => {
                let secret = Secret::new(s.trim());
                drop(Secret::new(s));
                Ok((!secret.is_blank()).then_some(secret))
            }
            Err(e) => {
                wipe(e.into_bytes());
                Ok(None)
            }
        }
    }
}

impl std::fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = read(&self.data);
        let secrets: Vec<(&str, &StoredSecret)> =
            d.secrets.iter().map(|(k, v)| (k.as_str(), v)).collect();
        f.debug_struct("SettingsStore")
            .field("path", &self.path)
            .field("load_status", &self.load_status)
            .field("revision", &d.revision)
            .field("profiles", &d.profiles.len())
            .field("active_profile_id", &d.active_profile_id)
            .field("llm_provider", &d.llm_provider)
            .field("secrets", &secrets)
            .finish_non_exhaustive()
    }
}
