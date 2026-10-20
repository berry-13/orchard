//! "Send to app": copy local media into the Documents folder of apps with
//! file sharing enabled (VLC, Infuse, Documents, ...), as an alternative to
//! syncing the built-in Music, TV and Books apps.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};

use orchard_core::{Error, Progress, Result};
use orchard_device::{AppInfo, DeviceConnection, DeviceFiles, FileEntry, FileKind, join_path};
use serde::{Deserialize, Serialize};

use crate::photos::{MAX_SUFFIX, is_safe_component, numbered_name};

/// Root of an app's shared folder in a `FileLocation::AppDocuments` session.
const DOCUMENTS: &str = "/Documents";

/// Coarse content type of a file, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaCategory {
    Audio,
    Video,
    Book,
    /// Anything else, including documents.
    Other,
}

const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "flac", "wav", "aiff", "aif", "alac", "opus", "ogg",
];
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "avi", "webm"];
const BOOKS: &[&str] = &["epub", "pdf", "cbz", "cbr", "mobi"];
const ALL: &[MediaCategory] = &[
    MediaCategory::Audio,
    MediaCategory::Video,
    MediaCategory::Book,
    MediaCategory::Other,
];

/// Classifies a file name by extension, case-insensitively.
pub fn media_category(name: &str) -> MediaCategory {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return MediaCategory::Other;
    };
    if stem.is_empty() {
        return MediaCategory::Other;
    }
    let ext = ext.to_ascii_lowercase();
    if AUDIO.contains(&ext.as_str()) {
        MediaCategory::Audio
    } else if VIDEO.contains(&ext.as_str()) {
        MediaCategory::Video
    } else if BOOKS.contains(&ext.as_str()) {
        MediaCategory::Book
    } else {
        MediaCategory::Other
    }
}

/// Well-known file sharing apps and what they play or open.
const KNOWN_APPS: &[(&str, &[MediaCategory])] = &[
    (
        "org.videolan.vlc-ios",
        &[MediaCategory::Audio, MediaCategory::Video],
    ),
    ("com.firecore.infuse", &[MediaCategory::Video]),
    ("com.readdle.ReaddleDocsIPad", ALL),
    (
        "com.readdle.PDFExpert5",
        &[MediaCategory::Book, MediaCategory::Other],
    ),
];

/// An app that can receive files through its shared Documents folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTarget {
    pub bundle_id: String,
    pub name: String,
    /// What the app plausibly handles. Every category for apps not in the
    /// built-in table.
    pub accepts: Vec<MediaCategory>,
    /// Whether `accepts` comes from the built-in table rather than the
    /// "any file" fallback.
    pub recognized: bool,
}

/// File sharing apps among `apps`, tagged with the media they accept.
pub fn media_targets_from_apps(apps: &[AppInfo]) -> Vec<MediaTarget> {
    apps.iter()
        .filter(|app| app.file_sharing)
        .map(|app| {
            let known = KNOWN_APPS.iter().find(|(id, _)| *id == app.bundle_id);
            MediaTarget {
                bundle_id: app.bundle_id.clone(),
                name: app.name.clone(),
                accepts: known.map_or(ALL, |(_, kinds)| kinds).to_vec(),
                recognized: known.is_some(),
            }
        })
        .collect()
}

/// Installed user apps that accept files over file sharing.
pub async fn media_targets(conn: &DeviceConnection) -> Result<Vec<MediaTarget>> {
    Ok(media_targets_from_apps(&conn.list_apps(false).await?))
}

/// Device side of "send to app". Implemented by [`DeviceFiles`] opened on
/// `FileLocation::AppDocuments`; tests use a fake.
pub trait MediaSink: Send {
    fn list_dir(&mut self, path: &str) -> impl Future<Output = Result<Vec<FileEntry>>> + Send;

    /// Metadata for `path`, `None` when it does not exist.
    fn stat(&mut self, path: &str) -> impl Future<Output = Result<Option<FileEntry>>> + Send;

    fn mkdir_all(&mut self, path: &str) -> impl Future<Output = Result<()>> + Send;

    /// Copies `local` to `remote`, reporting bytes copied so far.
    fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> impl Future<Output = Result<u64>> + Send;
}

impl MediaSink for DeviceFiles {
    async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        DeviceFiles::list_dir(self, path).await
    }

    async fn stat(&mut self, path: &str) -> Result<Option<FileEntry>> {
        match DeviceFiles::stat(self, path).await {
            Ok(entry) => Ok(Some(entry)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn mkdir_all(&mut self, path: &str) -> Result<()> {
        DeviceFiles::mkdir_all(self, path).await
    }

    async fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<u64> {
        DeviceFiles::upload(self, local, remote, |p| progress(p.done)).await
    }
}

/// A device file with its category; `category` is `None` for folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    #[serde(flatten)]
    pub entry: FileEntry,
    pub category: Option<MediaCategory>,
}

/// Lists one device folder (for example `/Documents`) with categories.
pub async fn list_media<S: MediaSink>(sink: &mut S, dir: &str) -> Result<Vec<MediaFile>> {
    Ok(sink
        .list_dir(dir)
        .await?
        .into_iter()
        .map(|entry| MediaFile {
            category: (entry.kind == FileKind::File).then(|| media_category(&entry.name)),
            entry,
        })
        .collect())
}

/// Totals of a [`send_media`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendSummary {
    pub uploaded: usize,
    /// Files already on the device with the same size.
    pub skipped: usize,
    /// Bytes actually copied.
    pub bytes: u64,
    /// Device paths of uploads that got a numbered name because a different
    /// file already used the original one.
    pub renamed: Vec<String>,
    /// Local symlinks, which are never followed.
    pub skipped_symlinks: usize,
    /// Local entries whose name is not valid UTF-8 or not a plain file name.
    pub skipped_invalid: usize,
}

/// A local file to send and where it goes relative to the destination root.
#[derive(Debug, PartialEq)]
struct Planned {
    local: PathBuf,
    rel_dir: Vec<String>,
    name: String,
    size: u64,
}

/// `/Documents` plus an optional subfolder, rejecting anything that could
/// leave the Documents folder.
fn destination_root(subdir: Option<&str>) -> Result<String> {
    let mut root = DOCUMENTS.to_string();
    for part in subdir.unwrap_or("").split('/').filter(|p| !p.is_empty()) {
        if !is_safe_component(part) {
            return Err(Error::InvalidInput(format!(
                "invalid destination folder {subdir:?}"
            )));
        }
        root = join_path(&root, part);
    }
    Ok(root)
}

/// Expands files and folders (recursively, in name order) into the files to
/// send. Symlinks are skipped and counted, never followed.
async fn plan(local_paths: &[PathBuf], summary: &mut SendSummary) -> Result<Vec<Planned>> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, Vec<String>)> = local_paths
        .iter()
        .rev()
        .map(|p| (p.clone(), Vec::new()))
        .collect();
    while let Some((path, rel_dir)) = stack.pop() {
        let meta = tokio::fs::symlink_metadata(&path).await?;
        if meta.file_type().is_symlink() {
            summary.skipped_symlinks += 1;
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| is_safe_component(n))
        else {
            summary.skipped_invalid += 1;
            continue;
        };
        if meta.is_dir() {
            let mut children = Vec::new();
            let mut entries = tokio::fs::read_dir(&path).await?;
            while let Some(entry) = entries.next_entry().await? {
                children.push(entry.path());
            }
            children.sort();
            let mut child_dir = rel_dir.clone();
            child_dir.push(name.to_string());
            stack.extend(children.into_iter().rev().map(|c| (c, child_dir.clone())));
        } else if meta.is_file() {
            out.push(Planned {
                name: name.to_string(),
                local: path.clone(),
                rel_dir,
                size: meta.len(),
            });
        }
    }
    Ok(out)
}

/// Where a file of `size` bytes named `name` goes in `dir`: `None` when a
/// same-size file is already there, otherwise the first free name.
async fn free_target<S: MediaSink>(
    sink: &mut S,
    dir: &str,
    name: &str,
    size: u64,
) -> Result<Option<String>> {
    for n in 0..=MAX_SUFFIX {
        let candidate = if n == 0 {
            name.to_string()
        } else {
            numbered_name(name, n)
        };
        let path = join_path(dir, &candidate);
        match sink.stat(&path).await? {
            None => return Ok(Some(path)),
            Some(e) if e.kind == FileKind::File && e.size == size => return Ok(None),
            Some(_) => {}
        }
    }
    Err(Error::InvalidInput(format!(
        "no free name for {name} in {dir}"
    )))
}

/// Uploads files and folders into the app's Documents folder (optionally
/// under `dest_subdir`).
///
/// Folders are copied recursively and local symlinks are skipped. A device
/// file with the same name and size counts as already sent; a different file
/// with the same name is kept and the upload gets `name (1).ext`, `(2)`, ...
/// Progress is stage `send` in bytes over all files, skipped ones included.
pub async fn send_media<S: MediaSink>(
    sink: &mut S,
    local_paths: &[PathBuf],
    dest_subdir: Option<&str>,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<SendSummary> {
    let root = destination_root(dest_subdir)?;
    let mut summary = SendSummary::default();
    let files = plan(local_paths, &mut summary).await?;
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    let mut created = HashSet::new();
    progress(Progress::new("send", 0, Some(total)));
    for file in &files {
        let dir = file
            .rel_dir
            .iter()
            .fold(root.clone(), |dir, part| join_path(&dir, part));
        if created.insert(dir.clone()) {
            sink.mkdir_all(&dir).await?;
        }
        if let Some(remote) = free_target(sink, &dir, &file.name, file.size).await? {
            let base = done;
            let copied = sink
                .upload(&file.local, &remote, &mut |n| {
                    progress(Progress::new("send", base + n.min(file.size), Some(total)))
                })
                .await?;
            summary.uploaded += 1;
            summary.bytes += copied;
            if remote != join_path(&dir, &file.name) {
                summary.renamed.push(remote);
            }
        } else {
            summary.skipped += 1;
        }
        done += file.size;
        progress(Progress::new("send", done, Some(total)));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use orchard_device::{AppKind, normalize_path};

    use super::*;

    #[derive(Default)]
    struct FakeSink {
        files: BTreeMap<String, Vec<u8>>,
        dirs: BTreeSet<String>,
        uploads: Vec<String>,
    }

    impl FakeSink {
        fn with_file(mut self, path: &str, bytes: &[u8]) -> Self {
            let mut dir = path.rsplit_once('/').unwrap().0.to_string();
            while !dir.is_empty() {
                self.dirs.insert(dir.clone());
                dir = dir.rsplit_once('/').unwrap().0.to_string();
            }
            self.files.insert(path.to_string(), bytes.to_vec());
            self
        }
    }

    fn entry(path: &str, kind: FileKind, size: u64) -> FileEntry {
        FileEntry {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            kind,
            size,
            modified: None,
            link_target: None,
        }
    }

    impl MediaSink for FakeSink {
        async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
            let parent = |p: &str| p.rsplit_once('/').unwrap().0.to_string();
            let mut out: Vec<FileEntry> = self
                .dirs
                .iter()
                .filter(|d| parent(d) == path)
                .map(|d| entry(d, FileKind::Directory, 0))
                .collect();
            out.extend(
                self.files
                    .iter()
                    .filter(|(f, _)| parent(f) == path)
                    .map(|(f, b)| entry(f, FileKind::File, b.len() as u64)),
            );
            Ok(out)
        }

        async fn stat(&mut self, path: &str) -> Result<Option<FileEntry>> {
            if self.dirs.contains(path) {
                return Ok(Some(entry(path, FileKind::Directory, 0)));
            }
            Ok(self
                .files
                .get(path)
                .map(|b| entry(path, FileKind::File, b.len() as u64)))
        }

        async fn mkdir_all(&mut self, path: &str) -> Result<()> {
            let mut current = String::new();
            for part in normalize_path(path).split('/').filter(|p| !p.is_empty()) {
                current = format!("{current}/{part}");
                if self.files.contains_key(&current) {
                    return Err(Error::Device(format!("{current} is a file")));
                }
                self.dirs.insert(current.clone());
            }
            Ok(())
        }

        async fn upload(
            &mut self,
            local: &Path,
            remote: &str,
            progress: &mut (dyn FnMut(u64) + Send),
        ) -> Result<u64> {
            let bytes = tokio::fs::read(local).await?;
            progress(bytes.len() as u64);
            self.uploads.push(remote.to_string());
            let n = bytes.len() as u64;
            self.files.insert(remote.to_string(), bytes);
            Ok(n)
        }
    }

    #[test]
    fn classifies_by_extension() {
        assert_eq!(media_category("Song.MP3"), MediaCategory::Audio);
        assert_eq!(media_category("a.opus"), MediaCategory::Audio);
        assert_eq!(media_category("clip.mkv"), MediaCategory::Video);
        assert_eq!(media_category("book.epub"), MediaCategory::Book);
        assert_eq!(media_category("scan.PDF"), MediaCategory::Book);
        assert_eq!(media_category("notes.txt"), MediaCategory::Other);
        assert_eq!(media_category("README"), MediaCategory::Other);
        assert_eq!(media_category(".mp3"), MediaCategory::Other);
    }

    fn app(bundle_id: &str, name: &str, file_sharing: bool) -> AppInfo {
        AppInfo {
            bundle_id: bundle_id.into(),
            name: name.into(),
            version: None,
            short_version: None,
            kind: AppKind::User,
            static_size: None,
            dynamic_size: None,
            path: None,
            container: None,
            file_sharing,
        }
    }

    #[test]
    fn tags_file_sharing_apps() {
        let apps = [
            app("org.videolan.vlc-ios", "VLC", true),
            app("com.example.reader", "Reader", true),
            app("com.example.game", "Game", false),
        ];
        let targets = media_targets_from_apps(&apps);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].bundle_id, "org.videolan.vlc-ios");
        assert_eq!(
            targets[0].accepts,
            [MediaCategory::Audio, MediaCategory::Video]
        );
        assert!(targets[0].recognized);
        assert_eq!(targets[1].accepts, ALL);
        assert!(!targets[1].recognized);
    }

    #[test]
    fn destination_stays_inside_documents() {
        assert_eq!(destination_root(None).unwrap(), "/Documents");
        assert_eq!(
            destination_root(Some("/Music//Live/")).unwrap(),
            "/Documents/Music/Live"
        );
        for bad in ["..", "Music/../..", "a\\b", "C:x"] {
            assert_eq!(
                destination_root(Some(bad)).unwrap_err().kind(),
                "invalidInput"
            );
        }
    }

    #[tokio::test]
    async fn sends_files_and_folders_without_overwriting() {
        let local = tempfile::tempdir().unwrap();
        let root = local.path();
        std::fs::write(root.join("song.mp3"), b"new song").unwrap();
        std::fs::create_dir_all(root.join("Movies/Extras")).unwrap();
        std::fs::write(root.join("Movies/a.mp4"), b"aaaa").unwrap();
        std::fs::write(root.join("Movies/Extras/b.mkv"), b"bb").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("song.mp3"), root.join("Movies/link.mp3")).unwrap();

        let mut sink = FakeSink::default()
            .with_file("/Documents/song.mp3", b"older")
            .with_file("/Documents/Movies/a.mp4", b"AAAA");
        let mut events = Vec::new();
        let summary = send_media(
            &mut sink,
            &[root.join("song.mp3"), root.join("Movies")],
            None,
            |p| events.push(p),
        )
        .await
        .unwrap();

        assert_eq!(
            sink.uploads,
            ["/Documents/song (1).mp3", "/Documents/Movies/Extras/b.mkv"]
        );
        assert_eq!(sink.files["/Documents/song.mp3"], b"older");
        assert_eq!(sink.files["/Documents/song (1).mp3"], b"new song");
        assert_eq!(summary.uploaded, 2);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.bytes, 10);
        assert_eq!(summary.renamed, ["/Documents/song (1).mp3"]);
        #[cfg(unix)]
        assert_eq!(summary.skipped_symlinks, 1);
        assert!(sink.dirs.contains("/Documents/Movies/Extras"));

        let total = 8 + 4 + 2;
        assert_eq!(events.first(), Some(&Progress::new("send", 0, Some(total))));
        assert_eq!(
            events.last(),
            Some(&Progress::new("send", total, Some(total)))
        );
        assert!(events.windows(2).all(|w| w[0].done <= w[1].done));

        // Everything is on the device now, so a second run sends nothing.
        sink.uploads.clear();
        let again = send_media(
            &mut sink,
            &[root.join("song.mp3"), root.join("Movies")],
            None,
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(again.uploaded, 0);
        assert_eq!(again.skipped, 3);
        assert!(sink.uploads.is_empty());
    }

    #[tokio::test]
    async fn sends_into_a_subfolder() {
        let local = tempfile::tempdir().unwrap();
        let file = local.path().join("talk.m4a");
        std::fs::write(&file, b"x").unwrap();
        let mut sink = FakeSink::default();
        send_media(&mut sink, &[file], Some("Podcasts"), |_| {})
            .await
            .unwrap();
        assert_eq!(sink.uploads, ["/Documents/Podcasts/talk.m4a"]);
    }

    #[tokio::test]
    async fn plans_recursively_in_name_order() {
        let local = tempfile::tempdir().unwrap();
        let root = local.path().join("Album");
        std::fs::create_dir_all(root.join("CD2")).unwrap();
        std::fs::write(root.join("02.flac"), b"2").unwrap();
        std::fs::write(root.join("01.flac"), b"1").unwrap();
        std::fs::write(root.join("CD2/01.flac"), b"3").unwrap();
        let mut summary = SendSummary::default();
        let planned = plan(&[root], &mut summary).await.unwrap();
        let names: Vec<_> = planned
            .iter()
            .map(|p| format!("{}/{}", p.rel_dir.join("/"), p.name))
            .collect();
        assert_eq!(
            names,
            ["Album/01.flac", "Album/02.flac", "Album/CD2/01.flac"]
        );
    }

    #[tokio::test]
    async fn lists_device_media_with_categories() {
        let mut sink = FakeSink::default()
            .with_file("/Documents/a.mp3", b"1")
            .with_file("/Documents/Shows/e1.mkv", b"2")
            .with_file("/Documents/b.pdf", b"3");
        let media = list_media(&mut sink, "/Documents").await.unwrap();
        let summary: Vec<_> = media
            .iter()
            .map(|m| (m.entry.name.as_str(), m.category))
            .collect();
        assert_eq!(
            summary,
            [
                ("Shows", None),
                ("a.mp3", Some(MediaCategory::Audio)),
                ("b.pdf", Some(MediaCategory::Book)),
            ]
        );
        let json = serde_json::to_value(&media[1]).unwrap();
        assert_eq!(json["name"], "a.mp3");
        assert_eq!(json["category"], "audio");
    }
}
