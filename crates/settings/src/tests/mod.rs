//! Store tests. Everything runs in tempdirs with a fake keystore (plus one
//! real DPAPI roundtrip under cfg(windows)).

mod backup;
mod load;
mod migrate;
mod secrets;
mod write;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use callcore_contract::ports::{Keystore, KeystoreError};
use callcore_contract::{ProviderInfo, SettingsPatch};
use serde_json::Value;

use crate::SettingsStore;

/// Reversible, clearly-not-plaintext "encryption" with switchable failures.
#[derive(Default)]
pub(crate) struct FakeKeystore {
    pub fail_protect: AtomicBool,
    pub fail_unprotect: AtomicBool,
    pub protect_calls: AtomicUsize,
}

const FAKE_MAGIC: &[u8] = b"FK1:";

pub(crate) fn fake_encrypt(plain: &[u8]) -> Vec<u8> {
    let mut v = FAKE_MAGIC.to_vec();
    v.extend(plain.iter().map(|b| b ^ 0x5A));
    v
}

impl Keystore for FakeKeystore {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        self.protect_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_protect.load(Ordering::SeqCst) {
            return Err(KeystoreError("fake protect failure".into()));
        }
        Ok(fake_encrypt(plaintext))
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        if self.fail_unprotect.load(Ordering::SeqCst) {
            return Err(KeystoreError("fake unprotect failure".into()));
        }
        match blob.strip_prefix(FAKE_MAGIC) {
            Some(rest) => Ok(rest.iter().map(|b| b ^ 0x5A).collect()),
            None => Err(KeystoreError("not a fake blob".into())),
        }
    }
}

pub(crate) fn providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            id: "anthropic".into(),
            display_name: "Claude Haiku 4.5 (recommended)".into(),
            key_id: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
        },
        ProviderInfo {
            id: "groq".into(),
            display_name: "Groq".into(),
            key_id: "groq".into(),
            model: "llama".into(),
        },
    ]
}

pub(crate) struct Env {
    pub dir: tempfile::TempDir,
    pub ks: Arc<FakeKeystore>,
}

impl Env {
    pub fn new() -> Self {
        Env {
            dir: tempfile::tempdir().unwrap(),
            ks: Arc::new(FakeKeystore::default()),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().join("settings.json")
    }

    pub fn open(&self) -> SettingsStore {
        SettingsStore::open(self.dir.path(), self.ks.clone(), providers())
    }

    pub fn write_json(&self, v: &Value) {
        std::fs::write(self.path(), serde_json::to_vec_pretty(v).unwrap()).unwrap();
    }

    pub fn write_raw(&self, bytes: &[u8]) {
        std::fs::write(self.path(), bytes).unwrap();
    }

    pub fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.path()).unwrap()
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.bytes()).unwrap()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes()).unwrap()
    }

    /// File names in the settings dir other than settings.json.
    pub fn others(&self) -> Vec<String> {
        list_others(self.dir.path())
    }
}

pub(crate) fn list_others(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "settings.json")
        .collect();
    v.sort();
    v
}

/// `dpapi:` value for the fake keystore.
pub(crate) fn fake_dpapi(plain: &str) -> String {
    format!("dpapi:{}", B64.encode(fake_encrypt(plain.as_bytes())))
}

pub(crate) fn patch(base: u64) -> SettingsPatch {
    SettingsPatch {
        base_revision: base,
        ..Default::default()
    }
}

pub(crate) fn profile(id: &str, name: &str) -> callcore_contract::Profile {
    callcore_contract::Profile {
        id: id.into(),
        name: name.into(),
        call_type: callcore_contract::CallType::Behavioral,
        focus: String::new(),
        resume: String::new(),
        job_description: String::new(),
        notes: String::new(),
    }
}
