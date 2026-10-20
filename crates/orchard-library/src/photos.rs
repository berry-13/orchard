//! Camera roll listing and export from the media partition's `/DCIM` tree.

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use orchard_core::{Error, Progress, Result};
use orchard_device::{DeviceFiles, FileEntry, FileKind, join_path};
use serde::{Deserialize, Serialize};

const DCIM: &str = "/DCIM";
const PHOTO_EXTENSIONS: &[&str] = &[
    "heic", "heif", "jpg", "jpeg", "png", "dng", "gif", "tif", "tiff",
];
const VIDEO_EXTENSIONS: &[&str] = &["mov", "mp4", "m4v", "hevc"];

/// Where camera roll files come from. Implemented by [`DeviceFiles`]; tests use a fake.
pub trait MediaSource: Send {
    fn list_dir(&mut self, path: &str) -> impl Future<Output = Result<Vec<FileEntry>>> + Send;

    /// Copies `remote` to `local`, reporting bytes copied so far.
    fn download(
        &mut self,
        remote: &str,
        local: &Path,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> impl Future<Output = Result<u64>> + Send;
}

impl MediaSource for DeviceFiles {
    async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        DeviceFiles::list_dir(self, path).await
    }

    async fn download(
        &mut self,
        remote: &str,
        local: &Path,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<u64> {
        DeviceFiles::download(self, remote, local, |p| progress(p.done)).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Photo,
    Video,
}

/// One camera roll file, such as `/DCIM/100APPLE/IMG_0001.HEIC`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    /// Device path.
    pub path: String,
    /// DCIM folder, such as `100APPLE`.
    pub album: String,
    pub name: String,
    pub kind: MediaKind,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: Option<i64>,
}

impl PhotoItem {
    /// Where the item lands under an export folder: `<dest>/<album>/<name>`.
    ///
    /// Fails when the device-supplied album or name could escape `dest`.
    pub fn local_path(&self, dest: &Path) -> Result<PathBuf> {
        if !is_safe_component(&self.album) || !is_safe_component(&self.name) {
            return Err(Error::InvalidInput(format!(
                "unsafe camera roll path {:?} / {:?}",
                self.album, self.name
            )));
        }
        Ok(dest.join(&self.album).join(&self.name))
    }
}

/// Totals of an [`export_photos`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub exported: usize,
    pub skipped: usize,
    /// Bytes actually copied.
    pub bytes: u64,
    /// Exported files that got a numbered name because a different file
    /// already used the original one.
    pub renamed: Vec<PathBuf>,
    /// Items not exported because their album or name is not a plain file name.
    pub rejected: usize,
}

/// Whether a device-supplied name is exactly one plain path component on
/// every platform: not empty, `.` or `..`, and free of separators, drive
/// colons and NUL.
pub(crate) fn is_safe_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && matches!(components.next(), Some(Component::Normal(c)) if c == name)
        && components.next().is_none()
}

fn media_kind(name: &str) -> Option<MediaKind> {
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    if PHOTO_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Photo)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

/// DCIM folders are named with three digits then a suffix (`100APPLE`, `101CLOUD`).
fn is_album(entry: &FileEntry) -> bool {
    let name = entry.name.as_bytes();
    entry.kind == FileKind::Directory
        && is_safe_component(&entry.name)
        && name.len() > 3
        && name[..3].iter().all(u8::is_ascii_digit)
        && entry.name.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Lists photos and videos under `/DCIM`, ordered by folder then name. Edit
/// sidecars (`.AAE`), other files and names that are not a plain file name
/// (`..`, separators, colons, NUL) are left out. A device without `/DCIM`
/// yields an empty list.
pub async fn list_camera_roll<S: MediaSource>(source: &mut S) -> Result<Vec<PhotoItem>> {
    let albums = match source.list_dir(DCIM).await {
        Ok(entries) => entries,
        Err(Error::NotFound(_)) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut albums: Vec<FileEntry> = albums.into_iter().filter(is_album).collect();
    albums.sort_by(|a, b| a.name.cmp(&b.name));

    let mut items = Vec::new();
    for album in albums {
        let mut files = source.list_dir(&join_path(DCIM, &album.name)).await?;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        for file in files {
            if file.kind != FileKind::File || !is_safe_component(&file.name) {
                continue;
            }
            let Some(kind) = media_kind(&file.name) else {
                continue;
            };
            items.push(PhotoItem {
                path: file.path,
                album: album.name.clone(),
                name: file.name,
                kind,
                size: file.size,
                modified: file.modified,
            });
        }
    }
    Ok(items)
}

/// Most numbered alternatives tried before giving up on a name.
pub(crate) const MAX_SUFFIX: u32 = 10_000;

fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    let since = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_secs()).ok()
}

/// Whether `meta` describes an earlier export of `item`: same size and, when
/// the device reported one, same modification time.
fn is_copy_of(item: &PhotoItem, meta: &std::fs::Metadata) -> bool {
    meta.is_file()
        && meta.len() == item.size
        && item.modified.is_none_or(|t| mtime_secs(meta) == Some(t))
}

/// `IMG_0001.HEIC` becomes `IMG_0001 (n).HEIC`.
fn numbered(path: &Path, n: u32) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(numbered_name(&name, n))
}

/// `IMG_0001.HEIC` becomes `IMG_0001 (n).HEIC`; a name without extension gets ` (n)`.
pub(crate) fn numbered_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Picks where `item` goes: `None` when it was already exported, otherwise the
/// preferred path or the first free numbered variant. Never an existing file.
async fn target_for(item: &PhotoItem, preferred: &Path) -> Result<Option<PathBuf>> {
    for n in 0..=MAX_SUFFIX {
        let candidate = if n == 0 {
            preferred.to_path_buf()
        } else {
            numbered(preferred, n)
        };
        match tokio::fs::symlink_metadata(&candidate).await {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Some(candidate)),
            Err(e) => return Err(e.into()),
            Ok(meta) if is_copy_of(item, &meta) => return Ok(None),
            Ok(_) => {}
        }
    }
    Err(Error::InvalidInput(format!(
        "no free file name for {} in {}",
        item.name,
        preferred.parent().unwrap_or(preferred).display()
    )))
}

fn set_mtime(path: &Path, secs: i64) -> Result<()> {
    let Ok(secs) = u64::try_from(secs) else {
        return Ok(());
    };
    let file = std::fs::File::options().write(true).open(path)?;
    file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))?;
    Ok(())
}

/// Exports `items` to `<dest>/<album>/<name>`.
///
/// A file already there with the same size and modification time counts as
/// exported and is skipped. A different file using the name is never
/// replaced: the item goes to `<name> (1).<ext>`, `(2)`, and so on. Exported
/// files get the device modification time. Items whose album or name is not
/// a plain file name are counted in `rejected` and not written. Progress is stage `export` in
/// bytes over the size of all items, skipped ones included.
pub async fn export_photos<S: MediaSource>(
    source: &mut S,
    items: &[PhotoItem],
    dest: &Path,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<ExportSummary> {
    let total: u64 = items.iter().map(|i| i.size).sum();
    let mut done = 0u64;
    let mut summary = ExportSummary::default();
    progress(Progress::new("export", 0, Some(total)));
    for item in items {
        let Ok(preferred) = item.local_path(dest) else {
            summary.rejected += 1;
            done += item.size;
            progress(Progress::new("export", done, Some(total)));
            continue;
        };
        if let Some(parent) = preferred.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match target_for(item, &preferred).await? {
            None => summary.skipped += 1,
            Some(target) => {
                let base = done;
                let copied = source
                    .download(&item.path, &target, &mut |n| {
                        progress(Progress::new(
                            "export",
                            base + n.min(item.size),
                            Some(total),
                        ))
                    })
                    .await?;
                if let Some(secs) = item.modified {
                    set_mtime(&target, secs)?;
                }
                summary.exported += 1;
                summary.bytes += copied;
                if target != preferred {
                    summary.renamed.push(target);
                }
            }
        }
        done += item.size;
        progress(Progress::new("export", done, Some(total)));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use orchard_device::normalize_path;

    use super::*;

    #[derive(Default)]
    struct FakeSource {
        dirs: HashMap<String, Vec<FileEntry>>,
        files: HashMap<String, Vec<u8>>,
        downloads: Vec<String>,
    }

    impl FakeSource {
        fn dir(&mut self, path: &str) {
            self.dirs.entry(path.to_string()).or_default();
            if let Some((parent, name)) = path.rsplit_once('/') {
                let parent = if parent.is_empty() { "/" } else { parent };
                self.dirs.entry(parent.to_string()).or_default().push(entry(
                    path,
                    name,
                    FileKind::Directory,
                    0,
                ));
            }
        }

        fn file(&mut self, path: &str, bytes: &[u8]) {
            let (parent, name) = path.rsplit_once('/').unwrap();
            self.dirs.entry(parent.to_string()).or_default().push(entry(
                path,
                name,
                FileKind::File,
                bytes.len() as u64,
            ));
            self.files.insert(path.to_string(), bytes.to_vec());
        }
    }

    fn entry(path: &str, name: &str, kind: FileKind, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: normalize_path(path),
            kind,
            size,
            modified: Some(1_718_000_000),
            link_target: None,
        }
    }

    impl MediaSource for FakeSource {
        async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
            self.dirs
                .get(path)
                .cloned()
                .ok_or_else(|| Error::NotFound(path.to_string()))
        }

        async fn download(
            &mut self,
            remote: &str,
            local: &Path,
            progress: &mut (dyn FnMut(u64) + Send),
        ) -> Result<u64> {
            let bytes = self
                .files
                .get(remote)
                .ok_or_else(|| Error::NotFound(remote.into()))?;
            let half = bytes.len() / 2;
            progress(half as u64);
            tokio::fs::write(local, bytes).await?;
            progress(bytes.len() as u64);
            self.downloads.push(remote.to_string());
            Ok(bytes.len() as u64)
        }
    }

    fn camera_roll() -> FakeSource {
        let mut src = FakeSource::default();
        src.dir("/DCIM");
        src.dir("/DCIM/101APPLE");
        src.dir("/DCIM/100APPLE");
        src.dir("/DCIM/.MISC");
        src.file("/DCIM/100APPLE/IMG_0002.MOV", b"movie-bytes");
        src.file("/DCIM/100APPLE/IMG_0001.HEIC", b"heic");
        src.file("/DCIM/100APPLE/IMG_0001.AAE", b"<plist/>");
        src.file("/DCIM/101APPLE/IMG_1000.JPG", b"jpeg-data");
        src.file("/DCIM/.MISC/Info.plist", b"x");
        src.file("/DCIM/100APPLE/.DS_Store", b"x");
        src
    }

    #[tokio::test]
    async fn lists_camera_roll_in_order() {
        let mut src = camera_roll();
        let items = list_camera_roll(&mut src).await.unwrap();
        let paths: Vec<_> = items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/DCIM/100APPLE/IMG_0001.HEIC",
                "/DCIM/100APPLE/IMG_0002.MOV",
                "/DCIM/101APPLE/IMG_1000.JPG"
            ]
        );
        assert_eq!(items[0].album, "100APPLE");
        assert_eq!(items[0].kind, MediaKind::Photo);
        assert_eq!(items[1].kind, MediaKind::Video);
        assert_eq!(items[1].size, 11);
    }

    #[tokio::test]
    async fn missing_dcim_is_empty() {
        let mut src = FakeSource::default();
        assert!(list_camera_roll(&mut src).await.unwrap().is_empty());
    }

    #[test]
    fn classifies_extensions() {
        assert_eq!(media_kind("IMG_0001.heic"), Some(MediaKind::Photo));
        assert_eq!(media_kind("IMG_0001.DNG"), Some(MediaKind::Photo));
        assert_eq!(media_kind("IMG_0002.MP4"), Some(MediaKind::Video));
        assert_eq!(media_kind("IMG_0001.AAE"), None);
        assert_eq!(media_kind("README"), None);
    }

    const DEVICE_MTIME: i64 = 1_718_000_000;

    fn write_with_mtime(path: &Path, bytes: &[u8], secs: Option<i64>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        if let Some(secs) = secs {
            set_mtime(path, secs).unwrap();
        }
    }

    fn local_mtime(path: &Path) -> Option<i64> {
        mtime_secs(&std::fs::metadata(path).unwrap())
    }

    #[tokio::test]
    async fn exports_without_replacing_other_files() {
        let dest = tempfile::tempdir().unwrap();
        let mut src = camera_roll();
        let items = list_camera_roll(&mut src).await.unwrap();

        // Earlier export of the same photo: same size and mtime, skipped.
        let heic = items[0].local_path(dest.path()).unwrap();
        write_with_mtime(&heic, b"HEIC", Some(DEVICE_MTIME));
        // A different video with the same name and size: kept, item renamed.
        let mov = items[1].local_path(dest.path()).unwrap();
        write_with_mtime(&mov, b"other-movie", None);
        // A different photo with the same name: kept, item renamed.
        let jpg = items[2].local_path(dest.path()).unwrap();
        write_with_mtime(&jpg, b"another phone", Some(DEVICE_MTIME));

        let mut events = Vec::new();
        let summary = export_photos(&mut src, &items, dest.path(), |p| events.push(p))
            .await
            .unwrap();
        let mov_copy = dest.path().join("100APPLE/IMG_0002 (1).MOV");
        let jpg_copy = dest.path().join("101APPLE/IMG_1000 (1).JPG");
        assert_eq!(
            summary,
            ExportSummary {
                exported: 2,
                skipped: 1,
                bytes: 20,
                renamed: vec![mov_copy.clone(), jpg_copy.clone()],
                rejected: 0,
            }
        );
        assert_eq!(
            src.downloads,
            ["/DCIM/100APPLE/IMG_0002.MOV", "/DCIM/101APPLE/IMG_1000.JPG"]
        );
        assert_eq!(std::fs::read(&heic).unwrap(), b"HEIC");
        assert_eq!(std::fs::read(&mov).unwrap(), b"other-movie");
        assert_eq!(std::fs::read(&jpg).unwrap(), b"another phone");
        assert_eq!(std::fs::read(&mov_copy).unwrap(), b"movie-bytes");
        assert_eq!(std::fs::read(&jpg_copy).unwrap(), b"jpeg-data");
        assert_eq!(local_mtime(&mov_copy), Some(DEVICE_MTIME));
        assert_eq!(local_mtime(&jpg_copy), Some(DEVICE_MTIME));

        let total = 4 + 11 + 9;
        assert_eq!(
            events.first(),
            Some(&Progress::new("export", 0, Some(total)))
        );
        assert_eq!(
            events.last(),
            Some(&Progress::new("export", total, Some(total)))
        );
        assert!(events.windows(2).all(|w| w[0].done <= w[1].done));
        assert!(events.iter().all(|p| p.stage == "export"));

        // A second run recognises every earlier copy.
        src.downloads.clear();
        let again = export_photos(&mut src, &items, dest.path(), |_| {})
            .await
            .unwrap();
        assert_eq!(again.skipped, 3);
        assert!(src.downloads.is_empty());
    }

    #[tokio::test]
    async fn same_size_with_other_mtime_is_not_skipped() {
        let dest = tempfile::tempdir().unwrap();
        let mut src = camera_roll();
        let items = list_camera_roll(&mut src).await.unwrap();
        let heic = items[0].local_path(dest.path()).unwrap();
        write_with_mtime(&heic, b"HEIC", Some(DEVICE_MTIME - 60));
        let summary = export_photos(&mut src, &items[..1], dest.path(), |_| {})
            .await
            .unwrap();
        assert_eq!(summary.exported, 1);
        assert_eq!(
            summary.renamed,
            [dest.path().join("100APPLE/IMG_0001 (1).HEIC")]
        );
    }

    #[test]
    fn rejects_hostile_components() {
        for bad in [
            "",
            ".",
            "..",
            "../x.HEIC",
            "a/b.HEIC",
            "a\\b.HEIC",
            "C:x.HEIC",
            "/etc",
            "x\0.HEIC",
        ] {
            assert!(!is_safe_component(bad), "{bad:?}");
        }
        for good in ["IMG_0001.HEIC", "100APPLE", "..HEIC", "IMG 0001 (1).JPG"] {
            assert!(is_safe_component(good), "{good:?}");
        }
    }

    #[tokio::test]
    async fn hostile_device_names_are_skipped() {
        let mut src = FakeSource::default();
        src.dir("/DCIM");
        src.dir("/DCIM/100APPLE");
        src.file("/DCIM/100APPLE/IMG_0001.HEIC", b"ok");
        let hostile = [
            ("..", FileKind::Directory),
            ("..\\..\\evil.HEIC", FileKind::File),
            ("C:evil.JPG", FileKind::File),
            ("evil\0.JPG", FileKind::File),
        ];
        for (name, kind) in hostile {
            let dir = if kind == FileKind::Directory {
                "/DCIM"
            } else {
                "/DCIM/100APPLE"
            };
            src.dirs
                .get_mut(dir)
                .unwrap()
                .push(entry(&format!("{dir}/x"), name, kind, 3));
        }
        let items = list_camera_roll(&mut src).await.unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["IMG_0001.HEIC"]);

        // Items built elsewhere are checked again before writing.
        let dest = tempfile::tempdir().unwrap();
        let mut evil = items[0].clone();
        evil.album = "..".into();
        evil.name = "escaped.HEIC".into();
        let summary = export_photos(&mut src, &[evil], &dest.path().join("out"), |_| {})
            .await
            .unwrap();
        assert_eq!(summary.rejected, 1);
        assert_eq!(summary.exported, 0);
        assert!(!dest.path().join("escaped.HEIC").exists());
        assert!(src.downloads.is_empty());
    }

    #[test]
    fn numbers_file_names() {
        assert_eq!(
            numbered(Path::new("/x/IMG_0001.HEIC"), 2),
            PathBuf::from("/x/IMG_0001 (2).HEIC")
        );
        assert_eq!(
            numbered(Path::new("/x/README"), 1),
            PathBuf::from("/x/README (1)")
        );
        assert_eq!(
            numbered(Path::new("/x/.hidden"), 1),
            PathBuf::from("/x/.hidden (1)")
        );
    }

    #[test]
    fn only_numbered_folders_are_albums() {
        assert!(is_album(&entry(
            "/DCIM/100APPLE",
            "100APPLE",
            FileKind::Directory,
            0
        )));
        assert!(is_album(&entry(
            "/DCIM/102CLOUD",
            "102CLOUD",
            FileKind::Directory,
            0
        )));
        assert!(!is_album(&entry(
            "/DCIM/.MISC",
            ".MISC",
            FileKind::Directory,
            0
        )));
        assert!(!is_album(&entry(
            "/DCIM/100",
            "100",
            FileKind::Directory,
            0
        )));
        assert!(!is_album(&entry(
            "/DCIM/100APPLE",
            "100APPLE",
            FileKind::File,
            0
        )));
    }
}
