use serde::{Deserialize, Serialize};
use stadesk_core::{CoreError, PermanentPassword, PublicIdentity};
use stadesk_net::ServerFingerprint;
use tauri::{AppHandle, State};

use crate::config::{Methods, decode_hex, encode_hex};
use crate::error::UiError;
use crate::host;
use crate::state::{AppState, Decision, HostStatusView, lock, server_endpoint};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    id: String,
    key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    server_address: String,
    server_key: String,
    methods: Methods,
    has_permanent_password: bool,
    trusted_devices: Vec<TrustedDevice>,
    language: String,
    default_server_address: &'static str,
    default_server_key: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    id: String,
    one_time_password: String,
    host_status: HostStatusView,
    settings: SettingsView,
    platform: &'static str,
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

fn overview(state: &AppState) -> Overview {
    let config = state.config();
    let trusted_devices = config
        .trusted_devices
        .iter()
        .filter_map(|key| {
            let identity = PublicIdentity::decode(&decode_hex(key)?).ok()?;
            Some(TrustedDevice {
                id: identity.id().to_string(),
                key: key.clone(),
            })
        })
        .collect();
    Overview {
        id: state.identity.id().to_string(),
        one_time_password: lock(&state.gate).one_time_password().to_string(),
        host_status: lock(&state.host_status).clone(),
        settings: SettingsView {
            server_address: config.server_address.clone(),
            server_key: config.server_key.clone(),
            methods: config.methods,
            has_permanent_password: config.permanent_password.is_some(),
            trusted_devices,
            language: config.language.clone(),
            default_server_address: crate::config::DEFAULT_SERVER_ADDRESS,
            default_server_key: crate::config::DEFAULT_SERVER_KEY,
        },
        platform: platform(),
    }
}

#[tauri::command]
pub fn get_overview(state: State<'_, AppState>) -> Overview {
    overview(&state)
}

#[tauri::command]
pub fn rotate_one_time_password(state: State<'_, AppState>) -> Result<Overview, UiError> {
    lock(&state.gate)
        .rotate_one_time_password()
        .map_err(|error| UiError::with_detail("unknown", error.to_string()))?;
    Ok(overview(&state))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    server_address: String,
    server_key: String,
    methods: Methods,
    language: String,
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SettingsInput,
) -> Result<Overview, UiError> {
    let server_address = input.server_address.trim().to_owned();
    let server_key = input.server_key.trim().to_owned();
    if !server_address.is_empty() && server_endpoint(&server_address).is_none() {
        return Err(UiError::new("invalidServerAddress"));
    }
    if !server_key.is_empty() && server_key.parse::<ServerFingerprint>().is_err() {
        return Err(UiError::new("invalidServerKey"));
    }
    if !["auto", "en", "tr"].contains(&input.language.as_str()) {
        return Err(UiError::new("invalidLanguage"));
    }
    let previous = state.config();
    let updated = state.update_config(|config| {
        config.server_address = server_address;
        config.server_key = server_key;
        config.methods = input.methods;
        config.language = input.language;
    })?;
    if previous.server_address != updated.server_address
        || previous.server_key != updated.server_key
    {
        tauri::async_runtime::spawn(host::restart(app));
    }
    Ok(overview(&state))
}

#[tauri::command]
pub fn set_permanent_password(
    state: State<'_, AppState>,
    password: Option<String>,
) -> Result<Overview, UiError> {
    let encoded = match password {
        Some(password) => Some(encode_hex(
            &PermanentPassword::from_plaintext(&password)
                .map_err(|error| match error {
                    CoreError::PasswordTooShort => UiError::new("passwordTooShort"),
                    CoreError::PasswordTooLong => UiError::new("passwordTooLong"),
                    other => UiError::with_detail("unknown", other.to_string()),
                })?
                .encode(),
        )),
        None => None,
    };
    state.update_config(|config| config.permanent_password = encoded)?;
    Ok(overview(&state))
}

#[tauri::command]
pub fn forget_device(state: State<'_, AppState>, key: String) -> Result<Overview, UiError> {
    state.update_config(|config| config.trusted_devices.retain(|existing| *existing != key))?;
    Ok(overview(&state))
}

#[tauri::command]
pub fn take_startup_target(state: State<'_, AppState>) -> Option<String> {
    lock(&state.startup_target).take()
}

#[tauri::command]
pub fn respond_request(
    state: State<'_, AppState>,
    request_id: u64,
    accept: bool,
    trust: bool,
) -> Result<(), UiError> {
    let sender = lock(&state.pending)
        .remove(&request_id)
        .ok_or_else(|| UiError::new("unknownRequest"))?;
    let _ = sender.send(Decision { accept, trust });
    Ok(())
}
