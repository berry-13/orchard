use std::collections::HashMap;
use std::path::{Path, PathBuf};

use idevice::IdeviceError;
use idevice::afc::AfcClient;
use idevice::afc::errors::AfcError;
use idevice::afc::opcode::AfcFopenMode;
use idevice::house_arrest::HouseArrestClient;
use orchard_core::{Error, Progress, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::connect::DeviceConnection;
use crate::error::device_error;

const CHUNK: usize = 256 * 1024;

/// Which file system an AFC session sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "bundleId", rename_all = "camelCase")]
pub enum FileLocation {
    /// The media partition (`/DCIM`, `/Downloads`, `/PublicStaging`, ...).
    Media,
    /// An app's whole sandbox container. Only works for development builds.
    AppContainer(String),
    /// An app's `Documents` folder, for apps with file sharing enabled.
    AppDocuments(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One entry of a device directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Absolute, normalised device path.
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: Option<i64>,
    pub link_target: Option<String>,
}

impl FileEntry {
    /// Builds an entry from AFC `GetFileInfo` key/values.
    pub(crate) fn from_afc(path: &str, raw: &HashMap<String, String>) -> Result<Self> {
        let path = normalize_path(path);
        let kind = match raw.get("st_ifmt").map(String::as_str) {
            Some("S_IFREG") => FileKind::File,
            Some("S_IFDIR") => FileKind::Directory,
            Some("S_IFLNK") => FileKind::Symlink,
            Some(_) => FileKind::Other,
            None => {
                return Err(Error::Protocol(format!(
                    "AFC info for {path} has no st_ifmt"
                )));
            }
        };
        let size = raw
            .get("st_size")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Protocol(format!("AFC info for {path} has no st_size")))?;
        let modified = raw
            .get("st_mtime")
            .and_then(|s| s.parse::<i64>().ok())
            .map(|ns| ns.div_euclid(1_000_000_000));
        Ok(Self {
            name: file_name(&path).to_string(),
            kind,
            size,
            modified,
            link_target: raw.get("st_link_target").cloned(),
            path,
        })
    }

    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Directory
    }
}

/// Space on the file system an AFC session sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub model: Option<String>,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub block_size: u64,
}

/// Normalises a device path: absolute, no empty, `.` or `..` segments, no
/// trailing slash. `..` never climbs above `/`.
pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Joins `name` onto `dir`. An absolute `name` replaces `dir`.
pub fn join_path(dir: &str, name: &str) -> String {
    if name.starts_with('/') {
        normalize_path(name)
    } else {
        normalize_path(&format!("{dir}/{name}"))
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Every proper ancestor of a normalised path plus the path itself, root excluded.
fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        current.push('/');
        current.push_str(segment);
        out.push(current.clone());
    }
    out
}

/// Sibling temp path used while downloading so a partial file never has the final name.
fn partial_path(local: &Path) -> PathBuf {
    let mut name = local.file_name().unwrap_or_default().to_os_string();
    name.push(".orchard-part");
    local.with_file_name(name)
}

/// Deletes a local temp file on drop unless committed, so an error or a
/// cancelled future never leaves a partial download behind.
struct PartGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> PartGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for PartGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// An AFC session on the media partition or an app container.
#[derive(Debug)]
pub struct DeviceFiles {
    afc: AfcClient,
}

impl DeviceConnection {
    /// Opens an AFC session on `location`.
    pub async fn files(&self, location: FileLocation) -> Result<DeviceFiles> {
        let afc = match location {
            FileLocation::Media => self.service::<AfcClient>().await?,
            FileLocation::AppContainer(bundle_id) => self
                .service::<HouseArrestClient>()
                .await?
                .vend_container(bundle_id.as_str())
                .await
                .map_err(|e| device_error(&format!("container of {bundle_id}"), e))?,
            FileLocation::AppDocuments(bundle_id) => self
                .service::<HouseArrestClient>()
                .await?
                .vend_documents(bundle_id.as_str())
                .await
                .map_err(|e| device_error(&format!("documents of {bundle_id}"), e))?,
        };
        Ok(DeviceFiles { afc })
    }
}

impl DeviceFiles {
    /// Lists a directory with metadata, without `.` and `..`, sorted by name.
    /// Entries that vanish or cannot be read while listing are skipped.
    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        let dir = normalize_path(path);
        let names = self
            .afc
            .list_dir(dir.as_str())
            .await
            .map_err(|e| device_error(&dir, e))?;
        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            if name == "." || name == ".." {
                continue;
            }
            match self.stat(&join_path(&dir, &name)).await {
                Ok(entry) => entries.push(entry),
                Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    pub async fn stat(&mut self, path: &str) -> Result<FileEntry> {
        let path = normalize_path(path);
        let raw = self
            .afc
            .get_file_info_raw(path.as_str())
            .await
            .map_err(|e| device_error(&path, e))?;
        FileEntry::from_afc(&path, &raw)
    }

    /// Whether `path` exists.
    pub async fn exists(&mut self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Copies a device file to `local`, reporting `download` progress in bytes.
    /// The file appears at `local` only once complete. Returns the byte count.
    pub async fn download(
        &mut self,
        remote: &str,
        local: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<u64> {
        self.download_dyn(remote, local, &mut progress).await
    }

    pub(crate) async fn download_dyn(
        &mut self,
        remote: &str,
        local: &Path,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<u64> {
        let entry = self.stat(remote).await?;
        if entry.kind != FileKind::File {
            return Err(Error::InvalidInput(format!("{} is not a file", entry.path)));
        }
        let partial = partial_path(local);
        let guard = PartGuard::new(&partial);
        let n = self.copy_out(&entry, &partial, progress).await?;
        tokio::fs::rename(&partial, local).await?;
        guard.commit();
        Ok(n)
    }

    async fn copy_out(
        &mut self,
        entry: &FileEntry,
        local: &Path,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<u64> {
        let path = entry.path.as_str();
        let mut out = tokio::fs::File::create(local).await?;
        let mut fd = self
            .afc
            .open(path, AfcFopenMode::RdOnly)
            .await
            .map_err(|e| device_error(path, e))?;
        let total = Some(entry.size);
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        progress(Progress::new("download", 0, total));
        loop {
            let n = fd.read(&mut buf).await.map_err(|e| read_error(path, e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).await?;
            done += n as u64;
            progress(Progress::new("download", done, total));
        }
        fd.close().await.map_err(|e| device_error(path, e))?;
        out.flush().await?;
        Ok(done)
    }

    /// Copies `local` to a device path, reporting `upload` progress in bytes.
    /// The data goes to `<remote>.orchard-part` first and replaces `remote`
    /// only once complete.
    pub async fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<u64> {
        self.upload_dyn(local, remote, &mut progress).await
    }

    pub(crate) async fn upload_dyn(
        &mut self,
        local: &Path,
        remote: &str,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<u64> {
        let path = normalize_path(remote);
        let part = format!("{path}.orchard-part");
        let written = self.write_remote(local, &part, progress).await;
        let result = match written {
            Ok(n) => self.rename(&part, &path).await.map(|()| n),
            Err(e) => Err(e),
        };
        if result.is_err() {
            let _ = self.afc.remove(part.as_str()).await;
        }
        result
    }

    async fn write_remote(
        &mut self,
        local: &Path,
        path: &str,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<u64> {
        let mut input = tokio::fs::File::open(local).await?;
        let total = Some(input.metadata().await?.len());
        let mut fd = self
            .afc
            .open(path, AfcFopenMode::WrOnly)
            .await
            .map_err(|e| device_error(path, e))?;
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        progress(Progress::new("upload", 0, total));
        loop {
            let n = input.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            fd.write_all(&buf[..n])
                .await
                .map_err(|e| read_error(path, e))?;
            done += n as u64;
            progress(Progress::new("upload", done, total));
        }
        fd.flush().await.map_err(|e| read_error(path, e))?;
        fd.close().await.map_err(|e| device_error(path, e))?;
        Ok(done)
    }

    /// Creates `path` and any missing parents.
    pub async fn mkdir_all(&mut self, path: &str) -> Result<()> {
        for dir in ancestors(&normalize_path(path)) {
            if let Err(e) = self.afc.mk_dir(dir.as_str()).await {
                match self.stat(&dir).await {
                    Ok(entry) if entry.is_dir() => {}
                    _ => return Err(device_error(&dir, e)),
                }
            }
        }
        Ok(())
    }

    /// Removes a file or an empty directory, or a whole tree when `recursive`.
    pub async fn remove(&mut self, path: &str, recursive: bool) -> Result<()> {
        let path = normalize_path(path);
        if path == "/" {
            return Err(Error::InvalidInput("refusing to remove /".into()));
        }
        let result = if recursive {
            self.afc.remove_all(path.as_str()).await
        } else {
            self.afc.remove(path.as_str()).await
        };
        result.map_err(|e| device_error(&path, e))
    }

    pub async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (normalize_path(from), normalize_path(to));
        self.afc
            .rename(from.as_str(), to.as_str())
            .await
            .map_err(|e| device_error(&format!("rename {from} to {to}"), e))
    }

    /// Capacity of the file system behind this session.
    pub async fn storage(&mut self) -> Result<StorageInfo> {
        let info = self
            .afc
            .get_device_info()
            .await
            .map_err(|e| device_error("AFC device info", e))?;
        Ok(StorageInfo {
            model: Some(info.model).filter(|m| !m.is_empty()),
            total_bytes: info.total_bytes as u64,
            free_bytes: info.free_bytes as u64,
            block_size: info.block_size as u64,
        })
    }
}

/// AFC file handles surface failures as `io::Error`; keep not-found distinct.
fn read_error(path: &str, e: std::io::Error) -> Error {
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<IdeviceError>())
    {
        Some(IdeviceError::Afc(AfcError::ObjectNotFound)) => Error::NotFound(path.to_string()),
        _ => Error::Device(format!("{path}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("DCIM"), "/DCIM");
        assert_eq!(normalize_path("//DCIM///100APPLE/"), "/DCIM/100APPLE");
        assert_eq!(
            normalize_path("/DCIM/./100APPLE/../101APPLE"),
            "/DCIM/101APPLE"
        );
        assert_eq!(normalize_path("/../../etc"), "/etc");
    }

    #[test]
    fn joins_paths() {
        assert_eq!(join_path("/", "DCIM"), "/DCIM");
        assert_eq!(join_path("/DCIM/", "100APPLE"), "/DCIM/100APPLE");
        assert_eq!(join_path("/DCIM", "../Downloads/a.txt"), "/Downloads/a.txt");
        assert_eq!(join_path("/DCIM", "/PublicStaging"), "/PublicStaging");
    }

    #[test]
    fn lists_ancestors() {
        assert_eq!(ancestors("/"), Vec::<String>::new());
        assert_eq!(ancestors("/a/b/c"), vec!["/a", "/a/b", "/a/b/c"]);
    }

    #[test]
    fn partial_path_is_a_sibling() {
        assert_eq!(
            partial_path(Path::new("/tmp/out/IMG_0001.HEIC")),
            PathBuf::from("/tmp/out/IMG_0001.HEIC.orchard-part")
        );
    }

    fn raw(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_afc_file_info() {
        let file = FileEntry::from_afc(
            "/DCIM/100APPLE//IMG_0001.HEIC",
            &raw(&[
                ("st_size", "2481152"),
                ("st_blocks", "4848"),
                ("st_nlink", "1"),
                ("st_ifmt", "S_IFREG"),
                ("st_mtime", "1718000000123456789"),
                ("st_birthtime", "1718000000000000000"),
            ]),
        )
        .unwrap();
        assert_eq!(file.name, "IMG_0001.HEIC");
        assert_eq!(file.path, "/DCIM/100APPLE/IMG_0001.HEIC");
        assert_eq!(file.kind, FileKind::File);
        assert_eq!(file.size, 2_481_152);
        assert_eq!(file.modified, Some(1_718_000_000));

        let dir = FileEntry::from_afc(
            "/DCIM",
            &raw(&[
                ("st_size", "160"),
                ("st_ifmt", "S_IFDIR"),
                ("st_nlink", "5"),
            ]),
        )
        .unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.modified, None);

        let link = FileEntry::from_afc(
            "/var/link",
            &raw(&[
                ("st_size", "11"),
                ("st_ifmt", "S_IFLNK"),
                ("st_link_target", "/private/var"),
            ]),
        )
        .unwrap();
        assert_eq!(link.kind, FileKind::Symlink);
        assert_eq!(link.link_target.as_deref(), Some("/private/var"));
    }

    #[test]
    fn part_guard_removes_uncommitted_files() {
        let dir = tempfile::tempdir().unwrap();
        let dropped = dir.path().join("a.orchard-part");
        std::fs::write(&dropped, b"partial").unwrap();
        drop(PartGuard::new(&dropped));
        assert!(!dropped.exists());

        let kept = dir.path().join("b.orchard-part");
        std::fs::write(&kept, b"done").unwrap();
        PartGuard::new(&kept).commit();
        assert!(kept.exists());
    }

    #[test]
    fn rejects_incomplete_afc_info() {
        assert!(FileEntry::from_afc("/x", &raw(&[("st_size", "1")])).is_err());
        assert!(FileEntry::from_afc("/x", &raw(&[("st_ifmt", "S_IFREG")])).is_err());
    }

    #[test]
    fn location_serializes_with_bundle_id() {
        let json =
            serde_json::to_string(&FileLocation::AppDocuments("com.example.app".into())).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"appDocuments","bundleId":"com.example.app"}"#
        );
        assert_eq!(
            serde_json::to_string(&FileLocation::Media).unwrap(),
            r#"{"kind":"media"}"#
        );
    }
}
