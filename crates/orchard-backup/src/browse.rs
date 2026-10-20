use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use orchard_core::{Error, Result};
use plist::Value;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::discovery::{BackupSummary, read_plist, read_summary};
use crate::keybag::{self, ClassKeys, Keybag};
use crate::mbfile;

const APP_DOMAIN: &str = "AppDomain-";
const APP_PLUGIN_DOMAIN: &str = "AppDomainPlugin-";

/// An opened backup whose `Manifest.db` can be browsed and whose files can be read.
#[derive(Debug)]
pub struct Backup {
    summary: BackupSummary,
    db: Connection,
    keys: Option<ClassKeys>,
    /// Decrypted copy of `Manifest.db`, removed when the backup is dropped.
    _plain_db: Option<NamedTempFile>,
}

/// Kind of a backed up item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

/// One row of `Manifest.db`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// SHA-1 of `domain-relativePath`, also the blob name inside the backup.
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    pub kind: FileKind,
    pub size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub modified: Option<i64>,
    /// Unix mode bits including the file type.
    pub mode: u32,
    pub link_target: Option<String>,
    #[serde(skip)]
    pub(crate) encryption_key: Option<Vec<u8>>,
}

/// Number of entries in one domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainInfo {
    pub name: String,
    pub file_count: u64,
}

/// Backed up data of one app, gathered from its `AppDomain-` and `AppDomainPlugin-` domains.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppData {
    pub bundle_id: String,
    pub domains: Vec<String>,
    pub file_count: u64,
}

/// Filter for [`Backup::files`]. Every set field must match.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileQuery {
    /// Exact domain name.
    pub domain: Option<String>,
    /// Only the item at this relative path and everything below it.
    pub prefix: Option<String>,
    /// Case insensitive substring of the relative path.
    pub search: Option<String>,
    pub limit: Option<u32>,
}

impl Backup {
    /// Opens the backup folder `path` (`<root>/<udid>`). Encrypted backups need `password`;
    /// a missing or wrong one fails with [`Error::BackupPassword`].
    pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self> {
        let path = path.as_ref();
        let summary = read_summary(path)?;
        let db_path = path.join("Manifest.db");
        if !db_path.is_file() {
            return Err(Error::NotFound(db_path.display().to_string()));
        }

        let mut keys = None;
        let mut plain_db = None;
        if summary.encrypted {
            let manifest = read_plist(&path.join("Manifest.plist"))?;
            let keybag = manifest
                .get("BackupKeyBag")
                .and_then(Value::as_data)
                .ok_or_else(|| Error::Protocol("encrypted backup has no BackupKeyBag".into()))?;
            let class_keys =
                Keybag::parse(keybag)?.unlock(password.ok_or(Error::BackupPassword)?)?;
            if let Some(manifest_key) = manifest.get("ManifestKey").and_then(Value::as_data) {
                let key = class_keys.unwrap_item_key(manifest_key)?;
                let plain = keybag::decrypt_all(&key, &fs::read(&db_path)?)?;
                if !plain.starts_with(b"SQLite format 3\0") {
                    return Err(Error::BackupPassword);
                }
                let mut file = NamedTempFile::new()?;
                file.write_all(&plain)?;
                file.flush()?;
                plain_db = Some(file);
            }
            keys = Some(class_keys);
        }

        let open_path = plain_db.as_ref().map_or(db_path.as_path(), |f| f.path());
        let db = Connection::open_with_flags(
            open_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        Ok(Self {
            summary,
            db,
            keys,
            _plain_db: plain_db,
        })
    }

    pub fn summary(&self) -> &BackupSummary {
        &self.summary
    }

    pub fn path(&self) -> &Path {
        &self.summary.path
    }

    /// All domains with their entry counts, sorted by name.
    pub fn domains(&self) -> Result<Vec<DomainInfo>> {
        let mut stmt = self
            .db
            .prepare("SELECT domain, COUNT(*) FROM Files GROUP BY domain ORDER BY domain")
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DomainInfo {
                    name: row.get(0)?,
                    file_count: row.get::<_, i64>(1)? as u64,
                })
            })
            .map_err(sql)?;
        rows.collect::<rusqlite::Result<_>>().map_err(sql)
    }

    /// Per app data. Extension domains (`AppDomainPlugin-<app id>.<ext>`) are grouped under
    /// their app when it is in the backup.
    pub fn apps(&self) -> Result<Vec<AppData>> {
        Ok(group_apps(self.domains()?))
    }

    /// Entries matching `query`, ordered by domain then path.
    pub fn files(&self, query: &FileQuery) -> Result<Vec<FileEntry>> {
        let mut sql_text =
            String::from("SELECT fileID, domain, relativePath, flags, file FROM Files WHERE 1=1");
        let mut params: Vec<String> = Vec::new();
        if let Some(domain) = &query.domain {
            params.push(domain.clone());
            sql_text.push_str(&format!(" AND domain = ?{}", params.len()));
        }
        if let Some(prefix) = query.prefix.as_deref().map(|p| p.trim_matches('/'))
            && !prefix.is_empty()
        {
            params.push(prefix.to_owned());
            let n = params.len();
            sql_text.push_str(&format!(
                    " AND (relativePath = ?{n} OR substr(relativePath, 1, length(?{n}) + 1) = ?{n} || '/')"
                ));
        }
        if let Some(search) = query.search.as_deref().filter(|s| !s.is_empty()) {
            params.push(search.to_lowercase());
            sql_text.push_str(&format!(
                " AND instr(lower(relativePath), ?{}) > 0",
                params.len()
            ));
        }
        sql_text.push_str(" ORDER BY domain, relativePath");
        if let Some(limit) = query.limit {
            sql_text.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = self.db.prepare(&sql_text).map_err(sql)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params.iter()))
            .map_err(sql)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(sql)? {
            entries.push(entry_from_row(row)?);
        }
        Ok(entries)
    }

    /// The entry at `relative_path` in `domain`.
    pub fn file(&self, domain: &str, relative_path: &str) -> Result<FileEntry> {
        self.db
            .query_row(
                "SELECT fileID, domain, relativePath, flags, file FROM Files \
                 WHERE domain = ?1 AND relativePath = ?2",
                [domain, relative_path],
                |row| Ok(entry_from_row(row)),
            )
            .optional()
            .map_err(sql)?
            .ok_or_else(|| Error::NotFound(format!("{domain}/{relative_path}")))?
    }

    /// Reads the whole content of a file entry, decrypting it when needed.
    pub fn read_file(&self, entry: &FileEntry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size as usize);
        self.copy_contents(entry, &mut out, &mut |_| {})?;
        Ok(out)
    }

    /// Streams the plaintext of `entry` into `writer`, reporting bytes written.
    pub(crate) fn copy_contents(
        &self,
        entry: &FileEntry,
        writer: &mut dyn Write,
        on_bytes: &mut dyn FnMut(u64),
    ) -> Result<u64> {
        if entry.kind != FileKind::File {
            return Err(Error::InvalidInput(format!(
                "{} is not a regular file",
                entry.relative_path
            )));
        }
        let blob = match File::open(self.blob_path(&entry.file_id)?) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound && entry.size == 0 => return Ok(0),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::NotFound(format!(
                    "{}/{} (blob {})",
                    entry.domain, entry.relative_path, entry.file_id
                )));
            }
            Err(e) => return Err(e.into()),
        };

        let Some(keys) = &self.keys else {
            let mut written = 0u64;
            let mut reader = io::BufReader::new(blob);
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    return Ok(written);
                }
                writer.write_all(&buf[..n])?;
                written += n as u64;
                on_bytes(written);
            }
        };
        let wrapped = match &entry.encryption_key {
            Some(key) => key.clone(),
            None => self
                .file_by_id(&entry.file_id)?
                .encryption_key
                .ok_or_else(|| {
                    Error::Protocol(format!("{} has no encryption key", entry.relative_path))
                })?,
        };
        let key = keys.unwrap_item_key(&wrapped)?;
        keybag::decrypt_stream(&key, io::BufReader::new(blob), writer, entry.size, on_bytes)
    }

    fn file_by_id(&self, file_id: &str) -> Result<FileEntry> {
        self.db
            .query_row(
                "SELECT fileID, domain, relativePath, flags, file FROM Files WHERE fileID = ?1",
                [file_id],
                |row| Ok(entry_from_row(row)),
            )
            .optional()
            .map_err(sql)?
            .ok_or_else(|| Error::NotFound(format!("file id {file_id}")))?
    }

    fn blob_path(&self, file_id: &str) -> Result<PathBuf> {
        if file_id.len() < 3 || !file_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Protocol(format!("invalid file id {file_id:?}")));
        }
        Ok(self.summary.path.join(&file_id[..2]).join(file_id))
    }
}

fn entry_from_row(row: &Row<'_>) -> Result<FileEntry> {
    let file_id: String = row.get(0).map_err(sql)?;
    let domain: String = row.get(1).map_err(sql)?;
    let relative_path: String = row.get(2).map_err(sql)?;
    let flags: i64 = row.get(3).map_err(sql)?;
    let blob: Option<Vec<u8>> = row.get(4).map_err(sql)?;
    let meta = match blob {
        Some(blob) if !blob.is_empty() => mbfile::decode(&blob)?,
        _ => mbfile::MbFile::default(),
    };
    let kind = match flags {
        2 => FileKind::Directory,
        4 => FileKind::Symlink,
        _ => FileKind::File,
    };
    Ok(FileEntry {
        file_id,
        domain,
        relative_path,
        kind,
        size: meta.size,
        modified: meta.modified,
        mode: meta.mode,
        link_target: meta.target,
        encryption_key: meta.encryption_key,
    })
}

fn group_apps(domains: Vec<DomainInfo>) -> Vec<AppData> {
    let mut apps: BTreeMap<String, AppData> = BTreeMap::new();
    let mut plugins = Vec::new();
    for domain in domains {
        if let Some(id) = domain.name.strip_prefix(APP_DOMAIN) {
            let app = apps.entry(id.to_owned()).or_insert_with(|| AppData {
                bundle_id: id.to_owned(),
                domains: Vec::new(),
                file_count: 0,
            });
            app.domains.insert(0, domain.name.clone());
            app.file_count += domain.file_count;
        } else if domain.name.starts_with(APP_PLUGIN_DOMAIN) {
            plugins.push(domain);
        }
    }
    for plugin in plugins {
        let id = &plugin.name[APP_PLUGIN_DOMAIN.len()..];
        let parent = apps
            .keys()
            .filter(|app| id.len() > app.len() && id.starts_with(app.as_str()))
            .filter(|app| id.as_bytes()[app.len()] == b'.')
            .max_by_key(|app| app.len())
            .cloned()
            .unwrap_or_else(|| id.to_owned());
        let app = apps.entry(parent.clone()).or_insert_with(|| AppData {
            bundle_id: parent,
            domains: Vec::new(),
            file_count: 0,
        });
        app.domains.push(plugin.name);
        app.file_count += plugin.file_count;
    }
    apps.into_values().collect()
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Protocol(format!("Manifest.db: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(name: &str, file_count: u64) -> DomainInfo {
        DomainInfo {
            name: name.into(),
            file_count,
        }
    }

    #[test]
    fn groups_plugins_under_their_app() {
        let apps = group_apps(vec![
            domain("AppDomain-com.example.app", 3),
            domain("AppDomain-com.example.app2", 1),
            domain("AppDomainPlugin-com.example.app.widget", 2),
            domain("AppDomainPlugin-com.other.share", 1),
            domain("HomeDomain", 10),
        ]);
        let ids: Vec<_> = apps.iter().map(|a| a.bundle_id.as_str()).collect();
        assert_eq!(
            ids,
            ["com.example.app", "com.example.app2", "com.other.share"]
        );
        assert_eq!(apps[0].file_count, 5);
        assert_eq!(
            apps[0].domains,
            [
                "AppDomain-com.example.app",
                "AppDomainPlugin-com.example.app.widget"
            ]
        );
    }
}
