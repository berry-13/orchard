use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use orchard_core::{Error, Result, Udid};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};

/// What Orchard knows about a backup folder without opening its database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub udid: Udid,
    /// The backup folder, `<root>/<udid>`.
    pub path: PathBuf,
    pub device_name: Option<String>,
    /// Hardware model, e.g. `iPhone15,2`.
    pub product_type: Option<String>,
    pub ios_version: Option<String>,
    /// RFC 3339 timestamp of the last backup.
    pub last_backup_date: Option<String>,
    pub encrypted: bool,
    /// The last backup was a full one rather than incremental.
    pub full_backup: bool,
    /// The last backup finished (`SnapshotState` is `finished`).
    pub complete: bool,
    /// Bytes used by the backup folder.
    pub size_on_disk: u64,
}

/// Candidate backup roots: `orchard_dir` first, then the iTunes / Finder locations for
/// the current platform. The folders may not exist; [`list_backups`] treats a missing
/// folder as empty.
pub fn default_backup_dirs(orchard_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = orchard_dir.map(Path::to_path_buf).into_iter().collect();
    for dir in platform_dirs(std::env::consts::OS, |key| std::env::var_os(key)) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn platform_dirs(os: &str, env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let under = |var: &str, parts: &[&str]| {
        env(var).filter(|v| !v.is_empty()).map(|base| {
            parts
                .iter()
                .fold(PathBuf::from(base), |p, part| p.join(part))
        })
    };
    match os {
        "macos" => under(
            "HOME",
            &["Library", "Application Support", "MobileSync", "Backup"],
        )
        .into_iter()
        .collect(),
        "windows" => [
            under("APPDATA", &["Apple Computer", "MobileSync", "Backup"]),
            under("USERPROFILE", &["Apple", "MobileSync", "Backup"]),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    }
}

/// Lists the backups stored in `root`, one per subfolder, newest first.
///
/// Subfolders that do not look like backups or whose plists cannot be read are skipped.
pub fn list_backups(root: &Path) -> Result<Vec<BackupSummary>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Ok(summary) = read_summary(&entry.path()) {
            backups.push(summary);
        }
    }
    backups.sort_by(|a, b| b.last_backup_date.cmp(&a.last_backup_date));
    Ok(backups)
}

/// Reads the summary of one backup folder from its `Info.plist`, `Manifest.plist` and
/// `Status.plist`.
pub fn read_summary(path: &Path) -> Result<BackupSummary> {
    let info = read_plist_opt(&path.join("Info.plist"))?;
    let manifest = read_plist_opt(&path.join("Manifest.plist"))?;
    let status = read_plist_opt(&path.join("Status.plist"))?;
    if info.is_none() && manifest.is_none() && status.is_none() {
        return Err(Error::NotFound(format!("backup at {}", path.display())));
    }

    let lockdown = manifest
        .as_ref()
        .and_then(|m| m.get("Lockdown"))
        .and_then(Value::as_dictionary);
    let info_str = |key: &str| string(info.as_ref(), key);
    let lockdown_str = |key: &str| string(lockdown, key);

    let udid = info_str("Unique Identifier")
        .or_else(|| info_str("Target Identifier"))
        .or_else(|| lockdown_str("UniqueDeviceID"))
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let last_backup_date = date(info.as_ref(), "Last Backup Date")
        .or_else(|| date(status.as_ref(), "Date"))
        .or_else(|| date(manifest.as_ref(), "Date"));
    let flag = |dict: Option<&Dictionary>, key: &str| {
        dict.and_then(|d| d.get(key))
            .and_then(Value::as_boolean)
            .unwrap_or(false)
    };

    Ok(BackupSummary {
        udid: Udid(udid),
        path: path.to_path_buf(),
        device_name: info_str("Device Name")
            .or_else(|| info_str("Display Name"))
            .or_else(|| lockdown_str("DeviceName")),
        product_type: info_str("Product Type").or_else(|| lockdown_str("ProductType")),
        ios_version: info_str("Product Version").or_else(|| lockdown_str("ProductVersion")),
        last_backup_date,
        encrypted: flag(manifest.as_ref(), "IsEncrypted"),
        full_backup: flag(status.as_ref(), "IsFullBackup"),
        complete: string(status.as_ref(), "SnapshotState").as_deref() == Some("finished"),
        size_on_disk: dir_size(path)?,
    })
}

pub(crate) fn read_plist(path: &Path) -> Result<Dictionary> {
    read_plist_opt(path)?.ok_or_else(|| Error::NotFound(path.display().to_string()))
}

fn read_plist_opt(path: &Path) -> Result<Option<Dictionary>> {
    match plist::from_file::<_, Value>(path) {
        Ok(Value::Dictionary(dict)) => Ok(Some(dict)),
        Ok(_) => Err(Error::Protocol(format!(
            "{} is not a dictionary",
            path.display()
        ))),
        Err(e) if e.as_io().map(std::io::Error::kind) == Some(std::io::ErrorKind::NotFound) => {
            Ok(None)
        }
        Err(e) => Err(Error::Protocol(format!("{}: {e}", path.display()))),
    }
}

fn string(dict: Option<&Dictionary>, key: &str) -> Option<String> {
    dict?.get(key)?.as_string().map(str::to_owned)
}

fn date(dict: Option<&Dictionary>, key: &str) -> Option<String> {
    Some(dict?.get(key)?.as_date()?.to_xml_format())
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| {
            vars.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn macos_location() {
        let dirs = platform_dirs("macos", env(&[("HOME", "/Users/me")]));
        assert_eq!(
            dirs,
            vec![PathBuf::from(
                "/Users/me/Library/Application Support/MobileSync/Backup"
            )]
        );
    }

    #[test]
    fn windows_locations() {
        let dirs = platform_dirs(
            "windows",
            env(&[
                ("APPDATA", "C:/Users/me/AppData/Roaming"),
                ("USERPROFILE", ""),
            ]),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("C:/Users/me/AppData/Roaming")
                    .join("Apple Computer")
                    .join("MobileSync")
                    .join("Backup")
            ]
        );
    }

    #[test]
    fn other_platforms_have_no_default() {
        assert!(platform_dirs("linux", env(&[("HOME", "/home/me")])).is_empty());
    }
}
