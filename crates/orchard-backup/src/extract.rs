use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use orchard_core::{Error, Progress, Result};
use serde::{Deserialize, Serialize};

use crate::browse::{Backup, FileEntry, FileKind, FileQuery};

/// What [`Backup::extract`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractSummary {
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
    /// Symlinks are not recreated, only counted.
    pub skipped_links: u64,
    /// Entries whose blob is missing from the backup folder; nothing is written for them.
    pub missing: u64,
    /// Files written under a free name like `name (1).ext` because the target existed.
    pub renamed: u64,
}

impl Backup {
    /// Writes the content of `entry` to the file `dest`, creating parent folders and
    /// restoring the modification time. An existing `dest` is replaced only once the new
    /// content is complete, and is left untouched when the blob is missing.
    pub fn extract_file(&self, entry: &FileEntry, dest: &Path) -> Result<u64> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        self.write_entry(entry, dest, &mut |_| {})
    }

    /// Extracts `prefix` (a file or folder, `""` for the whole domain) of `domain` into
    /// `dest_dir`, keeping paths relative to the domain root.
    ///
    /// Fails with [`Error::InvalidInput`] before writing anything when an entry path would
    /// escape `dest_dir`. Existing files are never overwritten: a file whose target exists
    /// is written as `name (1).ext` instead. Progress reports bytes under the `extract` stage.
    pub fn extract(
        &self,
        domain: &str,
        prefix: &str,
        dest_dir: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<ExtractSummary> {
        let entries = self.files(&FileQuery {
            domain: Some(domain.to_owned()),
            prefix: Some(prefix.to_owned()),
            ..FileQuery::default()
        })?;
        if entries.is_empty() {
            return Err(Error::NotFound(format!("{domain}/{prefix}")));
        }
        let targets = entries
            .iter()
            .map(|entry| safe_join(dest_dir, &entry.relative_path))
            .collect::<Result<Vec<_>>>()?;

        let total: u64 = entries
            .iter()
            .filter(|e| e.kind == FileKind::File)
            .map(|e| e.size)
            .sum();
        let mut summary = ExtractSummary::default();
        progress(Progress::new("extract", 0, Some(total)));
        fs::create_dir_all(dest_dir)?;

        for (entry, target) in entries.iter().zip(&targets) {
            match entry.kind {
                FileKind::Directory => {
                    fs::create_dir_all(target)?;
                    summary.directories += 1;
                }
                FileKind::Symlink => summary.skipped_links += 1,
                FileKind::File => {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let base = summary.bytes;
                    let mut report =
                        |n: u64| progress(Progress::new("extract", base + n, Some(total)));
                    let free = free_path(target);
                    match self.write_entry(entry, &free, &mut report) {
                        Ok(written) => {
                            summary.files += 1;
                            summary.bytes += written;
                            if free != *target {
                                summary.renamed += 1;
                            }
                        }
                        Err(Error::NotFound(_)) => summary.missing += 1,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        progress(Progress::new("extract", summary.bytes, Some(total)));
        Ok(summary)
    }

    fn write_entry(
        &self,
        entry: &FileEntry,
        dest: &Path,
        on_bytes: &mut dyn FnMut(u64),
    ) -> Result<u64> {
        let mut part_name = dest.file_name().unwrap_or_default().to_os_string();
        part_name.push(".orchard-part");
        let part = dest.with_file_name(part_name);
        let result = (|| {
            let mut writer = BufWriter::new(File::create(&part)?);
            let written = self.copy_contents(entry, &mut writer, on_bytes)?;
            let file = writer.into_inner().map_err(|e| Error::Io(e.into_error()))?;
            if let Some(secs) = entry.modified.filter(|s| *s >= 0) {
                file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64))?;
            }
            drop(file);
            fs::rename(&part, dest)?;
            Ok(written)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }
}

/// `path` when nothing exists there, otherwise the first free `name (n).ext` next to it.
fn free_path(path: &Path) -> PathBuf {
    if fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1u32..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Joins a backup relative path onto `base`, refusing anything that could leave it.
pub(crate) fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = base.to_path_buf();
    for part in relative.split('/').filter(|p| !p.is_empty()) {
        let unsafe_part = part == "."
            || part == ".."
            || part.contains(['\\', ':', '\0'])
            || Path::new(part).components().count() != 1;
        if unsafe_part {
            return Err(Error::InvalidInput(format!(
                "refusing to extract unsafe path {relative:?}"
            )));
        }
        path.push(part);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_accepts_nested_paths() {
        let base = Path::new("out");
        assert_eq!(
            safe_join(base, "Library/Preferences/a.plist").unwrap(),
            base.join("Library").join("Preferences").join("a.plist")
        );
        assert_eq!(safe_join(base, "").unwrap(), base);
    }

    #[test]
    fn free_path_picks_unused_name() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.txt");
        assert_eq!(free_path(&target), target);
        std::fs::write(&target, b"x").unwrap();
        std::fs::write(dir.path().join("a (1).txt"), b"x").unwrap();
        assert_eq!(free_path(&target), dir.path().join("a (2).txt"));
        let bare = dir.path().join("README");
        std::fs::write(&bare, b"x").unwrap();
        assert_eq!(free_path(&bare), dir.path().join("README (1)"));
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let base = Path::new("out");
        for bad in ["../x", "a/../../x", "a/./b", "C:x", "a\\..\\b", "a/\0"] {
            assert!(
                matches!(safe_join(base, bad), Err(Error::InvalidInput(_))),
                "{bad}"
            );
        }
    }
}
