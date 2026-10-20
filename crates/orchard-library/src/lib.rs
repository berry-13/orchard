//! Local media library and device sync.

mod media;
mod photos;

pub use media::{
    MediaCategory, MediaFile, MediaSink, MediaTarget, SendSummary, list_media, media_category,
    media_targets, media_targets_from_apps, send_media,
};
pub use photos::{
    ExportSummary, MediaKind, MediaSource, PhotoItem, export_photos, list_camera_roll,
};
