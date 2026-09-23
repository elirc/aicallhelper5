//! The real `Platform`: window geometry, always-on-top, hotkey registration,
//! opening links and content protection, over Tauri + Win32. Logic lives in
//! `callcore_shell`; this file only measures and applies.

use std::sync::mpsc;
use std::time::Duration;

use callcore_contract::{Bounds, LayoutMode, Protection};
use callcore_shell::geometry::{self, layout_sizes, Monitor, Rect};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::app::Platform;
use crate::protection::{apply_and_verify, HwndAffinity};

pub const MAIN_WINDOW: &str = "main";

pub fn to_monitor(m: &tauri::Monitor) -> Monitor {
    let wa = m.work_area();
    Monitor {
        work_area: Rect::new(wa.position.x, wa.position.y, wa.size.width, wa.size.height),
        scale: m.scale_factor(),
    }
}

/// All monitors, PRIMARY FIRST (geometry::restore docks on the first one
/// when there is no current monitor).
pub fn monitors<R: Runtime>(app: &AppHandle<R>) -> Vec<Monitor> {
    let primary = app.primary_monitor().ok().flatten().map(|m| to_monitor(&m));
    let mut all: Vec<Monitor> = app
        .available_monitors()
        .map(|v| v.iter().map(to_monitor).collect())
        .unwrap_or_default();
    if let Some(p) = primary {
        if let Some(i) = all.iter().position(|m| *m == p) {
            all.remove(i);
        }
        all.insert(0, p);
    }
    all
}

/// Apply content protection on the CURRENT thread (must be the UI thread).
pub fn protect_on_ui_thread<R: Runtime>(window: &WebviewWindow<R>) -> Protection {
    match window.hwnd() {
        Ok(h) => apply_and_verify(&HwndAffinity::new(h.0 as isize)),
        Err(e) => {
            tracing::warn!(error = %e, "no native window handle for content protection");
            Protection::Unprotected
        }
    }
}

pub struct TauriPlatform<R: Runtime> {
    pub app: AppHandle<R>,
    pub window: WebviewWindow<R>,
}

fn e2s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl<R: Runtime> Platform for TauriPlatform<R> {
    fn set_always_on_top(&self, on: bool) -> Result<(), String> {
        self.window.set_always_on_top(on).map_err(e2s)
    }

    fn apply_layout(&self, layout: LayoutMode, saved: Option<Bounds>) -> Result<Bounds, String> {
        let (min, default) = layout_sizes(layout);
        let mons = monitors(&self.app);
        let current = self
            .window
            .current_monitor()
            .ok()
            .flatten()
            .map(|m| to_monitor(&m));
        let b = geometry::restore(saved, &mons, current.as_ref(), default, min);
        if self.window.is_maximized().unwrap_or(false) {
            let _ = self.window.unmaximize();
        }
        // Min size first: shrinking to the prompter strip needs the lower min.
        self.window
            .set_min_size(Some(tauri::LogicalSize::new(
                min.width as f64,
                min.height as f64,
            )))
            .map_err(e2s)?;
        self.window
            .set_size(PhysicalSize::new(b.width, b.height))
            .map_err(e2s)?;
        self.window
            .set_position(PhysicalPosition::new(b.x, b.y))
            .map_err(e2s)?;
        Ok(b)
    }

    fn dock(&self) -> Result<Bounds, String> {
        let m = self
            .window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| self.window.primary_monitor().ok().flatten())
            .ok_or_else(|| "no monitor".to_string())?;
        let gm = to_monitor(&m);
        let inner = self.window.inner_size().map_err(e2s)?;
        let outer = self.window.outer_size().map_err(e2s)?;
        // Centre the OUTER frame (the invisible resize borders are symmetric).
        let frame_w = outer.width.saturating_sub(inner.width);
        let b = geometry::dock_top_centre(&gm, inner.width + frame_w, inner.height);
        self.window
            .set_position(PhysicalPosition::new(b.x, b.y))
            .map_err(e2s)?;
        Ok(Bounds {
            x: b.x,
            y: b.y,
            width: inner.width,
            height: inner.height,
        })
    }

    fn current_bounds(&self) -> Option<Bounds> {
        // A minimized window measures (-32000,-32000); a maximized one is not
        // the user's chosen normal size. Neither is persisted.
        if self.window.is_minimized().unwrap_or(true) || self.window.is_maximized().unwrap_or(true)
        {
            return None;
        }
        let pos = self.window.outer_position().ok()?;
        // INNER size: restore applies it with set_size (inner), so saving the
        // outer size would grow the window by the frame on every launch.
        let size = self.window.inner_size().ok()?;
        if size.width == 0 || size.height == 0 {
            return None;
        }
        Some(Bounds {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
        })
    }

    fn register_hotkey(&self, accelerator: &str) -> Result<(), String> {
        self.app
            .global_shortcut()
            .register(accelerator)
            .map_err(e2s)
    }

    fn unregister_hotkey(&self, accelerator: &str) {
        if let Err(e) = self.app.global_shortcut().unregister(accelerator) {
            tracing::debug!(error = %e, "hotkey unregister failed");
        }
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        shell_open(url)
    }

    fn protect_once(&self) -> Protection {
        let (tx, rx) = mpsc::channel();
        let win = self.window.clone();
        let sent = self.app.run_on_main_thread(move || {
            let _ = tx.send(protect_on_ui_thread(&win));
        });
        if sent.is_err() {
            return Protection::Unprotected;
        }
        rx.recv_timeout(Duration::from_secs(2))
            .unwrap_or(Protection::Unprotected)
    }
}

/// Hand an ALREADY VALIDATED https URL to the default browser.
#[cfg(windows)]
pub fn shell_open(url: &str) -> Result<(), String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; all other
    // pointers are null or static literals.
    let r = unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value > 32 on success.
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed ({})", r.0 as isize))
    }
}

#[cfg(not(windows))]
pub fn shell_open(_url: &str) -> Result<(), String> {
    Err("opening links is only implemented on Windows".into())
}

/// Focus the existing window (second launch, single-instance plugin).
pub fn focus_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        // Unminimize BEFORE focusing — focusing a minimized window is a no-op.
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Windows version string for diagnostics (RtlGetVersion does not lie
/// about the version the way GetVersionEx does).
#[cfg(windows)]
pub fn os_version() -> String {
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        csd: [0; 128],
    };
    // SAFETY: `info` is a correctly sized, initialized OSVERSIONINFOW.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status == 0 {
        format!("Windows {}.{}.{}", info.major, info.minor, info.build)
    } else {
        "Windows (version unknown)".into()
    }
}

#[cfg(not(windows))]
pub fn os_version() -> String {
    std::env::consts::OS.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn os_version_reads_a_real_version() {
        let v = os_version();
        assert!(
            v.starts_with("Windows 10.") || v.starts_with("Windows 6."),
            "{v}"
        );
    }

    #[test]
    fn every_parsed_hotkey_is_accepted_by_the_plugin_parser() {
        use callcore_shell::hotkey::{all_key_names, parse, HotkeyParse};
        use std::str::FromStr;
        use tauri_plugin_global_shortcut::Shortcut;
        for key in all_key_names() {
            for mods in ["Ctrl", "Alt+Shift", "Win", "Ctrl+Alt+Shift+Win"] {
                let raw = format!("{mods}+{key}");
                let HotkeyParse::Valid(h) = parse(&raw) else {
                    panic!("{raw} invalid")
                };
                let accel = h.accelerator();
                assert!(Shortcut::from_str(&accel).is_ok(), "plugin rejects {accel}");
            }
        }
    }
}
