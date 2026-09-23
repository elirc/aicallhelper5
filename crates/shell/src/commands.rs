//! Command-boundary helpers (spec §9): nothing panics or hangs across the
//! IPC boundary, and commands that need the core wait for it only while it
//! is starting.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use callcore_contract::config::{COMMAND_TIMEOUT, CORE_READY_WAIT};
use callcore_contract::{copy, AppError, CmdResult};
use futures_util::FutureExt;
use tokio::sync::watch;

pub const TIMEOUT_MESSAGE: &str = "The app took too long to respond (30 s). Try again.";
pub const PANIC_MESSAGE: &str =
    "Something went wrong inside the app. Try again; if it keeps happening, copy diagnostics from Settings.";

/// Bound `fut` by `limit`; a timeout becomes `internal`.
pub async fn with_timeout<T, F>(limit: Duration, fut: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>>,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(AppError::internal(TIMEOUT_MESSAGE)),
    }
}

/// Run `fut`, turning a panic into `internal` (the payload is NOT included —
/// it could in theory carry user text).
pub async fn catch_panic<T, F>(fut: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(_) => {
            tracing::error!("command panicked");
            Err(AppError::internal(PANIC_MESSAGE))
        }
    }
}

/// The standard wrapper for every Tauri command: panic-catch + 30 s timeout
/// + `CmdResult` envelope.
pub async fn guarded<T, F>(name: &'static str, fut: F) -> CmdResult<T>
where
    F: Future<Output = Result<T, AppError>>,
{
    guarded_with(name, COMMAND_TIMEOUT, fut).await
}

pub async fn guarded_with<T, F>(name: &'static str, limit: Duration, fut: F) -> CmdResult<T>
where
    F: Future<Output = Result<T, AppError>>,
{
    let r = with_timeout(limit, catch_panic(fut)).await;
    if let Err(e) = &r {
        // Code only; messages are user copy and may quote provider snippets.
        tracing::debug!(command = name, code = ?e.code, "command failed");
    }
    r.into()
}

// ───────────────────────────── core gate ─────────────────────────────

enum GateState<T> {
    Starting,
    Ready(Arc<T>),
    Failed(AppError),
}

impl<T> Clone for GateState<T> {
    fn clone(&self) -> Self {
        match self {
            GateState::Starting => GateState::Starting,
            GateState::Ready(c) => GateState::Ready(Arc::clone(c)),
            GateState::Failed(e) => GateState::Failed(e.clone()),
        }
    }
}

/// Holds the heavy core once it is built. Commands call [`CoreGate::get`]:
/// it waits (≤ `CORE_READY_WAIT`) while the core is starting and fails
/// immediately with actionable copy once startup failed.
pub struct CoreGate<T> {
    tx: watch::Sender<GateState<T>>,
    max_wait: Duration,
}

impl<T: Send + Sync + 'static> Default for CoreGate<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Sync + 'static> CoreGate<T> {
    pub fn new() -> Self {
        Self::with_max_wait(CORE_READY_WAIT)
    }

    pub fn with_max_wait(max_wait: Duration) -> Self {
        let (tx, _rx) = watch::channel(GateState::Starting);
        Self { tx, max_wait }
    }

    pub fn set_ready(&self, core: Arc<T>) {
        self.tx.send_replace(GateState::Ready(core));
    }

    pub fn set_failed(&self, error: AppError) {
        self.tx.send_replace(GateState::Failed(error));
    }

    /// Non-blocking: the core if it is ready.
    pub fn try_get(&self) -> Option<Arc<T>> {
        match &*self.tx.borrow() {
            GateState::Ready(c) => Some(Arc::clone(c)),
            _ => None,
        }
    }

    pub fn is_starting(&self) -> bool {
        matches!(&*self.tx.borrow(), GateState::Starting)
    }

    pub async fn get(&self) -> Result<Arc<T>, AppError> {
        let mut rx = self.tx.subscribe();
        let wait = async {
            loop {
                let state = rx.borrow_and_update().clone();
                match state {
                    GateState::Ready(c) => return Ok(c),
                    GateState::Failed(_) => return Err(AppError::internal(copy::CORE_FAILED)),
                    GateState::Starting => {
                        if rx.changed().await.is_err() {
                            return Err(AppError::internal(copy::CORE_FAILED));
                        }
                    }
                }
            }
        };
        match tokio::time::timeout(self.max_wait, wait).await {
            Ok(r) => r,
            Err(_) => Err(AppError::internal(copy::CORE_STARTING)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callcore_contract::ErrorCode;

    #[tokio::test(start_paused = true)]
    async fn timeout_maps_to_internal() {
        let r: Result<(), _> = with_timeout(Duration::from_secs(30), async {
            tokio::time::sleep(Duration::from_secs(31)).await;
            Ok(())
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!(e.code, ErrorCode::Internal);
        assert_eq!(e.message, TIMEOUT_MESSAGE);
    }

    #[tokio::test(start_paused = true)]
    async fn guarded_passes_values_and_errors_through() {
        let ok: CmdResult<u8> = guarded("t", async { Ok(7) }).await;
        assert_eq!(ok, CmdResult::Ok(7));
        let err: CmdResult<u8> =
            guarded("t", async { Err(AppError::new(ErrorCode::NoSpeech, "x")) }).await;
        assert_eq!(err, CmdResult::Err(AppError::new(ErrorCode::NoSpeech, "x")));
    }

    #[tokio::test(start_paused = true)]
    async fn guarded_command_exceeding_30s_resolves_internal() {
        let r: CmdResult<()> = guarded("slow", std::future::pending()).await;
        match r {
            CmdResult::Err(e) => assert_eq!(e.code, ErrorCode::Internal),
            CmdResult::Ok(_) => panic!("should time out"),
        }
    }

    #[tokio::test]
    async fn panics_become_internal_without_the_payload() {
        let r: CmdResult<()> = guarded("boom", async {
            if true {
                panic!("resume text sk-secret");
            }
            Ok(())
        })
        .await;
        match r {
            CmdResult::Err(e) => {
                assert_eq!(e.code, ErrorCode::Internal);
                assert!(!e.message.contains("sk-secret") && !e.message.contains("resume"));
            }
            CmdResult::Ok(_) => panic!(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn gate_waits_while_starting_then_returns_core() {
        let gate = Arc::new(CoreGate::<u32>::new());
        let g2 = Arc::clone(&gate);
        let waiter = tokio::spawn(async move { g2.get().await });
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!waiter.is_finished());
        gate.set_ready(Arc::new(42));
        assert_eq!(*waiter.await.unwrap().unwrap(), 42);
        assert_eq!(gate.try_get().map(|c| *c), Some(42));
    }

    #[tokio::test(start_paused = true)]
    async fn gate_times_out_with_core_starting_copy() {
        let gate = CoreGate::<u32>::new();
        let e = gate.get().await.unwrap_err();
        assert_eq!(e.message, copy::CORE_STARTING);
    }

    #[tokio::test(start_paused = true)]
    async fn gate_fails_immediately_after_failure() {
        let gate = CoreGate::<u32>::new();
        gate.set_failed(AppError::internal("disk on fire"));
        let t0 = tokio::time::Instant::now();
        let e = gate.get().await.unwrap_err();
        assert_eq!(e.message, copy::CORE_FAILED);
        assert_eq!(t0.elapsed(), Duration::ZERO);
        assert!(!gate.is_starting());
    }

    #[tokio::test(start_paused = true)]
    async fn gate_waiter_released_by_failure() {
        let gate = Arc::new(CoreGate::<u32>::new());
        let g2 = Arc::clone(&gate);
        let waiter = tokio::spawn(async move { g2.get().await });
        tokio::time::sleep(Duration::from_secs(1)).await;
        gate.set_failed(AppError::internal("x"));
        assert_eq!(
            waiter.await.unwrap().unwrap_err().message,
            copy::CORE_FAILED
        );
    }
}
