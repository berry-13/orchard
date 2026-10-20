//! Camera roll listing and export.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::events::Events;
use orchard_device::FileLocation;

use crate::dto::{PhotoItem, PhotoKind};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::paths::safe_name;
use crate::state::DeviceState;

pub fn to_photo(p: orchard_library::PhotoItem) -> PhotoItem {
    PhotoItem {
        path: p.path,
        name: p.name,
        size: p.size,
        modified: p.modified.map(|s| s * 1000),
        kind: match p.kind {
            orchard_library::MediaKind::Photo => PhotoKind::Image,
            orchard_library::MediaKind::Video => PhotoKind::Video,
        },
    }
}

pub async fn list_photos(state: &Arc<DeviceState>, udid: String) -> CmdResult<Vec<PhotoItem>> {
    let mut files = state
        .connection(&udid)
        .await?
        .files(FileLocation::Media)
        .await?;
    let items = orchard_library::list_camera_roll(&mut files).await?;
    Ok(items.into_iter().map(to_photo).collect())
}

/// Exports to `<dest_dir>/<album>/<name>`, skipping files already exported
/// there. Task stage: `export` (bytes).
pub async fn export_photos(
    app: Events,
    tasks: &TaskRegistry,
    state: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    paths: Vec<String>,
    dest_dir: String,
) -> CmdResult<()> {
    let mut files = state
        .connection(&udid)
        .await?
        .files(FileLocation::Media)
        .await?;
    run_task(&app, tasks, task_id, |rep| async move {
        let wanted: HashSet<String> = paths.into_iter().collect();
        let items: Vec<_> = orchard_library::list_camera_roll(&mut files)
            .await?
            .into_iter()
            .filter(|i| wanted.contains(&i.path))
            .collect();
        // Items land at `<dest>/<album>/<name>`; both come from the device.
        for item in &items {
            safe_name(&item.album)?;
            safe_name(&item.name)?;
        }
        let dest = PathBuf::from(dest_dir);
        orchard_library::export_photos(&mut files, &items, &dest, rep.callback()).await?;
        Ok(())
    })
    .await
}
