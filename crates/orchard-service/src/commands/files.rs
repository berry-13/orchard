//! File browsing and transfers over AFC (media partition or app containers).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::Events;
use orchard_device::{DeviceFiles, FileLocation, join_path};

use crate::dto::{FileEntry, FileKind, FileSource};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::paths::safe_name;
use crate::state::DeviceState;

pub fn location(source: FileSource) -> FileLocation {
    match source {
        FileSource::Media => FileLocation::Media,
        FileSource::AppDocuments { bundle_id } => FileLocation::AppDocuments(bundle_id),
        FileSource::AppContainer { bundle_id } => FileLocation::AppContainer(bundle_id),
    }
}

pub fn to_entry(e: orchard_device::FileEntry) -> FileEntry {
    FileEntry {
        kind: match e.kind {
            orchard_device::FileKind::Directory => FileKind::Dir,
            orchard_device::FileKind::Symlink => FileKind::Symlink,
            orchard_device::FileKind::File | orchard_device::FileKind::Other => FileKind::File,
        },
        name: e.name,
        path: e.path,
        size: e.size,
        modified: e.modified.map(|s| s * 1000),
    }
}

async fn open(state: &DeviceState, udid: &str, source: FileSource) -> CmdResult<DeviceFiles> {
    Ok(state
        .connection(udid)
        .await?
        .files(location(source))
        .await?)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a transfer will do: files with their destination and size, folders to
/// create first, and how many symlinks or special files were left out.
#[derive(Debug, Default)]
struct Plan<S, D> {
    files: Vec<(S, D, u64)>,
    dirs: Vec<D>,
    skipped: u64,
}

impl<S, D> Plan<S, D> {
    fn total(&self) -> u64 {
        self.files.iter().map(|(_, _, size)| size).sum()
    }

    fn note(&self, rep: &crate::events::Reporter) {
        if self.skipped > 0 {
            rep.note(format!("Skipped {} links or special files", self.skipped));
        }
    }
}

/// Local files under `path` paired with their device destination under
/// `remote_dir`. Symlinks are skipped, which also rules out link loops.
fn local_plan(path: &Path, remote_dir: &str, plan: &mut Plan<PathBuf, String>) -> CmdResult<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let remote = join_path(remote_dir, &file_name(path));
    if meta.is_dir() {
        plan.dirs.push(remote.clone());
        let mut children: Vec<_> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        children.sort();
        for child in children {
            local_plan(&child, &remote, plan)?;
        }
    } else if meta.is_file() {
        plan.files.push((path.to_path_buf(), remote, meta.len()));
    } else {
        plan.skipped += 1;
    }
    Ok(())
}

/// `path` when it does not exist yet, otherwise the first free `name (n).ext`.
pub fn free_name(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Device files under `remote` paired with their local destination under
/// `local_dir`. Only regular files are queued; links and special files are
/// counted as skipped. Top level items never replace local files: an existing
/// name gets a ` (n)` suffix, and folders below it are new.
async fn remote_plan(
    files: &mut DeviceFiles,
    remote: &str,
    local_dir: &Path,
    top: bool,
    plan: &mut Plan<String, PathBuf>,
) -> CmdResult<()> {
    let entry = files.stat(remote).await?;
    let mut local = local_dir.join(safe_name(&entry.name)?);
    if top {
        local = free_name(&local);
    }
    match entry.kind {
        orchard_device::FileKind::Directory => {
            plan.dirs.push(local.clone());
            for child in files.list_dir(remote).await? {
                if child.name == "." || child.name == ".." {
                    continue;
                }
                Box::pin(remote_plan(files, &child.path, &local, false, plan)).await?;
            }
        }
        orchard_device::FileKind::File => plan.files.push((entry.path, local, entry.size)),
        orchard_device::FileKind::Symlink | orchard_device::FileKind::Other => plan.skipped += 1,
    }
    Ok(())
}

pub async fn list_files(
    state: &Arc<DeviceState>,
    udid: String,
    source: FileSource,
    path: String,
) -> CmdResult<Vec<FileEntry>> {
    let mut files = open(state, &udid, source).await?;
    Ok(files
        .list_dir(&path)
        .await?
        .into_iter()
        .map(to_entry)
        .collect())
}

/// Uploads files and folders into `dest_dir`. Task stage: `upload` (bytes).
pub async fn upload_files(
    app: Events,
    tasks: &TaskRegistry,
    state: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    source: FileSource,
    dest_dir: String,
    local_paths: Vec<String>,
) -> CmdResult<()> {
    let mut files = open(state, &udid, source).await?;
    run_task(&app, tasks, task_id, |rep| async move {
        let mut plan = Plan::default();
        for path in &local_paths {
            local_plan(Path::new(path), &dest_dir, &mut plan)?;
        }
        for dir in &plan.dirs {
            files.mkdir_all(dir).await?;
        }
        plan.note(&rep);
        let total = plan.total();
        let mut offset = 0;
        for (local, remote, size) in plan.files {
            files
                .upload(&local, &remote, rep.offset_callback(offset, total))
                .await?;
            offset += size;
        }
        Ok(())
    })
    .await
}

/// Downloads files and folders into `local_dir`. Task stage: `download` (bytes).
pub async fn download_files(
    app: Events,
    tasks: &TaskRegistry,
    state: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    source: FileSource,
    remote_paths: Vec<String>,
    local_dir: String,
) -> CmdResult<()> {
    let mut files = open(state, &udid, source).await?;
    run_task(&app, tasks, task_id, |rep| async move {
        let local_dir = PathBuf::from(local_dir);
        let mut plan = Plan::default();
        for remote in &remote_paths {
            remote_plan(&mut files, remote, &local_dir, true, &mut plan).await?;
        }
        for dir in &plan.dirs {
            tokio::fs::create_dir_all(dir).await?;
        }
        plan.note(&rep);
        let total = plan.total();
        let mut offset = 0;
        for (remote, local, size) in plan.files {
            files
                .download(&remote, &local, rep.offset_callback(offset, total))
                .await?;
            offset += size;
        }
        Ok(())
    })
    .await
}

pub async fn create_folder(
    state: &Arc<DeviceState>,
    udid: String,
    source: FileSource,
    path: String,
) -> CmdResult<()> {
    open(state, &udid, source).await?.mkdir_all(&path).await?;
    Ok(())
}

pub async fn rename_path(
    state: &Arc<DeviceState>,
    udid: String,
    source: FileSource,
    from: String,
    to: String,
) -> CmdResult<()> {
    open(state, &udid, source).await?.rename(&from, &to).await?;
    Ok(())
}

pub async fn delete_paths(
    state: &Arc<DeviceState>,
    udid: String,
    source: FileSource,
    paths: Vec<String>,
) -> CmdResult<()> {
    let mut files = open(state, &udid, source).await?;
    for path in paths {
        files.remove(&path, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_maps_to_location() {
        assert_eq!(location(FileSource::Media), FileLocation::Media);
        assert_eq!(
            location(FileSource::AppDocuments {
                bundle_id: "a.b".into()
            }),
            FileLocation::AppDocuments("a.b".into())
        );
    }

    #[test]
    fn modified_becomes_milliseconds() {
        let e = to_entry(orchard_device::FileEntry {
            name: "DCIM".into(),
            path: "/DCIM".into(),
            kind: orchard_device::FileKind::Directory,
            size: 0,
            modified: Some(10),
            link_target: None,
        });
        assert_eq!(e.kind, FileKind::Dir);
        assert_eq!(e.modified, Some(10_000));
    }

    #[test]
    fn local_plan_walks_folders_and_skips_links() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pics");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"de").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, dir.join("sub/loop")).unwrap();
        let mut plan = Plan::default();
        local_plan(&dir, "/Downloads", &mut plan).unwrap();
        assert_eq!(plan.dirs, ["/Downloads/pics", "/Downloads/pics/sub"]);
        let remotes: Vec<_> = plan
            .files
            .iter()
            .map(|(_, r, s)| (r.as_str(), *s))
            .collect();
        assert_eq!(
            remotes,
            [
                ("/Downloads/pics/a.txt", 3),
                ("/Downloads/pics/sub/b.txt", 2)
            ]
        );
        #[cfg(unix)]
        assert_eq!(plan.skipped, 1);
    }

    #[test]
    fn free_name_never_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("photo.jpg");
        assert_eq!(free_name(&a), a);
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(tmp.path().join("photo (1).jpg"), b"x").unwrap();
        assert_eq!(free_name(&a), tmp.path().join("photo (2).jpg"));
        std::fs::create_dir(tmp.path().join("DCIM")).unwrap();
        assert_eq!(
            free_name(&tmp.path().join("DCIM")),
            tmp.path().join("DCIM (1)")
        );
    }
}
