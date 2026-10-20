//! Device side backup operations over `com.apple.mobilebackup2`.

use std::future::Future;
use std::io::{Read, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::SystemTime;

use idevice::lockdown::LockdownClient;
use idevice::mobilebackup2::{
    BackupDelegate, BackupProgress, DirEntryInfo, FsBackupDelegate, MobileBackup2Client,
    RestoreOptions,
};
use idevice::provider::IdeviceProvider;
use idevice::{IdeviceError, IdeviceService};
use orchard_core::{Error, Progress, Result};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};

use crate::discovery::{BackupSummary, read_summary};

/// Options for [`restore_backup`]. Defaults match iTunes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RestoreSettings {
    /// Reboot the device when the restore finishes.
    pub reboot: bool,
    /// Keep the device's current settings instead of the backed up ones.
    pub preserve_settings: bool,
    pub system_files: bool,
    /// Delete items on the device that are not in the backup.
    pub remove_items_not_restored: bool,
    /// Password of an encrypted backup.
    pub password: Option<String>,
}

impl Default for RestoreSettings {
    fn default() -> Self {
        Self {
            reboot: true,
            preserve_settings: true,
            system_files: false,
            remove_items_not_restored: false,
            password: None,
        }
    }
}

/// Backs the device up into `backup_root/<udid>`. The backup is incremental when a
/// complete previous backup is already there, unless `full` forces a full one.
///
/// Progress uses the `backup` stage, in bytes when the device announced a total and in
/// percent otherwise.
pub async fn create_backup(
    provider: &dyn IdeviceProvider,
    backup_root: &Path,
    full: bool,
    progress: impl FnMut(Progress) + Send,
) -> Result<BackupSummary> {
    let values = device_values(provider).await?;
    let udid = values
        .get("UniqueDeviceID")
        .and_then(Value::as_string)
        .ok_or_else(|| Error::Device("device did not report its UDID".into()))?
        .to_owned();
    let backup_dir = backup_root.join(&udid);
    std::fs::create_dir_all(&backup_dir)?;
    let full = full || !backup_dir.join("Status.plist").is_file();
    write_info_plist(&backup_dir.join("Info.plist"), &values)?;

    let delegate = ProgressDelegate::new("backup", progress);
    let mut client = MobileBackup2Client::connect(provider)
        .await
        .map_err(device)?;
    let mut options = Dictionary::new();
    if full {
        options.insert("ForceFullBackup".into(), Value::Boolean(true));
    }
    let response = client
        .backup_from_path(backup_root, Some(&udid), Some(options), &delegate)
        .await
        .map_err(device);
    let _ = client.disconnect().await;
    check_response(response?)?;
    read_summary(&backup_dir)
}

/// Restores the backup at `backup_root/<source_udid>` onto the device. `source_udid`
/// defaults to the device's own UDID. Progress uses the `restore` stage.
pub async fn restore_backup(
    provider: &dyn IdeviceProvider,
    backup_root: &Path,
    source_udid: Option<&str>,
    settings: &RestoreSettings,
    progress: impl FnMut(Progress) + Send,
) -> Result<()> {
    if let Some(source) = source_udid
        && !backup_root.join(source).join("Manifest.plist").is_file()
    {
        return Err(Error::NotFound(format!("backup {source}")));
    }
    let options = RestoreOptions {
        reboot: settings.reboot,
        copy: true,
        preserve_settings: settings.preserve_settings,
        system_files: settings.system_files,
        remove_items_not_restored: settings.remove_items_not_restored,
        password: settings.password.clone(),
    };
    let delegate = ProgressDelegate::new("restore", progress);
    let mut client = MobileBackup2Client::connect(provider)
        .await
        .map_err(device)?;
    let response = client
        .restore_from_path(backup_root, source_udid, Some(options), &delegate)
        .await
        .map_err(device);
    let _ = client.disconnect().await;
    check_response(response?)
}

/// Whether the device encrypts its backups (`WillEncrypt` in `com.apple.mobile.backup`).
pub async fn backup_encryption_enabled(provider: &dyn IdeviceProvider) -> Result<bool> {
    let mut lockdown = lockdown_session(provider).await?;
    let value = lockdown
        .get_value(Some("WillEncrypt"), Some("com.apple.mobile.backup"))
        .await
        .map_err(device)?;
    Ok(value.as_boolean().unwrap_or(false))
}

/// Enables (`old` is `None`), disables (`new` is `None`) or changes backup encryption.
/// The device may ask for its passcode on screen.
pub async fn set_backup_password(
    provider: &dyn IdeviceProvider,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<()> {
    if old.is_none() && new.is_none() {
        return Err(Error::InvalidInput(
            "give the old or the new password".into(),
        ));
    }
    let host_dir = tempfile::tempdir()?;
    let delegate = FsBackupDelegate;
    let mut client = MobileBackup2Client::connect(provider)
        .await
        .map_err(device)?;
    let result = client
        .change_password_from_path(host_dir.path(), old, new, &delegate)
        .await
        .map_err(|e| match e {
            IdeviceError::UnexpectedResponse(msg) if msg.contains("assword") => {
                Error::BackupPassword
            }
            e => device(e),
        });
    let _ = client.disconnect().await;
    result
}

async fn lockdown_session(provider: &dyn IdeviceProvider) -> Result<LockdownClient> {
    let mut lockdown = LockdownClient::connect(provider).await.map_err(device)?;
    let pairing = provider.get_pairing_file().await.map_err(device)?;
    lockdown.start_session(&pairing).await.map_err(device)?;
    Ok(lockdown)
}

async fn device_values(provider: &dyn IdeviceProvider) -> Result<Dictionary> {
    let mut lockdown = lockdown_session(provider).await?;
    match lockdown.get_value(None, None).await.map_err(device)? {
        Value::Dictionary(dict) => Ok(dict),
        _ => Err(Error::Protocol(
            "lockdown values are not a dictionary".into(),
        )),
    }
}

/// Writes the host side `Info.plist` iTunes keeps next to each backup.
fn write_info_plist(path: &Path, values: &Dictionary) -> Result<()> {
    let mut info = Dictionary::new();
    let copies = [
        ("Device Name", "DeviceName"),
        ("Display Name", "DeviceName"),
        ("Product Type", "ProductType"),
        ("Product Version", "ProductVersion"),
        ("Build Version", "BuildVersion"),
        ("Serial Number", "SerialNumber"),
        ("Unique Identifier", "UniqueDeviceID"),
        ("Target Identifier", "UniqueDeviceID"),
        ("IMEI", "InternationalMobileEquipmentIdentity"),
        ("Phone Number", "PhoneNumber"),
    ];
    for (key, source) in copies {
        if let Some(value) = values.get(source) {
            info.insert(key.into(), value.clone());
        }
    }
    info.insert("Target Type".into(), Value::String("Device".into()));
    info.insert(
        "Last Backup Date".into(),
        Value::Date(SystemTime::now().into()),
    );
    plist::to_file_xml(path, &info).map_err(|e| Error::Protocol(format!("Info.plist: {e}")))
}

/// Maps the final status dictionary of a DeviceLink session to a result.
fn check_response(response: Option<Dictionary>) -> Result<()> {
    let Some(response) = response else {
        return Ok(());
    };
    let code = response
        .get("ErrorCode")
        .and_then(Value::as_signed_integer)
        .unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let description = response
        .get("ErrorDescription")
        .and_then(Value::as_string)
        .unwrap_or("unknown error");
    if description.to_lowercase().contains("password") {
        return Err(Error::BackupPassword);
    }
    Err(Error::Device(format!(
        "mobilebackup2 error {code}: {description}"
    )))
}

fn device(e: IdeviceError) -> Error {
    match e {
        IdeviceError::NotFound => Error::NotFound("backup".into()),
        e => Error::Device(e.to_string()),
    }
}

fn to_progress(stage: &str, p: &BackupProgress) -> Progress {
    if p.session_bytes_total > 0 {
        Progress::new(stage, p.session_bytes_done, Some(p.session_bytes_total))
    } else if p.overall_progress >= 0.0 {
        Progress::new(stage, p.overall_progress.round() as u64, Some(100))
    } else {
        Progress::new(stage, p.session_bytes_done, None)
    }
}

/// Filesystem delegate that forwards progress snapshots to an Orchard callback.
struct ProgressDelegate<F> {
    stage: &'static str,
    callback: Mutex<F>,
}

impl<F: FnMut(Progress) + Send> ProgressDelegate<F> {
    fn new(stage: &'static str, callback: F) -> Self {
        Self {
            stage,
            callback: Mutex::new(callback),
        }
    }
}

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl<F: FnMut(Progress) + Send> BackupDelegate for ProgressDelegate<F> {
    fn get_free_disk_space(&self, path: &Path) -> u64 {
        FsBackupDelegate.get_free_disk_space(path)
    }

    fn open_file_read<'a>(
        &'a self,
        path: &'a Path,
    ) -> Fut<'a, std::result::Result<Box<dyn Read + Send>, IdeviceError>> {
        FsBackupDelegate.open_file_read(path)
    }

    fn create_file_write<'a>(
        &'a self,
        path: &'a Path,
    ) -> Fut<'a, std::result::Result<Box<dyn Write + Send>, IdeviceError>> {
        FsBackupDelegate.create_file_write(path)
    }

    fn create_dir_all<'a>(
        &'a self,
        path: &'a Path,
    ) -> Fut<'a, std::result::Result<(), IdeviceError>> {
        FsBackupDelegate.create_dir_all(path)
    }

    fn remove<'a>(&'a self, path: &'a Path) -> Fut<'a, std::result::Result<(), IdeviceError>> {
        FsBackupDelegate.remove(path)
    }

    fn rename<'a>(
        &'a self,
        from: &'a Path,
        to: &'a Path,
    ) -> Fut<'a, std::result::Result<(), IdeviceError>> {
        FsBackupDelegate.rename(from, to)
    }

    fn copy<'a>(
        &'a self,
        src: &'a Path,
        dst: &'a Path,
    ) -> Fut<'a, std::result::Result<(), IdeviceError>> {
        FsBackupDelegate.copy(src, dst)
    }

    fn exists<'a>(&'a self, path: &'a Path) -> Fut<'a, bool> {
        FsBackupDelegate.exists(path)
    }

    fn is_dir<'a>(&'a self, path: &'a Path) -> Fut<'a, bool> {
        FsBackupDelegate.is_dir(path)
    }

    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
    ) -> Fut<'a, std::result::Result<Vec<DirEntryInfo>, IdeviceError>> {
        FsBackupDelegate.list_dir(path)
    }

    fn on_progress(&self, progress: BackupProgress) {
        if let Ok(mut callback) = self.callback.lock() {
            callback(to_progress(self.stage, &progress));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_prefers_bytes_then_percent() {
        let bytes = BackupProgress {
            session_bytes_done: 10,
            session_bytes_total: 40,
            overall_progress: 25.0,
            ..BackupProgress::default()
        };
        assert_eq!(
            to_progress("backup", &bytes),
            Progress::new("backup", 10, Some(40))
        );
        let percent = BackupProgress {
            overall_progress: 42.6,
            ..BackupProgress::default()
        };
        assert_eq!(
            to_progress("backup", &percent),
            Progress::new("backup", 43, Some(100))
        );
        let unknown = BackupProgress {
            session_bytes_done: 7,
            overall_progress: -1.0,
            ..BackupProgress::default()
        };
        assert_eq!(
            to_progress("backup", &unknown),
            Progress::new("backup", 7, None)
        );
    }

    #[test]
    fn delegate_forwards_progress() {
        let mut seen = Vec::new();
        {
            let delegate = ProgressDelegate::new("restore", |p| seen.push(p));
            delegate.on_progress(BackupProgress {
                overall_progress: 50.0,
                ..BackupProgress::default()
            });
        }
        assert_eq!(seen, [Progress::new("restore", 50, Some(100))]);
    }

    #[test]
    fn response_errors_are_mapped() {
        assert!(check_response(None).is_ok());
        let mut ok = Dictionary::new();
        ok.insert("ErrorCode".into(), Value::Integer(0.into()));
        assert!(check_response(Some(ok)).is_ok());

        let mut failed = Dictionary::new();
        failed.insert("ErrorCode".into(), Value::Integer(105.into()));
        failed.insert("ErrorDescription".into(), Value::String("no space".into()));
        assert!(matches!(
            check_response(Some(failed)),
            Err(Error::Device(_))
        ));

        let mut password = Dictionary::new();
        password.insert("ErrorCode".into(), Value::Integer(207.into()));
        password.insert(
            "ErrorDescription".into(),
            Value::String("Invalid password".into()),
        );
        assert!(matches!(
            check_response(Some(password)),
            Err(Error::BackupPassword)
        ));
    }
}
