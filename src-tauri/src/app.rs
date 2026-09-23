//! Tauri-free application context: the core wiring (`build_core`), the shared
//! state every command uses (`AppCtx`) and the command implementations. The
//! `#[tauri::command]` handlers in `commands.rs` are one-line calls into the
//! functions here, so the integration test (§14.1) drives the SAME code path
//! with a fake `Platform` and a fake event `Delivery`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use callcore_contract::config::{SessionTimeouts, STATUS_TIMEOUT};
use callcore_contract::ports::{
    AudioSource, Clock, EventSink, ProviderRegistry, SettingsReader, SttConnector,
};
use callcore_contract::{
    AppError, Bounds, BuildInfo, CmdResult, CoreEvent, LayoutMode, Protection, SessionId,
    SettingsPatch, SettingsView, StatusSnapshot,
};
use callcore_session::{SessionDeps, SessionHandle};
use callcore_settings::SettingsStore;
use callcore_shell::close_guard::{CloseDecision, CloseGuard};
use callcore_shell::commands::{catch_panic, guarded, CoreGate};
use callcore_shell::diagnostics::{self, DiagnosticsInput};
use callcore_shell::events::{Delivery, EventPump};
use callcore_shell::hotkey::{Completion, HotkeyReport, Registrar};
use callcore_shell::status::StatusHub;
use callcore_shell::url::validate_external;

/// Startup protection retries (§11): until Windows confirms.
pub const STARTUP_PROTECTION_DELAYS_MS: [u64; 5] = [0, 100, 300, 1000, 3000];
/// Re-apply on show / reload / layout change.
pub const REAPPLY_PROTECTION_DELAYS_MS: [u64; 3] = [0, 100, 300];
/// Geometry autosave debounce.
pub const BOUNDS_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long a save waits for the OS to register a hotkey before reporting
/// and moving on (a late completion is then handled by the registrar).
pub const HOTKEY_REGISTER_WAIT: Duration = Duration::from_secs(2);

/// Window / OS side effects. Production: `platform::TauriPlatform`. All
/// methods may block briefly (they hop to the UI thread) — call them from a
/// blocking thread, never from the UI thread.
pub trait Platform: Send + Sync {
    fn set_always_on_top(&self, on: bool) -> Result<(), String>;
    /// Switch the window to `layout`: min size + restored geometry.
    fn apply_layout(&self, layout: LayoutMode, saved: Option<Bounds>) -> Result<Bounds, String>;
    /// Dock top-centre of the window's CURRENT monitor at its current size.
    fn dock(&self) -> Result<Bounds, String>;
    /// Current normal-state bounds; None when minimized/maximized/unknown.
    fn current_bounds(&self) -> Option<Bounds>;
    fn register_hotkey(&self, accelerator: &str) -> Result<(), String>;
    fn unregister_hotkey(&self, accelerator: &str);
    fn open_url(&self, url: &str) -> Result<(), String>;
    /// Apply WDA_EXCLUDEFROMCAPTURE and read it back. Protected only if the
    /// read-back is 0x11.
    fn protect_once(&self) -> Protection;
}

// ───────────────────────────── core ─────────────────────────────

/// Everything `build_core` needs. Production passes the real crates; the
/// integration test passes fakes behind the same ports.
pub struct CoreDeps {
    pub settings: Arc<SettingsStore>,
    pub audio: Arc<dyn AudioSource>,
    pub stt: Arc<dyn SttConnector>,
    pub providers: Arc<dyn ProviderRegistry>,
    pub clock: Arc<dyn Clock>,
    pub timeouts: SessionTimeouts,
    /// Stops the audio worker thread at exit (bounded by the caller).
    pub audio_shutdown: Option<Box<dyn FnOnce() + Send>>,
}

pub struct Core {
    pub settings: Arc<SettingsStore>,
    pub session: SessionHandle,
    audio_shutdown: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    watcher: tokio::task::JoinHandle<()>,
}

impl Core {
    /// Cancel the live session, stop the actor, stop the audio worker.
    /// The caller bounds this with `SHUTDOWN_BUDGET`.
    pub async fn shutdown(&self) {
        self.session.shutdown().await;
        self.watcher.abort();
        let f = lock(&self.audio_shutdown).take();
        if let Some(f) = f {
            let _ = tokio::task::spawn_blocking(f).await;
        }
    }
}

/// THE wiring: session actor with the pump as its event sink, and a watcher
/// that mirrors session status into the status hub (revision bump per
/// change). Used by `run()` and by the integration test.
pub fn build_core(ctx: &Arc<AppCtx>, deps: CoreDeps) -> Arc<Core> {
    let sink: Arc<dyn EventSink> = ctx.pump.clone();
    let reader: Arc<dyn SettingsReader> = deps.settings.clone();
    let session = SessionHandle::spawn(SessionDeps {
        audio: deps.audio,
        stt: deps.stt,
        providers: deps.providers,
        settings: reader,
        sink,
        clock: deps.clock,
        timeouts: deps.timeouts,
    });
    let mut rx = session.status_watch();
    let hub = Arc::clone(&ctx.hub);
    let watcher = ctx.rt.spawn(async move {
        loop {
            let status = rx.borrow_and_update().clone();
            hub.set_session(status);
            if rx.changed().await.is_err() {
                return;
            }
        }
    });
    Arc::new(Core {
        settings: deps.settings,
        session,
        audio_shutdown: Mutex::new(deps.audio_shutdown),
        watcher,
    })
}

// ───────────────────────────── context ─────────────────────────────

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::internal(callcore_shell::commands::PANIC_MESSAGE))
}

type LogTail = Box<dyn Fn() -> Vec<String> + Send + Sync>;

pub struct AppCtx {
    pub pump: Arc<EventPump>,
    pub hub: Arc<StatusHub>,
    pub gate: CoreGate<Core>,
    pub build: BuildInfo,
    pub os: String,
    rt: tokio::runtime::Handle,
    settings: OnceLock<Arc<SettingsStore>>,
    platform: OnceLock<Arc<dyn Platform>>,
    close_guard: Mutex<CloseGuard>,
    hotkey: Mutex<Registrar>,
    /// Serializes set_settings including its side effects (§12).
    settings_lock: tokio::sync::Mutex<()>,
    protect_lock: tokio::sync::Mutex<()>,
    layout: Mutex<LayoutMode>,
    bounds_gen: AtomicU64,
    log_tail: LogTail,
}

impl AppCtx {
    pub fn new(
        build: BuildInfo,
        os: String,
        rt: tokio::runtime::Handle,
        log_tail: LogTail,
    ) -> Arc<Self> {
        Arc::new(Self {
            pump: Arc::new(EventPump::new()),
            hub: Arc::new(StatusHub::new()),
            gate: CoreGate::new(),
            build,
            os,
            rt,
            settings: OnceLock::new(),
            platform: OnceLock::new(),
            close_guard: Mutex::new(CloseGuard::new()),
            hotkey: Mutex::new(Registrar::new()),
            settings_lock: tokio::sync::Mutex::new(()),
            protect_lock: tokio::sync::Mutex::new(()),
            layout: Mutex::new(LayoutMode::Full),
            bounds_gen: AtomicU64::new(0),
            log_tail,
        })
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.rt
    }

    /// Settings are loaded before the window (geometry, always-on-top).
    pub fn set_settings_store(&self, store: Arc<SettingsStore>) {
        *lock(&self.layout) = store.view().layout_mode;
        let _ = self.settings.set(store);
    }

    pub fn settings_store(&self) -> Option<Arc<SettingsStore>> {
        self.settings.get().cloned()
    }

    pub fn set_platform(&self, p: Arc<dyn Platform>) {
        let _ = self.platform.set(p);
    }

    fn platform(&self) -> Option<Arc<dyn Platform>> {
        self.platform.get().cloned()
    }

    pub fn layout(&self) -> LayoutMode {
        *lock(&self.layout)
    }

    pub fn emit(&self, ev: CoreEvent) {
        self.pump.emit(ev);
    }

    /// Any page traffic counts as a sign of life for the close guard.
    fn touch(&self) {
        lock(&self.close_guard).ping(Instant::now());
    }

    pub fn hotkey_report(&self) -> HotkeyReport {
        lock(&self.hotkey).report().clone()
    }

    pub fn mark_core_ready(&self, core: Arc<Core>) {
        self.gate.set_ready(core);
        let revision = self.hub.set_core_ready();
        self.emit(CoreEvent::CoreReady { revision });
        tracing::info!(revision, "core ready");
    }

    pub fn mark_core_failed(&self, error: AppError) {
        tracing::error!(code = ?error.code, "core failed to start");
        self.gate.set_failed(error.clone());
        let revision = self.hub.set_core_failed(error.clone());
        self.emit(CoreEvent::CoreFailed { revision, error });
    }

    fn fill_view(&self, mut v: SettingsView) -> SettingsView {
        let r = self.hotkey_report();
        v.hotkey_registered = r.registered();
        v.hotkey_status = r.status;
        v.hotkey_message = r.message;
        v.build = self.build.clone();
        v
    }

    // ── protection ──

    fn publish_protection(&self, p: Protection) {
        let (revision, _) = self.hub.set_protection(p);
        let ev = if p == Protection::Protected {
            CoreEvent::ProtectionOk {
                revision,
                protection: p,
            }
        } else {
            CoreEvent::ProtectionFailed {
                revision,
                protection: p,
            }
        };
        self.emit(ev);
    }

    /// Apply + verify with retries; publish the final verdict. Intermediate
    /// failures are not published (the verdict stays as it was — `unknown`
    /// at startup — until Windows confirms or the retries run out).
    /// `always_emit`: publish even when the verdict did not change (startup,
    /// page (re)subscribe); re-applies on focus/show only publish changes.
    pub async fn verify_protection(
        self: &Arc<Self>,
        delays_ms: &[u64],
        always_emit: bool,
    ) -> Protection {
        let _g = self.protect_lock.lock().await;
        let Some(p) = self.platform() else {
            return self.hub.protection();
        };
        let mut verdict = Protection::Unprotected;
        for d in delays_ms {
            if *d > 0 {
                tokio::time::sleep(Duration::from_millis(*d)).await;
            }
            let p = Arc::clone(&p);
            verdict = blocking(move || p.protect_once())
                .await
                .unwrap_or(Protection::Unprotected);
            if verdict == Protection::Protected {
                break;
            }
        }
        if verdict != Protection::Protected {
            tracing::warn!("screen-capture exclusion NOT confirmed by Windows");
        }
        if always_emit || self.hub.protection() != verdict {
            self.publish_protection(verdict);
        }
        verdict
    }

    /// Fire-and-forget re-apply (window show, focus, layout change).
    pub fn reapply_protection(self: &Arc<Self>) {
        self.spawn_reverify(false);
    }

    fn spawn_reverify(self: &Arc<Self>, always_emit: bool) {
        let ctx = Arc::clone(self);
        self.rt.spawn(async move {
            ctx.verify_protection(&REAPPLY_PROTECTION_DELAYS_MS, always_emit)
                .await;
        });
    }

    // ── hotkey ──

    /// Apply the hotkey setting through the generation state machine. Waits
    /// at most `HOTKEY_REGISTER_WAIT` for the OS; a late completion is undone
    /// (if superseded) or published via `settings:changed`.
    pub async fn apply_hotkey(self: &Arc<Self>, hotkey: &str) -> HotkeyReport {
        let Some(platform) = self.platform() else {
            return self.hotkey_report();
        };
        let plan = lock(&self.hotkey).begin(hotkey);
        if let Some(old) = plan.unregister.clone() {
            let p = Arc::clone(&platform);
            let _ = tokio::time::timeout(
                HOTKEY_REGISTER_WAIT,
                blocking(move || p.unregister_hotkey(&old)),
            )
            .await;
        }
        if let Some(report) = plan.immediate {
            return report;
        }
        let Some(accel) = plan.register else {
            return self.hotkey_report();
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<Completion>();
        let ctx = Arc::clone(self);
        let generation = plan.generation;
        // Not awaited past the wait bound: the closure finishes the state
        // machine itself, so a late completion is still handled.
        tokio::task::spawn_blocking(move || {
            let ok = match platform.register_hotkey(&accel) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(error = %e, "hotkey registration failed");
                    false
                }
            };
            let completion = lock(&ctx.hotkey).complete(generation, &accel, ok);
            if let Completion::Undo(a) = &completion {
                tracing::info!("undoing a late hotkey registration");
                platform.unregister_hotkey(a);
            }
            let applied = matches!(completion, Completion::Applied(_) | Completion::Adopted(_));
            if tx.send(completion).is_err() && applied {
                // Nobody is waiting any more: the status changed on its own.
                ctx.emit(CoreEvent::SettingsChanged {});
            }
        });
        match tokio::time::timeout(HOTKEY_REGISTER_WAIT, rx).await {
            Ok(Ok(Completion::Applied(r))) => r,
            _ => self.hotkey_report(),
        }
    }

    // ── window ──

    /// Debounced geometry autosave for the current layout.
    pub fn schedule_bounds_save(self: &Arc<Self>) {
        let gen = self.bounds_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let ctx = Arc::clone(self);
        self.rt.spawn(async move {
            tokio::time::sleep(BOUNDS_DEBOUNCE).await;
            if ctx.bounds_gen.load(Ordering::SeqCst) == gen {
                ctx.save_current_bounds().await;
            }
        });
    }

    /// Synchronous variant for the close path (runs on the UI thread, where
    /// the window getters execute directly).
    pub fn save_current_bounds_now(&self) {
        self.bounds_gen.fetch_add(1, Ordering::SeqCst);
        let (Some(p), Some(store)) = (self.platform(), self.settings_store()) else {
            return;
        };
        if let Some(b) = p.current_bounds() {
            let _ = store.save_bounds(self.layout(), b);
        }
    }

    /// Save the window's current bounds for the current layout now.
    pub async fn save_current_bounds(&self) {
        let (Some(p), Some(store)) = (self.platform(), self.settings_store()) else {
            return;
        };
        let layout = self.layout();
        let _ = blocking(move || {
            if let Some(b) = p.current_bounds() {
                if let Err(e) = store.save_bounds(layout, b) {
                    tracing::warn!(code = ?e.code, "geometry not saved");
                }
            }
        })
        .await;
    }

    async fn switch_layout(self: &Arc<Self>, new: LayoutMode) {
        let old = self.layout();
        if old == new {
            return;
        }
        // Persist the outgoing layout's geometry before it changes.
        self.bounds_gen.fetch_add(1, Ordering::SeqCst);
        self.save_current_bounds().await;
        *lock(&self.layout) = new;
        if let (Some(p), Some(store)) = (self.platform(), self.settings_store()) {
            let r = blocking(move || p.apply_layout(new, store.bounds(new))).await;
            if let Ok(Err(e)) = r {
                tracing::warn!(error = %e, "layout change: window not moved");
            }
        }
        self.reapply_protection();
    }

    /// Close request from the OS (X button, Alt+F4).
    pub fn on_close_request(&self) -> CloseDecision {
        let decision =
            lock(&self.close_guard).on_close_request(Instant::now(), self.pump.is_attached());
        if decision == CloseDecision::CancelAndNotify {
            self.emit(CoreEvent::WindowCloseRequested {});
        }
        decision
    }

    /// Cancel the live session, stop the actor and the audio worker. The
    /// caller bounds this with `SHUTDOWN_BUDGET`.
    pub async fn shutdown(&self) {
        if let Some(core) = self.gate.try_get() {
            core.shutdown().await;
        }
    }
}

// ───────────────────────────── commands ─────────────────────────────
//
// Every command: page ping, panic-catch + 30 s timeout (`guarded`), result
// in the `{ok, value|error}` envelope. Commands that need the core go
// through the `CoreGate`.

pub async fn get_settings(ctx: &Arc<AppCtx>) -> CmdResult<SettingsView> {
    ctx.touch();
    guarded("get_settings", async {
        let core = ctx.gate.get().await?;
        let store = Arc::clone(&core.settings);
        let view = blocking(move || store.view()).await?;
        Ok(ctx.fill_view(view))
    })
    .await
}

pub async fn set_settings(ctx: &Arc<AppCtx>, patch: SettingsPatch) -> CmdResult<SettingsView> {
    ctx.touch();
    guarded("set_settings", async {
        let core = ctx.gate.get().await?;
        // Held across the side effects: the next save starts only after this
        // save's hotkey/always-on-top/layout effects finished.
        let _serial = ctx.settings_lock.lock().await;
        let store = Arc::clone(&core.settings);
        let before = blocking(move || store.view()).await?;
        let store = Arc::clone(&core.settings);
        let after = blocking(move || store.apply_patch(patch)).await??;

        let retry_hotkey = !ctx.hotkey_report().registered()
            && ctx.hotkey_report().status == callcore_contract::HotkeyStatus::Unavailable;
        if before.hotkey != after.hotkey || retry_hotkey {
            ctx.apply_hotkey(&after.hotkey).await;
        }
        if before.always_on_top != after.always_on_top {
            if let Some(p) = ctx.platform() {
                let on = after.always_on_top;
                let _ = blocking(move || p.set_always_on_top(on)).await;
            }
        }
        if before.layout_mode != after.layout_mode {
            ctx.switch_layout(after.layout_mode).await;
        }
        Ok(ctx.fill_view(after))
    })
    .await
}

/// Never blocks on a busy core: answers within `STATUS_TIMEOUT`, with phase
/// `unknown` if the session could not be read in time.
pub async fn get_status(ctx: &Arc<AppCtx>) -> CmdResult<StatusSnapshot> {
    ctx.touch();
    let read = async {
        if let Some(core) = ctx.gate.try_get() {
            ctx.hub.set_session(core.session.status());
        }
        Ok::<_, AppError>(ctx.hub.snapshot())
    };
    match tokio::time::timeout(STATUS_TIMEOUT, catch_panic(read)).await {
        Ok(r) => r.into(),
        Err(_) => CmdResult::Ok(ctx.hub.snapshot_unknown_session()),
    }
}

pub async fn start_session(ctx: &Arc<AppCtx>) -> CmdResult<SessionId> {
    ctx.touch();
    guarded("start_session", async {
        let core = ctx.gate.get().await?;
        core.session.start_session().await
    })
    .await
}

pub async fn stop_session(ctx: &Arc<AppCtx>, session_id: SessionId) -> CmdResult<()> {
    ctx.touch();
    guarded("stop_session", async {
        let core = ctx.gate.get().await?;
        core.session.stop_session(&session_id).await
    })
    .await
}

pub async fn ask(ctx: &Arc<AppCtx>, text: String) -> CmdResult<SessionId> {
    ctx.touch();
    guarded("ask", async {
        let core = ctx.gate.get().await?;
        core.session.ask(text).await
    })
    .await
}

/// Always `Ok(null)` (fire-and-forget, idempotent).
pub async fn cancel_session(ctx: &Arc<AppCtx>, session_id: SessionId) -> CmdResult<()> {
    ctx.touch();
    let _ = guarded("cancel_session", async {
        if let Some(core) = ctx.gate.try_get() {
            core.session.cancel_session(&session_id);
        }
        Ok(())
    })
    .await;
    CmdResult::Ok(())
}

pub async fn set_close_guard(ctx: &Arc<AppCtx>, active: bool) -> CmdResult<()> {
    lock(&ctx.close_guard).set_active(active, Instant::now());
    CmdResult::Ok(())
}

pub async fn dock_window(ctx: &Arc<AppCtx>) -> CmdResult<()> {
    ctx.touch();
    guarded("dock_window", async {
        let p = ctx
            .platform()
            .ok_or_else(|| AppError::internal("The window is not ready yet."))?;
        let bounds = blocking(move || p.dock())
            .await?
            .map_err(|_| AppError::internal("Could not move the window."))?;
        if let Some(store) = ctx.settings_store() {
            let layout = ctx.layout();
            ctx.bounds_gen.fetch_add(1, Ordering::SeqCst);
            let _ = blocking(move || store.save_bounds(layout, bounds)).await;
        }
        Ok(())
    })
    .await
}

pub async fn open_external(ctx: &Arc<AppCtx>, url: String) -> CmdResult<()> {
    ctx.touch();
    guarded("open_external", async {
        let url = validate_external(&url)?;
        let p = ctx
            .platform()
            .ok_or_else(|| AppError::internal("The window is not ready yet."))?;
        blocking(move || p.open_url(&url))
            .await?
            .map_err(|_| AppError::internal("Windows could not open the link."))
    })
    .await
}

pub async fn get_diagnostics(ctx: &Arc<AppCtx>) -> CmdResult<String> {
    ctx.touch();
    guarded("get_diagnostics", async {
        let snap = ctx.hub.snapshot();
        let (load, blocked) = match ctx.settings_store() {
            Some(s) => {
                let issue = s.load_issue();
                (
                    s.load_status(),
                    issue.map(|i| i.writes_blocked).unwrap_or(false),
                )
            }
            None => (callcore_contract::SettingsLoadStatus::Unreadable, false),
        };
        let hk = ctx.hotkey_report();
        let logs = (ctx.log_tail)();
        Ok(diagnostics::render(&DiagnosticsInput {
            build: &ctx.build,
            os: &ctx.os,
            protection: snap.protection,
            core: snap.core,
            core_error: snap.core_error.as_ref(),
            settings_load: load,
            settings_writes_blocked: blocked,
            hotkey_status: hk.status,
            hotkey_message: hk.message.as_deref(),
            events: Some(ctx.pump.stats()),
            log_lines: &logs,
        }))
    })
    .await
}

/// Attach (or replace, on page reload) the ordered event channel, then
/// re-apply content protection (§11: re-verify on every reload).
pub async fn subscribe_events(ctx: &Arc<AppCtx>, delivery: Arc<dyn Delivery>) -> CmdResult<()> {
    ctx.touch();
    let gen = ctx.pump.attach(delivery);
    tracing::info!(generation = gen, "event channel attached");
    // Always publish: the freshly loaded page must hear the verdict.
    ctx.spawn_reverify(true);
    CmdResult::Ok(())
}
