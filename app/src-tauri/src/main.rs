#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod error;
mod host;
mod state;
mod viewer;

use stadesk_core::AccessGate;
use tauri::{Manager, WindowEvent};

use crate::config::Store;
use crate::state::AppState;

fn startup_target() -> Option<String> {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--connect" {
            return arguments.next();
        }
        if let Some(value) = argument.strip_prefix("--connect=") {
            return Some(value.to_owned());
        }
    }
    None
}

fn main() {
    let target = startup_target();
    tauri::Builder::default()
        .setup(move |app| {
            let store = Store::new(app.path().app_data_dir()?)?;
            let identity = store.load_or_create_identity()?;
            let config = store.load_config();
            let mut gate = AccessGate::new(config.methods.into())?;
            config.apply_to(&mut gate);
            app.manage(AppState::new(store, identity, config, gate, target.clone()));
            tauri::async_runtime::spawn(host::restart(app.handle().clone()));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event
                && let Some(session_id) = viewer::session_from_label(window.label())
            {
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    viewer::disconnect_session(&app, session_id).await;
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_overview,
            commands::rotate_one_time_password,
            commands::save_settings,
            commands::set_permanent_password,
            commands::forget_device,
            commands::respond_request,
            commands::take_startup_target,
            viewer::connect_to,
            viewer::attach_stream,
            viewer::send_input,
            viewer::request_keyframe,
            viewer::disconnect,
        ])
        .run(tauri::generate_context!())
        .expect("StaDesk could not start");
}
