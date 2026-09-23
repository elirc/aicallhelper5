//! Pure, Tauri-free logic of the desktop shell.
//!
//! Everything here is unit-testable without a webview: the ordered event pump
//! (`events`), the status revision hub (`status`), window geometry
//! (`geometry`), hotkey parsing + the late-registration state machine
//! (`hotkey`), external-URL validation (`url`), the close guard
//! (`close_guard`), build info (`build_info`), diagnostics rendering +
//! redaction (`diagnostics`) and the command-boundary helpers (`commands`:
//! timeout, panic catching, the core-readiness gate).
//!
//! `src-tauri` is thin glue over these modules.

pub mod build_info;
pub mod close_guard;
pub mod commands;
pub mod diagnostics;
pub mod events;
pub mod geometry;
pub mod hotkey;
pub mod status;
pub mod url;
