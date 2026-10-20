//! Local backups: list, create, browse, extract, restore.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::Events;
use orchard_backup::{Backup, FileQuery, RestoreSettings};
use orchard_core::{Error, Progress};

use crate::dto::{BackupDomain, BackupFile, BackupMode, BackupSummary, DomainKind};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::paths::{safe_join, safe_name};
use crate::state::{BackupState, DeviceState, blocking, locked};

/// Most files returned for one domain listing.
const FILE_LIMIT: u32 = 5000;

/// Where Orchard writes new backups.
pub fn backup_root(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// Unix milliseconds of an RFC 3339 timestamp such as `2024-05-01T10:20:30Z`.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = s.get(19..)?;
    let mut millis = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        millis = format!("{digits:0<3}").get(..3)?.parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_minutes = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let h: i64 = rest.get(1..3)?.parse().ok()?;
            let m: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (h * 60 + m)
        }
    };
    // Days from civil, Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(secs * 1000 + millis)
}

pub fn to_summary(b: orchard_backup::BackupSummary) -> BackupSummary {
    BackupSummary {
        id: b.path.display().to_string(),
        udid: b.udid.0.clone(),
        device_name: b.device_name.unwrap_or_else(|| b.udid.0.clone()),
        product_type: b.product_type.unwrap_or_default(),
        ios_version: b.ios_version.unwrap_or_default(),
        date: b
            .last_backup_date
            .as_deref()
            .and_then(parse_rfc3339)
            .unwrap_or(0),
        encrypted: b.encrypted,
        size: b.size_on_disk,
        path: b.path.display().to_string(),
    }
}

/// App domains carry the bundle id after the prefix; everything else is a system domain.
pub fn to_domain(d: orchard_backup::DomainInfo) -> BackupDomain {
    const APP_PREFIXES: [(&str, &str); 3] = [
        ("AppDomain-", ""),
        ("AppDomainGroup-", " (group)"),
        ("AppDomainPlugin-", " (extension)"),
    ];
    for (prefix, suffix) in APP_PREFIXES {
        if let Some(bundle) = d.name.strip_prefix(prefix) {
            return BackupDomain {
                label: format!("{bundle}{suffix}"),
                kind: DomainKind::App,
                bundle_id: Some(bundle.to_owned()),
                file_count: d.file_count,
                size: 0,
                domain: d.name,
            };
        }
    }
    BackupDomain {
        label: d.name.strip_suffix("Domain").unwrap_or(&d.name).to_owned(),
        kind: DomainKind::System,
        bundle_id: None,
        file_count: d.file_count,
        size: 0,
        domain: d.name,
    }
}

pub fn to_file(f: orchard_backup::FileEntry) -> BackupFile {
    BackupFile {
        file_id: f.file_id,
        domain: f.domain,
        relative_path: f.relative_path,
        size: f.size,
        modified: f.modified.map(|s| s * 1000),
    }
}

/// `dest/<domain>/<relative path>`, refusing paths that would leave `dest`.
pub fn extract_target(dest: &Path, domain: &str, relative: &str) -> CmdResult<PathBuf> {
    safe_join(&dest.join(safe_name(domain)?), relative)
}

fn find_backup(devices: &DeviceState, id: &str) -> CmdResult<orchard_backup::BackupSummary> {
    all_backups(devices.data_dir())
        .into_iter()
        .find(|b| b.path.display().to_string() == id)
        .ok_or_else(|| Error::NotFound(format!("backup {id}")).into())
}

fn all_backups(data_dir: &Path) -> Vec<orchard_backup::BackupSummary> {
    orchard_backup::default_backup_dirs(Some(&backup_root(data_dir)))
        .iter()
        .filter_map(|root| orchard_backup::list_backups(root).ok())
        .flatten()
        .collect()
}

pub async fn list_backups(devices: &Arc<DeviceState>) -> CmdResult<Vec<BackupSummary>> {
    let data_dir = devices.data_dir().to_path_buf();
    blocking(move || {
        let mut list: Vec<_> = all_backups(&data_dir).into_iter().map(to_summary).collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.date));
        Ok(list)
    })
    .await
}

/// Task stage: `backup` (bytes, or percent when the device gives no total).
pub async fn create_backup(
    app: Events,
    tasks: &TaskRegistry,
    devices: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    mode: BackupMode,
) -> CmdResult<()> {
    let conn = devices.connection(&udid).await?;
    let root = backup_root(devices.data_dir());
    run_task(&app, tasks, task_id, |rep| async move {
        rep.progress(&Progress::new("prepare", 0, None));
        tokio::fs::create_dir_all(&root).await?;
        orchard_backup::create_backup(&conn, &root, mode == BackupMode::Full, rep.callback())
            .await?;
        Ok(())
    })
    .await
}

/// Unlocks (when encrypted) and indexes a backup. Fails with
/// `backupPassword` when the password is missing or wrong. The opened
/// backup stays available to the other backup commands.
pub async fn open_backup(
    devices: &Arc<DeviceState>,
    backups: &BackupState,
    backup_id: String,
    password: Option<String>,
) -> CmdResult<Vec<BackupDomain>> {
    let summary = find_backup(devices, &backup_id)?;
    let password = password.filter(|p| !p.is_empty());
    let (backup, domains) = blocking(move || {
        let backup = Backup::open(&summary.path, password.as_deref())?;
        let domains = backup.domains()?;
        Ok((backup, domains))
    })
    .await?;
    backups.insert(&backup_id, backup);
    let mut domains: Vec<_> = domains.into_iter().map(to_domain).collect();
    domains.sort_by(|a, b| {
        (a.kind == DomainKind::App, &a.label).cmp(&(b.kind == DomainKind::App, &b.label))
    });
    Ok(domains)
}

pub async fn list_backup_files(
    backups: &BackupState,
    backup_id: String,
    domain: String,
    query: Option<String>,
) -> CmdResult<Vec<BackupFile>> {
    let backup = backups.get(&backup_id)?;
    blocking(move || {
        let files = locked(&backup).files(&FileQuery {
            domain: Some(domain),
            prefix: None,
            search: query.filter(|q| !q.trim().is_empty()),
            limit: Some(FILE_LIMIT),
        })?;
        Ok(files
            .into_iter()
            .filter(|f| f.kind == orchard_backup::FileKind::File)
            .map(to_file)
            .collect())
    })
    .await
}

/// Writes the files to `<dest_dir>/<domain>/<path>`. Task stage: `extract` (files).
pub async fn extract_backup_files(
    app: Events,
    tasks: &TaskRegistry,
    backups: &BackupState,
    task_id: TaskId,
    backup_id: String,
    file_ids: Vec<String>,
    dest_dir: String,
) -> CmdResult<()> {
    let backup = backups.get(&backup_id)?;
    run_task(&app, tasks, task_id, |rep| {
        blocking(move || {
            let wanted: HashSet<String> = file_ids.into_iter().collect();
            let backup = locked(&backup);
            let entries: Vec<_> = backup
                .files(&FileQuery::default())?
                .into_iter()
                .filter(|f| wanted.contains(&f.file_id))
                .collect();
            let total = entries.len() as u64;
            let dest = PathBuf::from(dest_dir);
            for (i, entry) in entries.iter().enumerate() {
                rep.check()?;
                rep.progress(&Progress::new("extract", i as u64, Some(total)));
                let target = extract_target(&dest, &entry.domain, &entry.relative_path)?;
                backup.extract_file(entry, &target)?;
            }
            rep.progress(&Progress::new("extract", total, Some(total)));
            Ok(())
        })
    })
    .await
}

/// Task stage: `restore`. The device reboots when it finishes.
pub async fn restore_backup(
    app: Events,
    tasks: &TaskRegistry,
    devices: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    backup_id: String,
    password: Option<String>,
) -> CmdResult<()> {
    let summary = find_backup(devices, &backup_id)?;
    let conn = devices.connection(&udid).await?;
    let root = summary
        .path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::InvalidInput(format!("backup {backup_id}")))?;
    let source = summary.udid.0.clone();
    let settings = RestoreSettings {
        password: password.filter(|p| !p.is_empty()),
        ..RestoreSettings::default()
    };
    run_task(&app, tasks, task_id, |rep| async move {
        rep.progress(&Progress::new("prepare", 0, None));
        orchard_backup::restore_backup(&conn, &root, Some(&source), &settings, rep.callback())
            .await?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2024-05-01T10:20:30Z"),
            Some(1_714_558_830_000)
        );
        assert_eq!(
            parse_rfc3339("2024-05-01T12:20:30.5+02:00"),
            Some(1_714_558_830_500)
        );
        assert_eq!(parse_rfc3339("not a date"), None);
    }

    #[test]
    fn app_domains_get_bundle_ids() {
        let d = to_domain(orchard_backup::DomainInfo {
            name: "AppDomain-com.example.app".into(),
            file_count: 3,
        });
        assert_eq!(d.kind, DomainKind::App);
        assert_eq!(d.bundle_id.as_deref(), Some("com.example.app"));
        let s = to_domain(orchard_backup::DomainInfo {
            name: "HomeDomain".into(),
            file_count: 1,
        });
        assert_eq!((s.kind, s.label.as_str()), (DomainKind::System, "Home"));
    }

    #[test]
    fn extract_target_rejects_escapes() {
        let dest = Path::new("/out");
        assert_eq!(
            extract_target(dest, "HomeDomain", "Library/a.db").unwrap(),
            PathBuf::from("/out/HomeDomain/Library/a.db")
        );
        assert!(extract_target(dest, "HomeDomain", "../etc/passwd").is_err());
        assert!(extract_target(dest, "HomeDomain", "/etc/passwd").is_err());
        assert!(extract_target(dest, "../x", "a").is_err());
    }
}
