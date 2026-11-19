use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::UiError;

const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
pub struct UpdateState {
    available: Mutex<Option<Update>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    version: String,
}

fn lock(state: &UpdateState) -> std::sync::MutexGuard<'_, Option<Update>> {
    state
        .available
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn supported() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_some()
}

async fn check(app: &AppHandle) {
    let Ok(updater) = app.updater() else {
        return;
    };
    let Ok(Some(update)) = updater.check().await else {
        return;
    };
    let view = UpdateView {
        version: update.version.clone(),
    };
    *lock(&app.state::<UpdateState>()) = Some(update);
    let _ = app.emit("update-available", view);
}

pub async fn watch(app: AppHandle) {
    if !supported() {
        return;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        check(&app).await;
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[tauri::command]
pub fn get_update(state: State<'_, UpdateState>) -> Option<UpdateView> {
    lock(&state).as_ref().map(|update| UpdateView {
        version: update.version.clone(),
    })
}

#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, UpdateState>) -> Result<(), UiError> {
    let Some(update) = lock(&state).clone() else {
        return Err(UiError::new("noUpdate"));
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| UiError::with_detail("updateFailed", error.to_string()))?;
    app.restart()
}
