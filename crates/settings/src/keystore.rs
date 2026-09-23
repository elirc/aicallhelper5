//! Windows DPAPI keystore (current-user scope), failing CLOSED.
//!
//! New blobs are protected WITH the optional entropy [`DPAPI_ENTROPY`]. The v3
//! Python app protected WITHOUT entropy, so `unprotect` tries with entropy
//! first and falls back to no entropy. Output buffers allocated by DPAPI are
//! released with `LocalFree`. On non-Windows targets every call errors — there
//! is no silent plaintext fallback anywhere in this crate.

use callcore_contract::ports::{Keystore, KeystoreError};

/// Optional DPAPI entropy mixed into every blob this version writes.
pub const DPAPI_ENTROPY: &[u8] = b"AICallAssistant.v4";

/// DPAPI (CryptProtectData, current-user scope). Fails closed.
#[derive(Debug, Clone, Copy, Default)]
pub struct DpapiKeystore;

impl Keystore for DpapiKeystore {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        imp::protect(plaintext, Some(DPAPI_ENTROPY))
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        match imp::unprotect(blob, Some(DPAPI_ENTROPY)) {
            Ok(v) => Ok(v),
            // v3 blobs carry no entropy.
            Err(_) => imp::unprotect(blob, None),
        }
    }
}

#[cfg(windows)]
mod imp {
    use callcore_contract::ports::KeystoreError;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob_of(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, KeystoreError> {
        let len = u32::try_from(data.len())
            .map_err(|_| KeystoreError("value too large to encrypt".into()))?;
        Ok(CRYPT_INTEGER_BLOB {
            cbData: len,
            pbData: data.as_ptr().cast_mut(),
        })
    }

    /// Copy the DPAPI-allocated output out and free it.
    fn take_output(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI succeeded and handed us `cbData` bytes at `pbData`,
        // allocated with LocalAlloc; we copy them before freeing exactly once.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: pbData was allocated by DPAPI with LocalAlloc and is not used afterwards.
        unsafe {
            let _ = LocalFree(HLOCAL(out.pbData.cast()));
        }
        bytes
    }

    pub(super) fn protect(plain: &[u8], entropy: Option<&[u8]>) -> Result<Vec<u8>, KeystoreError> {
        let input = blob_of(plain)?;
        let ent = entropy.map(blob_of).transpose()?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: all pointers reference live local buffers for the duration of the call.
        let res = unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                ent.as_ref().map(|e| e as *const CRYPT_INTEGER_BLOB),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        match res {
            Ok(()) if !out.pbData.is_null() => Ok(take_output(out)),
            Ok(()) => Err(KeystoreError("Windows DPAPI returned no data".into())),
            Err(e) => {
                let _ = take_output(out);
                Err(KeystoreError(format!(
                    "Windows DPAPI could not encrypt (0x{:08X})",
                    e.code().0
                )))
            }
        }
    }

    pub(super) fn unprotect(blob: &[u8], entropy: Option<&[u8]>) -> Result<Vec<u8>, KeystoreError> {
        let input = blob_of(blob)?;
        let ent = entropy.map(blob_of).transpose()?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: all pointers reference live local buffers for the duration of the call.
        let res = unsafe {
            CryptUnprotectData(
                &input,
                None,
                ent.as_ref().map(|e| e as *const CRYPT_INTEGER_BLOB),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        match res {
            Ok(()) if !out.pbData.is_null() => Ok(take_output(out)),
            Ok(()) => Ok(Vec::new()),
            Err(e) => {
                let _ = take_output(out);
                Err(KeystoreError(format!(
                    "Windows DPAPI could not decrypt (0x{:08X})",
                    e.code().0
                )))
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use callcore_contract::ports::KeystoreError;

    pub(super) fn protect(_: &[u8], _: Option<&[u8]>) -> Result<Vec<u8>, KeystoreError> {
        Err(KeystoreError("DPAPI is only available on Windows".into()))
    }

    pub(super) fn unprotect(_: &[u8], _: Option<&[u8]>) -> Result<Vec<u8>, KeystoreError> {
        Err(KeystoreError("DPAPI is only available on Windows".into()))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip_with_entropy() {
        let ks = DpapiKeystore;
        let blob = ks.protect(b"dg-secret-123").unwrap();
        assert!(!blob.windows(13).any(|w| w == b"dg-secret-123"));
        assert_eq!(ks.unprotect(&blob).unwrap(), b"dg-secret-123");
    }

    #[test]
    fn dpapi_reads_v3_blob_without_entropy() {
        let blob = imp::protect(b"legacy-key", None).unwrap();
        assert_eq!(DpapiKeystore.unprotect(&blob).unwrap(), b"legacy-key");
    }

    #[test]
    fn dpapi_rejects_garbage_blob() {
        assert!(DpapiKeystore.unprotect(b"not a dpapi blob").is_err());
    }
}
