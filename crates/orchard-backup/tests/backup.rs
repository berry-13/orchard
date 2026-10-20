//! End to end tests against synthetic backups, plain and encrypted.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use aes::Aes256;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncryptMut, KeyIvInit};
use orchard_backup::{Backup, FileKind, FileQuery, list_backups, read_summary};
use orchard_core::Error;
use plist::{Dictionary, Uid, Value};
use sha1::{Digest, Sha1};

const UDID: &str = "00008110-000A1B2C3D4E5F60";
const PASSWORD: &str = "hunter2";

enum Kind {
    File(Vec<u8>),
    Dir,
    Link(&'static str),
}

struct Item {
    domain: &'static str,
    path: &'static str,
    kind: Kind,
    mtime: i64,
}

fn file(domain: &'static str, path: &'static str, data: &[u8]) -> Item {
    Item {
        domain,
        path,
        kind: Kind::File(data.to_vec()),
        mtime: 1_600_000_000,
    }
}

fn dir(domain: &'static str, path: &'static str) -> Item {
    Item {
        domain,
        path,
        kind: Kind::Dir,
        mtime: 1_600_000_000,
    }
}

fn sample_items() -> Vec<Item> {
    vec![
        dir("HomeDomain", "Library"),
        dir("HomeDomain", "Library/Preferences"),
        file(
            "HomeDomain",
            "Library/Preferences/com.apple.a.plist",
            b"prefs A",
        ),
        file("HomeDomain", "Library/Notes.txt", &[7u8; 5000]),
        file("HomeDomain", "Library/Empty", b""),
        Item {
            domain: "HomeDomain",
            path: "Library/link",
            kind: Kind::Link("Notes.txt"),
            mtime: 0,
        },
        dir("AppDomain-com.example.app", "Documents"),
        file(
            "AppDomain-com.example.app",
            "Documents/save.json",
            b"{\"level\":3}",
        ),
        file(
            "AppDomainPlugin-com.example.app.widget",
            "Library/cache.db",
            b"cache",
        ),
        file("CameraRollDomain", "Media/DCIM/IMG_0001.JPG", b"jpeg bytes"),
    ]
}

fn file_id(domain: &str, path: &str) -> String {
    let digest = Sha1::digest(format!("{domain}-{path}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn key(seed: u8) -> [u8; 32] {
    std::array::from_fn(|i| seed.wrapping_mul(31).wrapping_add(i as u8))
}

fn wrap(kek: &[u8; 32], key: &[u8; 32]) -> Vec<u8> {
    let mut out = [0u8; 40];
    aes_kw::KekAes256::from(*kek).wrap(key, &mut out).unwrap();
    out.to_vec()
}

fn encrypt(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let pad = 16 - data.len() % 16;
    let mut buf = data.to_vec();
    buf.extend(std::iter::repeat_n(pad as u8, pad));
    let mut cipher = cbc::Encryptor::<Aes256>::new(key.into(), &[0u8; 16].into());
    for block in buf.as_chunks_mut::<16>().0 {
        cipher.encrypt_block_mut(GenericArray::from_mut_slice(block));
    }
    buf
}

fn mbfile(item: &Item, encryption_key: Option<Vec<u8>>) -> Vec<u8> {
    let (size, mode) = match &item.kind {
        Kind::File(data) => (data.len() as i64, 0o100644),
        Kind::Dir => (0, 0o040755),
        Kind::Link(_) => (0, 0o120755),
    };
    let mut objects = vec![Value::String("$null".into())];
    let mut root = Dictionary::new();
    root.insert("Size".into(), Value::Integer(size.into()));
    root.insert("LastModified".into(), Value::Integer(item.mtime.into()));
    root.insert("Mode".into(), Value::Integer(mode.into()));
    root.insert("Flags".into(), Value::Integer(0.into()));
    root.insert("ProtectionClass".into(), Value::Integer(3.into()));
    objects.push(Value::Dictionary(Dictionary::new()));
    objects.push(Value::String(item.path.into()));
    root.insert("RelativePath".into(), Value::Uid(Uid::new(2)));
    if let Kind::Link(target) = item.kind {
        objects.push(Value::String(target.into()));
        root.insert(
            "Target".into(),
            Value::Uid(Uid::new(objects.len() as u64 - 1)),
        );
    }
    if let Some(key) = encryption_key {
        let mut ns_data = Dictionary::new();
        ns_data.insert("NS.data".into(), Value::Data(key));
        objects.push(Value::Dictionary(ns_data));
        root.insert(
            "EncryptionKey".into(),
            Value::Uid(Uid::new(objects.len() as u64 - 1)),
        );
    }
    let mut class = Dictionary::new();
    class.insert("$classname".into(), Value::String("MBFile".into()));
    objects.push(Value::Dictionary(class));
    root.insert(
        "$class".into(),
        Value::Uid(Uid::new(objects.len() as u64 - 1)),
    );
    objects[1] = Value::Dictionary(root);

    let mut top = Dictionary::new();
    top.insert("root".into(), Value::Uid(Uid::new(1)));
    let mut archive = Dictionary::new();
    archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
    archive.insert("$version".into(), Value::Integer(100000.into()));
    archive.insert("$top".into(), Value::Dictionary(top));
    archive.insert("$objects".into(), Value::Array(objects));
    let mut out = Vec::new();
    plist::to_writer_binary(&mut out, &Value::Dictionary(archive)).unwrap();
    out
}

fn tlv(out: &mut Vec<u8>, tag: &[u8; 4], value: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

struct Crypto {
    keybag: Vec<u8>,
    class_keys: Vec<(u32, [u8; 32])>,
}

fn make_keybag(password: &str) -> Crypto {
    let salt = [1u8; 20];
    let dp_salt = [2u8; 20];
    let (iter, dp_iter) = (3u32, 2u32);
    let mut stage = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), &dp_salt, dp_iter, &mut stage);
    let mut pass_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&stage, &salt, iter, &mut pass_key);

    let mut bag = Vec::new();
    tlv(&mut bag, b"VERS", &4u32.to_be_bytes());
    tlv(&mut bag, b"TYPE", &1u32.to_be_bytes());
    tlv(&mut bag, b"UUID", &[9u8; 16]);
    tlv(&mut bag, b"HMCK", &[0u8; 40]);
    tlv(&mut bag, b"WRAP", &0u32.to_be_bytes());
    tlv(&mut bag, b"SALT", &salt);
    tlv(&mut bag, b"ITER", &iter.to_be_bytes());
    tlv(&mut bag, b"DPWT", &1u32.to_be_bytes());
    tlv(&mut bag, b"DPIC", &dp_iter.to_be_bytes());
    tlv(&mut bag, b"DPSL", &dp_salt);
    let mut class_keys = Vec::new();
    for class in 1..=4u32 {
        let class_key = key(class as u8 + 100);
        tlv(&mut bag, b"UUID", &[class as u8; 16]);
        tlv(&mut bag, b"CLAS", &class.to_be_bytes());
        tlv(&mut bag, b"WRAP", &2u32.to_be_bytes());
        tlv(&mut bag, b"KTYP", &0u32.to_be_bytes());
        tlv(&mut bag, b"WPKY", &wrap(&pass_key, &class_key));
        class_keys.push((class, class_key));
    }
    Crypto {
        keybag: bag,
        class_keys,
    }
}

fn item_key_blob(crypto: &Crypto, class: u32, key: &[u8; 32]) -> Vec<u8> {
    let class_key = crypto
        .class_keys
        .iter()
        .find(|(c, _)| *c == class)
        .unwrap()
        .1;
    let mut blob = class.to_le_bytes().to_vec();
    blob.extend(wrap(&class_key, key));
    blob
}

/// Writes a backup for `udid` into `root` and returns its folder.
fn build_backup(root: &Path, udid: &str, items: &[Item], password: Option<&str>) -> PathBuf {
    let dir = root.join(udid);
    fs::create_dir_all(&dir).unwrap();
    let crypto = password.map(make_keybag);

    let db_path = dir.join("Manifest.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);
         CREATE TABLE Properties (key TEXT PRIMARY KEY, value BLOB);",
    )
    .unwrap();
    for (n, item) in items.iter().enumerate() {
        let id = file_id(item.domain, item.path);
        let flags = match item.kind {
            Kind::File(_) => 1,
            Kind::Dir => 2,
            Kind::Link(_) => 4,
        };
        let mut wrapped = None;
        if let Kind::File(data) = &item.kind {
            let blob_dir = dir.join(&id[..2]);
            fs::create_dir_all(&blob_dir).unwrap();
            let stored = match &crypto {
                Some(crypto) => {
                    let file_key = key(n as u8);
                    wrapped = Some(item_key_blob(crypto, 3, &file_key));
                    encrypt(&file_key, data)
                }
                None => data.clone(),
            };
            if !data.is_empty() {
                fs::write(blob_dir.join(&id), stored).unwrap();
            }
        }
        db.execute(
            "INSERT INTO Files VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, item.domain, item.path, flags, mbfile(item, wrapped)],
        )
        .unwrap();
    }
    drop(db);

    let mut manifest = Dictionary::new();
    manifest.insert("IsEncrypted".into(), Value::Boolean(crypto.is_some()));
    manifest.insert("Version".into(), Value::String("10.0".into()));
    let mut lockdown = Dictionary::new();
    lockdown.insert("UniqueDeviceID".into(), Value::String(udid.into()));
    lockdown.insert("DeviceName".into(), Value::String("Lockdown Name".into()));
    lockdown.insert("ProductVersion".into(), Value::String("17.5".into()));
    manifest.insert("Lockdown".into(), Value::Dictionary(lockdown));
    if let Some(crypto) = &crypto {
        manifest.insert("BackupKeyBag".into(), Value::Data(crypto.keybag.clone()));
        let manifest_key = key(200);
        manifest.insert(
            "ManifestKey".into(),
            Value::Data(item_key_blob(crypto, 4, &manifest_key)),
        );
        let plain = fs::read(&db_path).unwrap();
        fs::write(&db_path, encrypt(&manifest_key, &plain)).unwrap();
    }
    plist::to_file_binary(dir.join("Manifest.plist"), &manifest).unwrap();

    let mut info = Dictionary::new();
    info.insert("Device Name".into(), Value::String("Test iPhone".into()));
    info.insert("Product Type".into(), Value::String("iPhone15,2".into()));
    info.insert("Unique Identifier".into(), Value::String(udid.into()));
    info.insert(
        "Last Backup Date".into(),
        Value::Date((SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)).into()),
    );
    plist::to_file_xml(dir.join("Info.plist"), &info).unwrap();

    let mut status = Dictionary::new();
    status.insert("IsFullBackup".into(), Value::Boolean(true));
    status.insert("SnapshotState".into(), Value::String("finished".into()));
    plist::to_file_xml(dir.join("Status.plist"), &status).unwrap();
    dir
}

#[test]
fn discovery_lists_backups_and_skips_junk() {
    let root = tempfile::tempdir().unwrap();
    build_backup(root.path(), UDID, &sample_items(), None);
    build_backup(
        root.path(),
        "encrypted-udid",
        &sample_items(),
        Some(PASSWORD),
    );
    fs::create_dir(root.path().join("not-a-backup")).unwrap();
    fs::write(root.path().join("stray.txt"), b"x").unwrap();

    let backups = list_backups(root.path()).unwrap();
    assert_eq!(backups.len(), 2);
    let plain = backups.iter().find(|b| b.udid.0 == UDID).unwrap();
    assert_eq!(plain.device_name.as_deref(), Some("Test iPhone"));
    assert_eq!(plain.product_type.as_deref(), Some("iPhone15,2"));
    assert_eq!(plain.ios_version.as_deref(), Some("17.5"));
    assert_eq!(
        plain.last_backup_date.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
    assert!(!plain.encrypted);
    assert!(plain.full_backup);
    assert!(plain.complete);
    assert!(plain.size_on_disk > 5000);
    assert!(backups.iter().any(|b| b.encrypted));

    let json = serde_json::to_value(plain).unwrap();
    assert!(json.get("lastBackupDate").is_some());
    assert!(json.get("sizeOnDisk").is_some());

    assert!(
        list_backups(&root.path().join("missing"))
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        read_summary(&root.path().join("not-a-backup")),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn browse_domains_apps_and_search() {
    let root = tempfile::tempdir().unwrap();
    let dir = build_backup(root.path(), UDID, &sample_items(), None);
    let backup = Backup::open(&dir, None).unwrap();

    let domains: Vec<_> = backup
        .domains()
        .unwrap()
        .into_iter()
        .map(|d| (d.name, d.file_count))
        .collect();
    assert_eq!(domains.len(), 4);
    assert!(domains.contains(&("HomeDomain".into(), 6)));

    let apps = backup.apps().unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].bundle_id, "com.example.app");
    assert_eq!(apps[0].file_count, 3);

    let home = backup
        .files(&FileQuery {
            domain: Some("HomeDomain".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(home.len(), 6);

    let prefs = backup
        .files(&FileQuery {
            domain: Some("HomeDomain".into()),
            prefix: Some("Library/Preferences/".into()),
            ..Default::default()
        })
        .unwrap();
    let paths: Vec<_> = prefs.iter().map(|e| e.relative_path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Library/Preferences",
            "Library/Preferences/com.apple.a.plist"
        ]
    );

    let hits = backup
        .files(&FileQuery {
            search: Some("img_".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].domain, "CameraRollDomain");
    assert_eq!(hits[0].size, 10);

    let limited = backup
        .files(&FileQuery {
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 2);

    let notes = backup.file("HomeDomain", "Library/Notes.txt").unwrap();
    assert_eq!(notes.kind, FileKind::File);
    assert_eq!(notes.size, 5000);
    assert_eq!(notes.modified, Some(1_600_000_000));
    assert_eq!(notes.mode & 0o777, 0o644);
    assert_eq!(notes.file_id, file_id("HomeDomain", "Library/Notes.txt"));
    assert_eq!(backup.read_file(&notes).unwrap(), vec![7u8; 5000]);

    let link = backup.file("HomeDomain", "Library/link").unwrap();
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.link_target.as_deref(), Some("Notes.txt"));

    assert!(matches!(
        backup.file("HomeDomain", "nope"),
        Err(Error::NotFound(_))
    ));

    let json = serde_json::to_value(&notes).unwrap();
    assert_eq!(json["relativePath"], "Library/Notes.txt");
    assert!(json.get("encryptionKey").is_none());
}

#[test]
fn extract_domain_prefix_and_single_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = build_backup(root.path(), UDID, &sample_items(), None);
    let backup = Backup::open(&dir, None).unwrap();
    let out = tempfile::tempdir().unwrap();

    let mut reports = Vec::new();
    let summary = backup
        .extract("HomeDomain", "Library", out.path(), |p| reports.push(p))
        .unwrap();
    assert_eq!(summary.files, 3);
    assert_eq!(summary.directories, 2);
    assert_eq!(summary.skipped_links, 1);
    assert_eq!(summary.bytes, 5007);
    assert_eq!(summary.missing, 0);
    assert_eq!(reports.last().unwrap().done, 5007);
    assert!(
        reports
            .iter()
            .all(|p| p.stage == "extract" && p.total == Some(5007))
    );

    let notes = out.path().join("Library/Notes.txt");
    assert_eq!(fs::read(&notes).unwrap(), vec![7u8; 5000]);
    let mtime = fs::metadata(&notes).unwrap().modified().unwrap();
    assert_eq!(
        mtime,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
    );
    assert_eq!(
        fs::read(out.path().join("Library/Preferences/com.apple.a.plist")).unwrap(),
        b"prefs A"
    );
    assert_eq!(fs::read(out.path().join("Library/Empty")).unwrap(), b"");
    assert!(!out.path().join("Library/link").exists());

    let entry = backup
        .file("AppDomain-com.example.app", "Documents/save.json")
        .unwrap();
    let single = out.path().join("single/save.json");
    assert_eq!(backup.extract_file(&entry, &single).unwrap(), 11);
    assert_eq!(fs::read(single).unwrap(), b"{\"level\":3}");

    assert!(matches!(
        backup.extract("HomeDomain", "Nothing", out.path(), |_| {}),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn extract_refuses_path_traversal() {
    let root = tempfile::tempdir().unwrap();
    let items = vec![
        file("HomeDomain", "Library/ok.txt", b"fine"),
        file("HomeDomain", "Library/../../escape.txt", b"evil"),
    ];
    let dir = build_backup(root.path(), UDID, &items, None);
    let backup = Backup::open(&dir, None).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("dest");

    let result = backup.extract("HomeDomain", "", &dest, |_| {});
    assert!(matches!(result, Err(Error::InvalidInput(_))));
    assert!(!out.path().join("escape.txt").exists());
    assert!(!dest.join("Library/ok.txt").exists());
}

#[test]
fn encrypted_backup_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let dir = build_backup(root.path(), UDID, &sample_items(), Some(PASSWORD));
    assert!(read_summary(&dir).unwrap().encrypted);
    assert!(fs::read(dir.join("Manifest.db")).unwrap()[..16] != *b"SQLite format 3\0");

    let backup = Backup::open(&dir, Some(PASSWORD)).unwrap();
    assert_eq!(backup.domains().unwrap().len(), 4);

    let notes = backup.file("HomeDomain", "Library/Notes.txt").unwrap();
    assert_eq!(backup.read_file(&notes).unwrap(), vec![7u8; 5000]);

    let prefs = backup
        .file("HomeDomain", "Library/Preferences/com.apple.a.plist")
        .unwrap();
    assert_eq!(backup.read_file(&prefs).unwrap(), b"prefs A");

    // An entry that went through JSON has lost its key and is looked up again.
    let roundtrip: orchard_backup::FileEntry =
        serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
    assert_eq!(backup.read_file(&roundtrip).unwrap(), b"prefs A");

    let out = tempfile::tempdir().unwrap();
    let summary = backup
        .extract("CameraRollDomain", "Media", out.path(), |_| {})
        .unwrap();
    assert_eq!(summary.files, 1);
    assert_eq!(
        fs::read(out.path().join("Media/DCIM/IMG_0001.JPG")).unwrap(),
        b"jpeg bytes"
    );
}

#[test]
fn encrypted_backup_rejects_wrong_or_missing_password() {
    let root = tempfile::tempdir().unwrap();
    let dir = build_backup(root.path(), UDID, &sample_items(), Some(PASSWORD));
    assert!(matches!(
        Backup::open(&dir, Some("wrong")),
        Err(Error::BackupPassword)
    ));
    assert!(matches!(
        Backup::open(&dir, None),
        Err(Error::BackupPassword)
    ));
}

#[test]
fn extract_keeps_existing_files_and_survives_missing_blobs() {
    let root = tempfile::tempdir().unwrap();
    let dir = build_backup(root.path(), UDID, &sample_items(), None);
    let notes_id = file_id("HomeDomain", "Library/Notes.txt");
    fs::remove_file(dir.join(&notes_id[..2]).join(&notes_id)).unwrap();
    let backup = Backup::open(&dir, None).unwrap();

    let out = tempfile::tempdir().unwrap();
    let library = out.path().join("Library");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("Notes.txt"), b"mine").unwrap();
    fs::write(library.join("Empty"), b"also mine").unwrap();

    let summary = backup
        .extract("HomeDomain", "Library", out.path(), |_| {})
        .unwrap();
    assert_eq!(summary.missing, 1);
    assert_eq!(summary.files, 2);
    assert_eq!(summary.renamed, 1);
    assert_eq!(fs::read(library.join("Notes.txt")).unwrap(), b"mine");
    assert_eq!(fs::read(library.join("Empty")).unwrap(), b"also mine");
    assert_eq!(fs::read(library.join("Empty (1)")).unwrap(), b"");
    let leftovers: Vec<_> = fs::read_dir(&library)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".orchard-part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    let notes = backup.file("HomeDomain", "Library/Notes.txt").unwrap();
    let dest = library.join("Notes.txt");
    assert!(matches!(
        backup.extract_file(&notes, &dest),
        Err(Error::NotFound(_))
    ));
    assert_eq!(fs::read(&dest).unwrap(), b"mine");
    assert!(!library.join("Notes.txt.orchard-part").exists());
}
