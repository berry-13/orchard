//! Backup creation, browsing, extraction and restore.
//!
//! Backups use the iTunes / Finder on-disk layout: one folder per device, named by UDID,
//! holding `Info.plist`, `Manifest.plist`, `Status.plist`, `Manifest.db` and the file blobs
//! stored under `<first two hex digits of the file id>/<file id>`. Encrypted backups are
//! fully supported for reading.

mod browse;
mod device;
mod discovery;
mod extract;
mod keybag;
mod mbfile;

pub use browse::{AppData, Backup, DomainInfo, FileEntry, FileKind, FileQuery};
pub use device::{
    RestoreSettings, backup_encryption_enabled, create_backup, restore_backup, set_backup_password,
};
pub use discovery::{BackupSummary, default_backup_dirs, list_backups, read_summary};
pub use extract::ExtractSummary;
