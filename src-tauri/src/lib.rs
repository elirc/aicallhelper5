//! Tauri 2 shell of AI Call Assistant v4 — thin glue. Behavior lives in
//! `callcore_shell` (pure, unit-tested) and `app.rs` (Tauri-free wiring +
//! command implementations, integration-tested in `tests/wiring.rs`).
//!
//! Startup order (spec §3, §15): logging → settings (needed for geometry and
//! always-on-top) → window built hidden at the restored geometry → content
//! protection applied → SHOWN → protection verified with retries → the heavy
//! core (audio worker, STT, providers, session actor) built on a task →
//! `core:ready` / `core:failed` → hotkey registered.

pub mod app;
pub mod commands;
pub mod logging;
pub mod platform;
pub mod protection;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use callcore_contract::config::{SessionTimeouts, SHUTDOWN_BUDGET};
use callcore_contract::ports::{ProviderRegistry, SystemClock};
use callcore_contract::{AppError, CoreEvent, LayoutMode};
use callcore_settings::{DpapiKeystore, SettingsStore};
use callcore_shell::close_guard::CloseDecision;
use callcore_shell::commands::catch_panic;
use callcore_shell::geometry::{self, layout_sizes};
use tauri::{
    Manager, PhysicalPosition, PhysicalSize, RunEvent, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_global_shortcut::ShortcutState;

use crate::app::{AppCtx, CoreDeps, STARTUP_PROTECTION_DELAYS_MS};
use crate::platform::{TauriPlatform, MAIN_WINDOW};

/// Navigation policy: the app never navigates away from its own origin.
pub fn allow_navigation(url: &tauri::Url) -> bool {
    let ok = callcore_shell::url::is_app_origin(url.scheme(), url.host_str(), url.port());
    if !ok {
        tracing::warn!(
            scheme = url.scheme(),
            "blocked navigation away from the app"
        );
    }
    ok
}

/// Every command the page can call (spec §9 + `get_diagnostics` +
/// `subscribe_events`). Shared by `run()` and the invoke-level test.
pub fn invoke_handler<R: tauri::Runtime>(
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        commands::get_settings,
        commands::set_settings,
        commands::get_status,
        commands::start_session,
        commands::stop_session,
        commands::ask,
        commands::cancel_session,
        commands::set_close_guard,
        commands::dock_window,
        commands::open_external,
        commands::get_diagnostics,
        commands::subscribe_events,
    ]
}

pub fn run() {
    let t0 = Instant::now();
    let data_dir = callcore_settings::default_dir();
    let log = logging::LogSink::new(Some(data_dir.join("logs")));
    logging::init_tracing(log.clone());
    logging::install_panic_hook(log.clone());
    let build = callcore_shell::build_info!();
    tracing::info!(version = %build.version, rev = %build.git_revision, dirty = build.dirty, "starting");

    let rt = tauri::async_runtime::block_on(async { tokio::runtime::Handle::current() });
    let tail = Arc::clone(&log);
    let ctx = AppCtx::new(
        build,
        platform::os_version(),
        rt,
        Box::new(move || tail.tail()),
    );

    // Settings + registry infos before the window: geometry and always-on-top
    // come from the file. Both are cheap (a JSON read; a client builder).
    let registry = Arc::new(callcore_llm::Registry::new(callcore_llm::http_client()));
    let infos = registry.infos();
    let dir = data_dir.clone();
    let store = std::panic::catch_unwind(AssertUnwindSafe(move || {
        Arc::new(SettingsStore::open(&dir, Arc::new(DpapiKeystore), infos))
    }))
    .ok();
    if let Some(s) = &store {
        ctx.set_settings_store(Arc::clone(s));
    }
    tracing::info!(ms = t0.elapsed().as_millis() as u64, "settings loaded");

    let setup_ctx = Arc::clone(&ctx);
    let app = tauri::Builder::default()
        // Must be first: decides whether this process lives at all.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            platform::focus_main(app);
            if let Some(ctx) = app.try_state::<Arc<AppCtx>>() {
                ctx.reapply_protection();
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Press only (RegisterHotKey uses MOD_NOREPEAT, so holding
                    // the keys does not auto-repeat); the page decides
                    // start vs stop.
                    if event.state() == ShortcutState::Pressed {
                        if let Some(ctx) = app.try_state::<Arc<AppCtx>>() {
                            ctx.emit(CoreEvent::HotkeyToggle {});
                        }
                    }
                })
                .build(),
        )
        .manage(Arc::clone(&ctx))
        .invoke_handler(invoke_handler())
        .setup(move |app| {
            setup(app, setup_ctx, store, registry, t0)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            let Some(ctx) = window.try_state::<Arc<AppCtx>>() else {
                return;
            };
            let ctx = Arc::clone(&ctx);
            match event {
                WindowEvent::Moved(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. } => {
                    ctx.schedule_bounds_save();
                }
                WindowEvent::Focused(true) => ctx.reapply_protection(),
                WindowEvent::CloseRequested { api, .. } => match ctx.on_close_request() {
                    CloseDecision::CancelAndNotify => api.prevent_close(),
                    CloseDecision::Allow => ctx.save_current_bounds_now(),
                },
                _ => {}
            }
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "failed to build the app");
            return;
        }
    };
    app.run(move |_handle, event| {
        if let RunEvent::Exit = event {
            shutdown_bounded(&ctx);
        }
    });
}

fn setup(
    app: &mut tauri::App,
    ctx: Arc<AppCtx>,
    store: Option<Arc<SettingsStore>>,
    registry: Arc<callcore_llm::Registry>,
    t0: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(ms = t0.elapsed().as_millis() as u64, "setup entered");
    let handle = app.handle().clone();
    let (layout, always_on_top, saved) = match &store {
        Some(s) => {
            let v = s.view();
            (v.layout_mode, v.always_on_top, s.bounds(v.layout_mode))
        }
        None => (LayoutMode::Full, true, None),
    };
    let (min, default) = layout_sizes(layout);
    let mons = platform::monitors(&handle);
    let bounds = geometry::restore(saved, &mons, None, default, min);

    let win = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .title("AI Call Assistant")
        .decorations(true)
        .resizable(true)
        .always_on_top(always_on_top)
        .min_inner_size(min.width as f64, min.height as f64)
        .inner_size(default.width as f64, default.height as f64)
        .visible(false)
        .on_navigation(allow_navigation)
        .build()?;
    tracing::info!(ms = t0.elapsed().as_millis() as u64, "window built");
    let _ = win.set_size(PhysicalSize::new(bounds.width, bounds.height));
    let _ = win.set_position(PhysicalPosition::new(bounds.x, bounds.y));
    // Apply before the first visible frame; verified (with retries) below.
    let first = platform::protect_on_ui_thread(&win);
    let _ = win.show();
    let _ = win.set_focus();
    tracing::info!(ms = t0.elapsed().as_millis() as u64, protection = ?first, "window shown");

    ctx.set_platform(Arc::new(TauriPlatform {
        app: handle.clone(),
        window: win.clone(),
    }));

    let pctx = Arc::clone(&ctx);
    ctx.runtime().spawn(async move {
        pctx.verify_protection(&STARTUP_PROTECTION_DELAYS_MS, true)
            .await;
    });

    // Heavy core on a task.
    let cctx = Arc::clone(&ctx);
    ctx.runtime().spawn(async move {
        let started = Instant::now();
        // A panic while building becomes `core:failed`, never a dead app.
        let built = catch_panic(build_production_core(&cctx, store, registry)).await;
        match built {
            Ok(core) => {
                let hotkey = core.settings.view().hotkey;
                cctx.mark_core_ready(core);
                tracing::info!(ms = started.elapsed().as_millis() as u64, "core built");
                cctx.apply_hotkey(&hotkey).await;
                cctx.emit(CoreEvent::SettingsChanged {});
            }
            Err(e) => cctx.mark_core_failed(e),
        }
    });
    Ok(())
}

async fn build_production_core(
    ctx: &Arc<AppCtx>,
    store: Option<Arc<SettingsStore>>,
    registry: Arc<callcore_llm::Registry>,
) -> Result<Arc<app::Core>, AppError> {
    let store =
        store.ok_or_else(|| AppError::internal("The settings file could not be loaded."))?;
    let audio = tokio::task::spawn_blocking(callcore_audio::LoopbackSource::new)
        .await
        .map_err(|_| AppError::internal("Could not start the audio worker."))?
        .map_err(|e| AppError::internal(e.to_string()))?;
    let audio = Arc::new(audio);
    let for_shutdown = Arc::clone(&audio);
    let providers: Arc<dyn ProviderRegistry> = registry;
    Ok(app::build_core(
        ctx,
        CoreDeps {
            settings: store,
            audio,
            stt: Arc::new(callcore_stt::DeepgramConnector::new()),
            providers,
            clock: Arc::new(SystemClock),
            timeouts: SessionTimeouts::default(),
            audio_shutdown: Some(Box::new(move || for_shutdown.shutdown())),
        },
    ))
}

/// Exit: cancel the live session, stop the actor and audio worker — bounded
/// by `SHUTDOWN_BUDGET` — and a watchdog forces the process out if anything
/// still hangs. The process must never hang (§13).
fn shutdown_bounded(ctx: &Arc<AppCtx>) {
    let started = Instant::now();
    std::thread::Builder::new()
        .name("exit-watchdog".into())
        .spawn(|| {
            std::thread::sleep(SHUTDOWN_BUDGET + Duration::from_secs(2));
            std::process::exit(0);
        })
        .ok();
    let c = Arc::clone(ctx);
    let done = tauri::async_runtime::block_on(async move {
        tokio::time::timeout(SHUTDOWN_BUDGET, c.shutdown())
            .await
            .is_ok()
    });
    tracing::info!(
        ms = started.elapsed().as_millis() as u64,
        clean = done,
        "shutdown"
    );
    if !done {
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conf() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid tauri.conf.json")
    }

    #[test]
    fn csp_is_tight() {
        let c = conf();
        let csp = c["app"]["security"]["csp"]
            .as_str()
            .expect("csp configured");
        for want in [
            "default-src 'self'",
            "script-src 'self'",
            "style-src 'self'",
            "object-src 'none'",
            "base-uri 'none'",
            "form-action 'none'",
        ] {
            assert!(csp.contains(want), "missing {want:?} in {csp}");
        }
        assert!(
            !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
            "{csp}"
        );
        // No network origin besides the IPC transport.
        let connect = csp
            .split(';')
            .find(|d| d.trim().starts_with("connect-src"))
            .expect("connect-src");
        for src in connect.split_whitespace().skip(1) {
            assert!(
                src == "ipc:" || src == "http://ipc.localhost",
                "unexpected connect-src {src}"
            );
        }
    }

    #[test]
    fn identity_and_version_are_stable() {
        let c = conf();
        assert_eq!(c["identifier"], "com.aicallassistant.app");
        assert_eq!(c["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(c["bundle"]["windows"]["nsis"]["installMode"], "currentUser");
    }

    #[test]
    fn capability_grants_no_plugin_or_window_apis() {
        let cap: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(cap["permissions"], serde_json::json!([]));
        assert_eq!(cap["windows"], serde_json::json!(["main"]));
    }

    #[test]
    fn navigation_stays_on_the_app_origin() {
        let ok = |u: &str| allow_navigation(&u.parse().unwrap());
        assert!(ok("http://tauri.localhost/index.html"));
        assert!(ok("tauri://localhost/"));
        assert!(ok("http://localhost:5173/"));
        assert!(!ok("https://evil.example/"));
        assert!(!ok("http://localhost:8080/"));
        assert!(!ok("file:///C:/Windows/win.ini"));
    }

    #[test]
    fn manifest_declares_per_monitor_v2_and_common_controls() {
        let m = include_str!("../windows-app-manifest.xml");
        assert!(m.contains("PerMonitorV2"));
        assert!(m.contains("Microsoft.Windows.Common-Controls"));
    }
}
