//! Partial download cache: `<key>.part` holds the bytes received so far and
//! `<key>.json` records where they came from, so a later download resumes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};
use orchard_core::Result;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// Partial downloads untouched for longer than this are pruned.
pub const PARTIAL_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What a partial file belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sidecar {
    pub url: String,
    /// Full size of the IPA, once a response told us.
    pub size: Option<u64>,
    pub md5: Option<String>,
}

pub(crate) struct Partial {
    pub data: PathBuf,
    sidecar: PathBuf,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Partial {
    /// Keyed by app, version and content: the MD5 when Apple sends one,
    /// otherwise a hash of the URL.
    pub fn new(
        dir: &Path,
        app_id: u64,
        version: Option<&str>,
        md5: Option<&str>,
        url: &str,
    ) -> Self {
        let content = match md5 {
            Some(md5) => md5.trim().to_ascii_lowercase(),
            None => hex(&Md5::digest(url.as_bytes())),
        };
        let version: String = version
            .unwrap_or("latest")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let key = format!("{app_id}_{version}_{content}");
        Self {
            data: dir.join(format!("{key}.part")),
            sidecar: dir.join(format!("{key}.json")),
        }
    }

    pub async fn sidecar(&self) -> Option<Sidecar> {
        let data = tokio::fs::read(&self.sidecar).await.ok()?;
        serde_json::from_slice(&data).ok()
    }

    pub async fn write_sidecar(&self, sidecar: &Sidecar) -> Result<()> {
        let data = serde_json::to_vec(sidecar).map_err(std::io::Error::other)?;
        tokio::fs::write(&self.sidecar, data).await?;
        Ok(())
    }

    /// Bytes already on disk.
    pub async fn len(&self) -> u64 {
        tokio::fs::metadata(&self.data)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub async fn remove(&self) {
        let _ = tokio::fs::remove_file(&self.data).await;
        let _ = tokio::fs::remove_file(&self.sidecar).await;
    }

    /// Feeds the bytes already on disk into `hasher`.
    pub async fn hash_existing(&self, hasher: &mut Md5) -> Result<()> {
        let mut file = tokio::fs::File::open(&self.data).await?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            hasher.update(&buf[..n]);
        }
    }
}

/// Removes partial downloads in `dir` not modified for `max_age`
/// (use [`PARTIAL_MAX_AGE`] for the default week). Returns how many files
/// were removed. A missing directory is not an error.
pub fn prune_partial_downloads(dir: &Path, max_age: Duration) -> Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("part" | "json")
        );
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > max_age);
        if ours && stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
