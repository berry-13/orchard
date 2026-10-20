//! Moving files between a remote client and the server: uploads land in a
//! private folder and become server paths the commands accept; results are
//! written to an exports folder and fetched with single-use download tickets
//! (a browser download cannot carry the bearer token).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use futures_util::{Stream, StreamExt};
use orchard_core::{Error, Result};
use tokio::io::AsyncWriteExt;

/// Largest accepted upload; an IPA rarely passes a few GB.
pub const MAX_UPLOAD: u64 = 16 * 1024 * 1024 * 1024;
const TICKET_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Uploads and generated zips older than this are removed at start.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Files {
    uploads: PathBuf,
    exports: PathBuf,
    zips: PathBuf,
    tickets: Mutex<HashMap<String, (PathBuf, Instant)>>,
}

fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn remove_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if old {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
}

impl Files {
    /// Uses `dir/uploads`, `dir/exports` and `dir/zips`.
    pub fn new(dir: &Path) -> Result<Self> {
        let files = Self {
            uploads: dir.join("uploads"),
            exports: dir.join("exports"),
            zips: dir.join("zips"),
            tickets: Mutex::default(),
        };
        for d in [&files.uploads, &files.exports, &files.zips] {
            std::fs::create_dir_all(d)?;
        }
        remove_stale(&files.uploads);
        remove_stale(&files.zips);
        Ok(files)
    }

    /// Writes an uploaded file as `name` in a fresh folder and returns its path.
    pub async fn upload<S, B, E>(&self, name: &str, mut body: S) -> Result<PathBuf>
    where
        S: Stream<Item = std::result::Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        let name =
            orchard_service::paths::safe_name(name).map_err(|e| Error::InvalidInput(e.message))?;
        let dir = self.uploads.join(random_id()?);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(name);
        let mut file = tokio::fs::File::create(&path).await?;
        let mut written = 0u64;
        let result: Result<()> = async {
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| Error::Network(e.to_string()))?;
                written += chunk.as_ref().len() as u64;
                if written > MAX_UPLOAD {
                    return Err(Error::InvalidInput("the file is too large".into()));
                }
                file.write_all(chunk.as_ref()).await?;
            }
            file.flush().await?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(e);
        }
        Ok(path)
    }

    /// A new empty folder for a command to write results into.
    pub fn new_export_dir(&self) -> Result<PathBuf> {
        let secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let dir = self.exports.join(format!("{secs}-{}", &random_id()?[..6]));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// A ticket for downloading `path`, which must be inside the exports
    /// folder or `download_dir` (the download folder set in Settings).
    pub fn ticket(&self, path: &Path, download_dir: Option<&Path>) -> Result<String> {
        let path = path
            .canonicalize()
            .map_err(|_| Error::NotFound(path.display().to_string()))?;
        let allowed = std::iter::once(self.exports.as_path())
            .chain(download_dir)
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| path.starts_with(root));
        if !allowed {
            return Err(Error::InvalidInput(
                "only exported files and the download folder can be downloaded".into(),
            ));
        }
        let ticket = random_id()?;
        let mut tickets = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        tickets.retain(|_, (_, at)| at.elapsed() < TICKET_LIFETIME);
        tickets.insert(ticket.clone(), (path, Instant::now()));
        Ok(ticket)
    }

    /// Redeems a ticket once.
    pub fn take(&self, ticket: &str) -> Option<PathBuf> {
        let mut tickets = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        match tickets.remove(ticket) {
            Some((path, at)) if at.elapsed() < TICKET_LIFETIME => Some(path),
            _ => None,
        }
    }

    /// Zips a folder for download and returns the archive path.
    pub fn zip_dir(&self, dir: &Path) -> Result<PathBuf> {
        let name = dir
            .file_name()
            .map_or_else(|| "orchard".into(), |n| n.to_string_lossy().into_owned());
        let path = self.zips.join(format!("{}-{name}.zip", random_id()?));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path)?);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                let full = entry.path();
                let rel = full
                    .strip_prefix(dir)
                    .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?
                    .to_string_lossy()
                    .replace('\\', "/");
                if kind.is_dir() {
                    stack.push(full);
                } else if kind.is_file() {
                    zip.start_file(rel, options)
                        .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
                    std::io::copy(&mut std::fs::File::open(&full)?, &mut zip)?;
                }
                // Symlinks are skipped so a link cannot pull in outside files.
            }
        }
        zip.finish()
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?
            .flush()?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(
        data: &[&'static [u8]],
    ) -> impl Stream<Item = std::result::Result<&'static [u8], String>> + Unpin {
        futures_util::stream::iter(data.iter().map(|c| Ok(*c)).collect::<Vec<_>>())
    }

    #[tokio::test]
    async fn uploads_land_in_their_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path()).unwrap();
        let path = files
            .upload("App.ipa", chunks(&[b"PK", b"\x03\x04"]))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"PK\x03\x04");
        assert!(path.starts_with(dir.path().join("uploads")));
        assert!(files.upload("../escape", chunks(&[b"x"])).await.is_err());
    }

    #[test]
    fn tickets_only_cover_exports_and_work_once() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path()).unwrap();
        let export = files.new_export_dir().unwrap();
        std::fs::write(export.join("a.txt"), "a").unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, "s").unwrap();

        assert!(files.ticket(&outside, None).is_err());
        assert!(
            files
                .ticket(&export.join("../../secret.txt"), None)
                .is_err()
        );
        assert!(files.ticket(&outside, Some(dir.path())).is_ok());

        let ticket = files.ticket(&export.join("a.txt"), None).unwrap();
        assert_eq!(
            files.take(&ticket),
            Some(export.join("a.txt").canonicalize().unwrap())
        );
        assert_eq!(files.take(&ticket), None);
    }

    #[test]
    fn zips_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path()).unwrap();
        let export = files.new_export_dir().unwrap();
        std::fs::create_dir(export.join("sub")).unwrap();
        std::fs::write(export.join("sub/b.txt"), "b").unwrap();
        let zip = files.zip_dir(&export).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
        assert_eq!(archive.by_name("sub/b.txt").unwrap().size(), 1);
    }
}
