//! Orchard as a Tauri app. On desktops it runs [`orchard_service::Service`]
//! itself; on iOS and Android it is a client of an Orchard server, and the
//! page sends its commands there.

pub mod material;

use serde_json::{Value, json};
use tauri::{AppHandle, State};

use material::WindowMaterial;

/// The one command the frontend calls: runs `command` from the table in
/// `app/src/lib/api.ts`. Errors are `{kind, message}`.
#[tauri::command]
async fn dispatch(
    app: AppHandle,
    material: State<'_, WindowMaterial>,
    command: String,
    args: Value,
) -> Result<Value, Value> {
    if command == "app_info" {
        // Also tells the page whether it runs on a mobile client.
        return Ok(json!({
            "version": app.package_info().version.to_string(),
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "material": material.0,
        }));
    }
    backend::dispatch(&app, &command, args).await
}

#[cfg(desktop)]
mod backend {
    use std::sync::Arc;

    use orchard_service::error::CommandError;
    use orchard_service::{Events, Service};
    use serde_json::{Value, json};
    use tauri::{App, AppHandle, Emitter, Manager};
    use tauri_plugin_opener::OpenerExt;

    pub async fn dispatch(app: &AppHandle, command: &str, args: Value) -> Result<Value, Value> {
        let service = app.state::<Arc<Service>>();
        let result = match command {
            // Done here rather than from the page so the page needs no
            // permission to open arbitrary paths.
            "open_data_dir" => app
                .opener()
                .open_path(service.data_dir().to_string_lossy(), None::<&str>)
                .map(|()| Value::Null)
                .map_err(|e| CommandError::new("io", e.to_string())),
            _ => service.dispatch(command, args).await,
        };
        result.map_err(|e| json!(e))
    }

    pub fn setup(app: &mut App) -> Result<(), String> {
        let handle = app.handle().clone();
        let events = Events::new(move |event, payload| {
            // Emission only fails when the webview is gone, nothing useful to do then.
            let _ = handle.emit(event, payload);
        });
        let service =
            Arc::new(Service::new(orchard_core::data_dir(), events).map_err(|e| e.message)?);
        tauri::async_runtime::spawn(service.watch_devices());
        app.manage(service);
        Ok(())
    }
}

#[cfg(mobile)]
mod backend {
    use serde_json::{Value, json};
    use tauri::{App, AppHandle};

    pub async fn dispatch(_app: &AppHandle, _command: &str, _args: Value) -> Result<Value, Value> {
        Err(json!({
            "kind": "notImplemented",
            "message": "This app works through an Orchard server; connect to one first",
        }))
    }

    pub fn setup(_app: &mut App) -> Result<(), String> {
        Ok(())
    }
}

/// Builds and runs the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            backend::setup(app)?;
            let material = tauri::Manager::get_webview_window(app, "main")
                .and_then(|window| material::apply(&window));
            tauri::Manager::manage(app, WindowMaterial(material));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![dispatch])
        .run(tauri::generate_context!())
        .expect("failed to run Orchard");
}
