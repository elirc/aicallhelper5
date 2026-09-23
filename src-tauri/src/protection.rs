//! Content protection — the moat (spec §11). `SetWindowDisplayAffinity(hwnd,
//! WDA_EXCLUDEFROMCAPTURE = 0x11)` and a `GetWindowDisplayAffinity` read-back:
//! the verdict is `Protected` ONLY when Windows reports exactly 0x11.
//! There is deliberately no command that lets the page touch any of this.

use callcore_contract::ports::DisplayAffinity;
use callcore_contract::Protection;

pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

/// Map a read-back to a verdict. `None` = the read itself failed.
pub fn verdict_from_readback(read_back: Option<u32>) -> Protection {
    match read_back {
        Some(WDA_EXCLUDEFROMCAPTURE) => Protection::Protected,
        _ => Protection::Unprotected,
    }
}

/// Apply, then verify. An apply error still verifies (the read-back is the
/// source of truth); a verify error is `Unprotected`.
pub fn apply_and_verify(aff: &dyn DisplayAffinity) -> Protection {
    if let Err(e) = aff.apply() {
        tracing::warn!(error = %e, "SetWindowDisplayAffinity failed");
    }
    match aff.verify() {
        Ok(true) => Protection::Protected,
        Ok(false) => Protection::Unprotected,
        Err(e) => {
            tracing::warn!(error = %e, "GetWindowDisplayAffinity failed");
            Protection::Unprotected
        }
    }
}

/// `DisplayAffinity` over a raw HWND. Must be used on the UI thread.
pub struct HwndAffinity {
    hwnd: isize,
}

impl HwndAffinity {
    pub fn new(hwnd: isize) -> Self {
        Self { hwnd }
    }

    /// Raw read-back (None when the call fails).
    #[cfg(windows)]
    pub fn read_back(&self) -> Option<u32> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowDisplayAffinity;
        let mut aff: u32 = 0;
        // SAFETY: `hwnd` is our live top-level window; `aff` is a valid
        // out-pointer for the duration of the call.
        let r = unsafe {
            GetWindowDisplayAffinity(HWND(self.hwnd as *mut core::ffi::c_void), &mut aff)
        };
        r.ok().map(|_| aff)
    }

    #[cfg(not(windows))]
    pub fn read_back(&self) -> Option<u32> {
        None
    }
}

impl DisplayAffinity for HwndAffinity {
    #[cfg(windows)]
    fn apply(&self) -> Result<(), String> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE as WDA,
        };
        // SAFETY: plain Win32 call on our own live window handle.
        unsafe { SetWindowDisplayAffinity(HWND(self.hwnd as *mut core::ffi::c_void), WDA) }
            .map_err(|e| e.message())
    }

    #[cfg(not(windows))]
    fn apply(&self) -> Result<(), String> {
        Err("content protection is only available on Windows".into())
    }

    fn verify(&self) -> Result<bool, String> {
        match self.read_back() {
            Some(v) => Ok(verdict_from_readback(Some(v)) == Protection::Protected),
            None => Err("could not read the window display affinity".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn only_0x11_is_protected() {
        assert_eq!(verdict_from_readback(Some(0x11)), Protection::Protected);
        // WDA_NONE and WDA_MONITOR (the pre-2004 black-box mode) are not the promise.
        assert_eq!(verdict_from_readback(Some(0x00)), Protection::Unprotected);
        assert_eq!(verdict_from_readback(Some(0x01)), Protection::Unprotected);
        assert_eq!(verdict_from_readback(None), Protection::Unprotected);
    }

    struct FakeAffinity {
        applied: Mutex<u32>,
        apply_result: Result<(), String>,
        store: bool,
        readable: bool,
    }

    impl DisplayAffinity for FakeAffinity {
        fn apply(&self) -> Result<(), String> {
            if self.store {
                *self.applied.lock().unwrap() = WDA_EXCLUDEFROMCAPTURE;
            }
            self.apply_result.clone()
        }
        fn verify(&self) -> Result<bool, String> {
            if !self.readable {
                return Err("nope".into());
            }
            Ok(*self.applied.lock().unwrap() == WDA_EXCLUDEFROMCAPTURE)
        }
    }

    fn fake(apply_result: Result<(), String>, store: bool, readable: bool) -> FakeAffinity {
        FakeAffinity {
            applied: Mutex::new(0),
            apply_result,
            store,
            readable,
        }
    }

    #[test]
    fn verdict_trusts_the_readback_not_the_apply_result() {
        // Apply claims success but Windows did not store it: NOT protected.
        assert_eq!(
            apply_and_verify(&fake(Ok(()), false, true)),
            Protection::Unprotected
        );
        // Apply reported an error but the read-back says 0x11: protected.
        assert_eq!(
            apply_and_verify(&fake(Err("x".into()), true, true)),
            Protection::Protected
        );
        assert_eq!(
            apply_and_verify(&fake(Ok(()), true, true)),
            Protection::Protected
        );
        // Unreadable: never claim protection.
        assert_eq!(
            apply_and_verify(&fake(Ok(()), true, false)),
            Protection::Unprotected
        );
    }

    #[cfg(windows)]
    #[test]
    fn invalid_hwnd_is_unprotected() {
        let aff = HwndAffinity::new(0);
        assert_eq!(apply_and_verify(&aff), Protection::Unprotected);
    }
}
