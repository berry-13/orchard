//! Installed apps and IPA install.

use std::path::PathBuf;
use std::sync::Arc;

use crate::events::Events;
use base64::Engine;

use crate::dto::{AppKind, InstalledApp, IpaInfo};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::state::{DeviceState, blocking};

/// PNG bytes as a `data:` URL.
pub fn png_data_url(png: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// Maps an installation_proxy entry; internal and hidden apps are dropped.
pub fn to_installed(app: orchard_device::AppInfo) -> Option<InstalledApp> {
    let kind = match app.kind {
        orchard_device::AppKind::User => AppKind::User,
        orchard_device::AppKind::System => AppKind::System,
        orchard_device::AppKind::Other => return None,
    };
    let size = match (app.static_size, app.dynamic_size) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    Some(InstalledApp {
        bundle_id: app.bundle_id,
        name: app.name,
        version: app.version.unwrap_or_default(),
        short_version: app.short_version.unwrap_or_default(),
        size,
        kind,
        file_sharing: app.file_sharing,
    })
}

pub fn to_ipa_info(path: String, info: orchard_ipa::IpaInfo) -> IpaInfo {
    IpaInfo {
        path,
        name: info.display_name,
        bundle_id: info.bundle_id,
        version: info.build,
        short_version: info.version,
        minimum_os_version: info.minimum_os,
        encrypted: info.encrypted.unwrap_or(false),
        size: info.file_size,
        icon: info.icon_png.as_deref().map(png_data_url),
    }
}

pub async fn list_apps(state: &Arc<DeviceState>, udid: String) -> CmdResult<Vec<InstalledApp>> {
    let apps = state.connection(&udid).await?.list_apps(true).await?;
    Ok(apps.into_iter().filter_map(to_installed).collect())
}

/// Returns a PNG data URL, or `None` when the app has no icon.
pub async fn app_icon(
    state: &Arc<DeviceState>,
    udid: String,
    bundle_id: String,
) -> CmdResult<Option<String>> {
    let conn = state.connection(&udid).await?;
    // A missing icon is not worth an error in the UI.
    Ok(conn
        .app_icon(&bundle_id)
        .await
        .ok()
        .map(|png| png_data_url(&png)))
}

/// Opens the App Store on the device at the app's page, for its owner to get
/// it with their own Apple ID.
pub async fn open_store_page(state: &Arc<DeviceState>, udid: String, app_id: u64) -> CmdResult<()> {
    state
        .connection(&udid)
        .await?
        .open_store_page(app_id)
        .await?;
    Ok(())
}

pub async fn uninstall_app(
    state: &Arc<DeviceState>,
    udid: String,
    bundle_id: String,
) -> CmdResult<()> {
    state
        .connection(&udid)
        .await?
        .uninstall_app(&bundle_id)
        .await?;
    Ok(())
}

pub async fn inspect_ipa(path: String) -> CmdResult<IpaInfo> {
    blocking(move || {
        let info = orchard_ipa::inspect(&path)?;
        Ok(to_ipa_info(path, info))
    })
    .await
}

/// Task stages: `upload` (bytes), `install` (percent).
pub async fn install_ipa(
    app: Events,
    tasks: &TaskRegistry,
    state: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    path: String,
) -> CmdResult<()> {
    let conn = state.connection(&udid).await?;
    run_task(&app, tasks, task_id, |rep| async move {
        conn.install_ipa(&PathBuf::from(path), rep.callback())
            .await?;
        Ok(())
    })
    .await
}
