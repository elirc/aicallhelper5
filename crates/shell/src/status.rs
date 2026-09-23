//! Status hub: core state, protection verdict and the latest session status,
//! behind ONE revision counter (spec §9 snapshot rules). The revision bumps on
//! every actual change; protection/core events carry the revision returned by
//! the setter, so the page can order snapshots and events against each other.

use std::sync::{Mutex, MutexGuard};

use callcore_contract::{AppError, CoreState, Phase, Protection, SessionStatus, StatusSnapshot};

#[derive(Debug, Clone)]
struct State {
    revision: u64,
    core: CoreState,
    core_error: Option<AppError>,
    protection: Protection,
    session: SessionStatus,
}

pub struct StatusHub {
    inner: Mutex<State>,
}

impl Default for StatusHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusHub {
    /// Core `Starting`, protection `Unknown`, session idle, revision 1.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(State {
                revision: 1,
                core: CoreState::Starting,
                core_error: None,
                protection: Protection::Unknown,
                session: SessionStatus::idle(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn revision(&self) -> u64 {
        self.lock().revision
    }

    pub fn core_state(&self) -> CoreState {
        self.lock().core
    }

    pub fn protection(&self) -> Protection {
        self.lock().protection
    }

    /// Returns the revision to put in `core:ready`.
    pub fn set_core_ready(&self) -> u64 {
        let mut s = self.lock();
        if s.core != CoreState::Ready || s.core_error.is_some() {
            s.core = CoreState::Ready;
            s.core_error = None;
            s.revision += 1;
        }
        s.revision
    }

    /// Returns the revision to put in `core:failed`.
    pub fn set_core_failed(&self, error: AppError) -> u64 {
        let mut s = self.lock();
        if s.core != CoreState::Failed || s.core_error.as_ref() != Some(&error) {
            s.core = CoreState::Failed;
            s.core_error = Some(error);
            s.revision += 1;
        }
        s.revision
    }

    /// Returns `(revision, changed)`. The caller emits `protection:ok` /
    /// `protection:failed` with that revision either way (a re-verify that
    /// confirms the same verdict is still news for a freshly loaded page).
    pub fn set_protection(&self, protection: Protection) -> (u64, bool) {
        let mut s = self.lock();
        let changed = s.protection != protection;
        if changed {
            s.protection = protection;
            s.revision += 1;
        }
        (s.revision, changed)
    }

    /// Returns the (possibly bumped) revision.
    pub fn set_session(&self, session: SessionStatus) -> u64 {
        let mut s = self.lock();
        if s.session != session {
            s.session = session;
            s.revision += 1;
        }
        s.revision
    }

    pub fn snapshot(&self) -> StatusSnapshot {
        let s = self.lock();
        StatusSnapshot {
            revision: s.revision,
            core: s.core,
            core_error: s.core_error.clone(),
            protection: s.protection,
            session: s.session.clone(),
        }
    }

    /// Snapshot for when the session could not be read in time: phase
    /// `unknown` (the page then waits and asks again). The revision is the
    /// current one, NOT bumped — "unknown" is not a state change.
    pub fn snapshot_unknown_session(&self) -> StatusSnapshot {
        let mut snap = self.snapshot();
        snap.session = SessionStatus {
            id: snap.session.id,
            phase: Phase::Unknown,
            recording: None,
        };
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callcore_contract::{RecordingInfo, SessionId};

    fn recording(id: &str) -> SessionStatus {
        SessionStatus {
            id: Some(SessionId(id.into())),
            phase: Phase::Recording,
            recording: Some(RecordingInfo {
                deadline_ms: 1_000,
                cap_ms: 120_000,
            }),
        }
    }

    #[test]
    fn starts_starting_unknown_idle() {
        let hub = StatusHub::new();
        let s = hub.snapshot();
        assert_eq!(s.core, CoreState::Starting);
        assert_eq!(s.protection, Protection::Unknown);
        assert_eq!(s.session, SessionStatus::idle());
        assert!(s.core_error.is_none());
    }

    #[test]
    fn every_change_bumps_the_revision_and_no_op_does_not() {
        let hub = StatusHub::new();
        let r0 = hub.revision();
        let r1 = hub.set_core_ready();
        assert!(r1 > r0);
        assert_eq!(hub.set_core_ready(), r1, "same state: no bump");
        let (r2, changed) = hub.set_protection(Protection::Protected);
        assert!(changed && r2 > r1);
        let (r2b, changed) = hub.set_protection(Protection::Protected);
        assert!(!changed);
        assert_eq!(r2b, r2);
        let r3 = hub.set_session(recording("s1"));
        assert!(r3 > r2);
        assert_eq!(hub.set_session(recording("s1")), r3);
        let r4 = hub.set_session(SessionStatus::idle());
        assert!(r4 > r3);
        assert_eq!(hub.snapshot().revision, r4);
    }

    #[test]
    fn core_failed_carries_the_error() {
        let hub = StatusHub::new();
        let r = hub.set_core_failed(AppError::internal("boom"));
        let s = hub.snapshot();
        assert_eq!(s.revision, r);
        assert_eq!(s.core, CoreState::Failed);
        assert_eq!(s.core_error.unwrap().message, "boom");
    }

    #[test]
    fn protection_regression_is_a_change() {
        let hub = StatusHub::new();
        let (a, _) = hub.set_protection(Protection::Protected);
        let (b, changed) = hub.set_protection(Protection::Unprotected);
        assert!(changed && b > a);
        assert_eq!(hub.protection(), Protection::Unprotected);
    }

    #[test]
    fn unknown_session_snapshot_keeps_revision_and_id() {
        let hub = StatusHub::new();
        let r = hub.set_session(recording("s4"));
        let s = hub.snapshot_unknown_session();
        assert_eq!(s.revision, r);
        assert_eq!(s.session.phase, Phase::Unknown);
        assert_eq!(s.session.id, Some(SessionId("s4".into())));
        assert_eq!(hub.revision(), r);
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let hub = StatusHub::new();
        let v = serde_json::to_value(hub.snapshot()).unwrap();
        assert!(v.get("coreError").is_some());
        assert_eq!(v["core"], "starting");
    }
}
