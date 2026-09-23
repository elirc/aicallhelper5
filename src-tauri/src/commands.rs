//! `#[tauri::command]` handlers: one-line delegations into `app.rs`, which
//! holds the (Tauri-free, integration-tested) implementations. Every handler
//! resolves to the `{ok, value|error}` envelope — the outer `Result` is only
//! there because Tauri requires it for async commands that borrow `State`,
//! and it is always `Ok`.

use std::sync::Arc;

use callcore_contract::{
    CmdResult, EventEnvelope, SessionId, SettingsPatch, SettingsView, StatusSnapshot,
};
use callcore_shell::events::{Delivery, DeliveryError};
use tauri::ipc::Channel;
use tauri::State;

use crate::app::{self, AppCtx};

type Ctx<'a> = State<'a, Arc<AppCtx>>;
type Reply<T> = Result<CmdResult<T>, ()>;

/// Production `Delivery`: the page's Tauri Channel.
pub struct ChannelDelivery(pub Channel<EventEnvelope>);

impl Delivery for ChannelDelivery {
    fn deliver(&self, env: &EventEnvelope) -> Result<(), DeliveryError> {
        self.0
            .send(env.clone())
            .map_err(|e| DeliveryError(e.to_string()))
    }
}

#[tauri::command]
pub async fn get_settings(ctx: Ctx<'_>) -> Reply<SettingsView> {
    Ok(app::get_settings(&ctx).await)
}

#[tauri::command]
pub async fn set_settings(ctx: Ctx<'_>, patch: SettingsPatch) -> Reply<SettingsView> {
    Ok(app::set_settings(&ctx, patch).await)
}

#[tauri::command]
pub async fn get_status(ctx: Ctx<'_>) -> Reply<StatusSnapshot> {
    Ok(app::get_status(&ctx).await)
}

#[tauri::command]
pub async fn start_session(ctx: Ctx<'_>) -> Reply<SessionId> {
    Ok(app::start_session(&ctx).await)
}

#[tauri::command]
pub async fn stop_session(ctx: Ctx<'_>, session_id: SessionId) -> Reply<()> {
    Ok(app::stop_session(&ctx, session_id).await)
}

#[tauri::command]
pub async fn ask(ctx: Ctx<'_>, text: String) -> Reply<SessionId> {
    Ok(app::ask(&ctx, text).await)
}

#[tauri::command]
pub async fn cancel_session(ctx: Ctx<'_>, session_id: SessionId) -> Reply<()> {
    Ok(app::cancel_session(&ctx, session_id).await)
}

#[tauri::command]
pub async fn set_close_guard(ctx: Ctx<'_>, active: bool) -> Reply<()> {
    Ok(app::set_close_guard(&ctx, active).await)
}

#[tauri::command]
pub async fn dock_window(ctx: Ctx<'_>) -> Reply<()> {
    Ok(app::dock_window(&ctx).await)
}

#[tauri::command]
pub async fn open_external(ctx: Ctx<'_>, url: String) -> Reply<()> {
    Ok(app::open_external(&ctx, url).await)
}

#[tauri::command]
pub async fn get_diagnostics(ctx: Ctx<'_>) -> Reply<String> {
    Ok(app::get_diagnostics(&ctx).await)
}

#[tauri::command]
pub async fn subscribe_events(ctx: Ctx<'_>, channel: Channel<EventEnvelope>) -> Reply<()> {
    Ok(app::subscribe_events(&ctx, Arc::new(ChannelDelivery(channel))).await)
}
