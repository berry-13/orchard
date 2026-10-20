//! Sending media files to apps that accept them through file sharing.

use std::path::PathBuf;
use std::sync::Arc;

use crate::events::Events;
use orchard_device::FileLocation;

use crate::commands::files::to_entry;
use crate::dto::{MediaCategory, MediaFile, MediaTarget, SendSummary};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::state::DeviceState;

pub fn to_category(c: orchard_library::MediaCategory) -> MediaCategory {
    match c {
        orchard_library::MediaCategory::Audio => MediaCategory::Audio,
        orchard_library::MediaCategory::Video => MediaCategory::Video,
        orchard_library::MediaCategory::Book => MediaCategory::Book,
        orchard_library::MediaCategory::Other => MediaCategory::Other,
    }
}

pub fn to_target(t: orchard_library::MediaTarget) -> MediaTarget {
    MediaTarget {
        bundle_id: t.bundle_id,
        name: t.name,
        accepts: t.accepts.into_iter().map(to_category).collect(),
        recognized: t.recognized,
    }
}

pub fn to_media_file(f: orchard_library::MediaFile) -> MediaFile {
    MediaFile {
        category: f.category.map(to_category),
        entry: to_entry(f.entry),
    }
}

pub fn to_send_summary(s: orchard_library::SendSummary) -> SendSummary {
    SendSummary {
        uploaded: s.uploaded as u64,
        skipped: s.skipped as u64,
        bytes: s.bytes,
        renamed: s.renamed,
        skipped_symlinks: s.skipped_symlinks as u64,
        skipped_invalid: s.skipped_invalid as u64,
    }
}

/// Installed user apps with file sharing, tagged with what they accept.
pub async fn media_targets(state: &Arc<DeviceState>, udid: String) -> CmdResult<Vec<MediaTarget>> {
    let conn = state.connection(&udid).await?;
    let targets = orchard_library::media_targets(&conn).await?;
    Ok(targets.into_iter().map(to_target).collect())
}

/// Lists `dir` (for example `/Documents`) of an app's shared folder.
pub async fn list_media(
    state: &Arc<DeviceState>,
    udid: String,
    bundle_id: String,
    dir: String,
) -> CmdResult<Vec<MediaFile>> {
    let mut files = state
        .connection(&udid)
        .await?
        .files(FileLocation::AppDocuments(bundle_id))
        .await?;
    let list = orchard_library::list_media(&mut files, &dir).await?;
    Ok(list.into_iter().map(to_media_file).collect())
}

/// Sends local files and folders into the app's Documents folder (optionally
/// under `subdir`). Same-size files are skipped, name clashes get `name (n)`.
/// Task stage: `send` (bytes).
pub async fn send_media(
    app: Events,
    tasks: &TaskRegistry,
    state: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    bundle_id: String,
    local_paths: Vec<String>,
    subdir: Option<String>,
) -> CmdResult<SendSummary> {
    let mut files = state
        .connection(&udid)
        .await?
        .files(FileLocation::AppDocuments(bundle_id))
        .await?;
    run_task(&app, tasks, task_id, |rep| async move {
        let paths: Vec<PathBuf> = local_paths.into_iter().map(PathBuf::from).collect();
        let subdir = subdir.filter(|s| !s.trim().is_empty());
        let summary =
            orchard_library::send_media(&mut files, &paths, subdir.as_deref(), rep.callback())
                .await?;
        Ok(to_send_summary(summary))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_serializes_for_the_ui() {
        let s = to_send_summary(orchard_library::SendSummary {
            uploaded: 2,
            skipped: 1,
            bytes: 10,
            renamed: vec!["/Documents/a (1).mp4".into()],
            skipped_symlinks: 0,
            skipped_invalid: 3,
        });
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["uploaded"], 2);
        assert_eq!(v["renamed"][0], "/Documents/a (1).mp4");
        assert_eq!(v["skippedInvalid"], 3);
    }

    #[test]
    fn categories_are_camel_case() {
        let t = to_target(orchard_library::MediaTarget {
            bundle_id: "org.videolan.vlc-ios".into(),
            name: "VLC".into(),
            accepts: vec![
                orchard_library::MediaCategory::Audio,
                orchard_library::MediaCategory::Video,
            ],
            recognized: true,
        });
        let v = serde_json::to_value(t).unwrap();
        assert_eq!(v["accepts"], serde_json::json!(["audio", "video"]));
        assert_eq!(v["bundleId"], "org.videolan.vlc-ios");
    }
}
