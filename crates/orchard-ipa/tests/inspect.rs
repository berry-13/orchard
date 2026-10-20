mod common;

use common::*;
use orchard_ipa::{Sinf, inspect, normalize_png, patch_ipa};

fn write_ipa(dir: &std::path::Path, spec: &IpaSpec) -> std::path::PathBuf {
    let path = dir.join("demo.ipa");
    std::fs::write(&path, build_ipa(spec)).unwrap();
    path
}

#[test]
fn inspects_bundle_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_ipa(dir.path(), &IpaSpec::default());
    let info = inspect(&path).unwrap();

    assert_eq!(info.bundle_id, "com.example.demo");
    assert_eq!(info.display_name, "Demo App");
    assert_eq!(info.version, "1.2.3");
    assert_eq!(info.build, "45");
    assert_eq!(info.minimum_os.as_deref(), Some("15.0"));
    assert_eq!(info.device_families, ["iphone", "ipad"]);
    assert_eq!(info.executable, EXECUTABLE);
    assert_eq!(info.file_size, std::fs::metadata(&path).unwrap().len());
    assert_eq!(info.encrypted, Some(true));
    assert!(!info.has_itunes_metadata);
    assert!(!info.has_sinf);

    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["bundleId"], "com.example.demo");
    assert_eq!(json["hasItunesMetadata"], false);
}

#[test]
fn detects_decrypted_binary_and_store_files() {
    let dir = tempfile::tempdir().unwrap();
    let spec = IpaSpec {
        encrypted: false,
        with_metadata: true,
        ..IpaSpec::default()
    };
    let info = inspect(write_ipa(dir.path(), &spec)).unwrap();
    assert_eq!(info.encrypted, Some(false));
    assert!(info.has_itunes_metadata);
    assert!(info.has_sinf);
}

#[test]
fn picks_largest_icon_and_uncrushes_it() {
    let dir = tempfile::tempdir().unwrap();
    let info = inspect(write_ipa(dir.path(), &IpaSpec::default())).unwrap();
    let (w, h, pixels) = decode_png(&info.icon_png.expect("icon"));
    assert_eq!((w, h), (4, 4));

    let expected = icon_pixels();
    for (got, want) in pixels.chunks(4).zip(expected.chunks(4)) {
        assert_eq!(got[3], want[3], "alpha");
        if want[3] == 0 {
            continue;
        }
        for c in 0..3 {
            let diff = i32::from(got[c]) - i32::from(want[c]);
            assert!(diff.abs() <= 2, "channel {c}: got {got:?}, want {want:?}");
        }
    }
}

#[test]
fn standard_png_passes_through() {
    let png = plain_png(&[9, 8, 7, 255], 1, 1);
    assert_eq!(normalize_png(&png).unwrap(), png);
    assert!(normalize_png(b"nope").is_err());
}

#[test]
fn rejects_archive_without_app() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.ipa");
    let zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.finish().unwrap();
    assert!(matches!(
        inspect(&path),
        Err(orchard_core::Error::InvalidInput(_))
    ));
}

#[test]
fn patch_replaces_existing_store_files() {
    let dir = tempfile::tempdir().unwrap();
    let spec = IpaSpec {
        with_metadata: true,
        ..IpaSpec::default()
    };
    let src = write_ipa(dir.path(), &spec);
    let dst = dir.path().join("patched.ipa");
    let mut metadata = plist::Dictionary::new();
    metadata.insert("itemId".into(), 42u64.into());
    let sinf = Sinf {
        id: 0,
        data: b"new sinf".to_vec(),
    };
    patch_ipa(&src, &dst, Some(&metadata), &[sinf]).unwrap();

    let entries = read_entries(&std::fs::read(&dst).unwrap());
    let get = |name: &str| {
        let hits: Vec<_> = entries.iter().filter(|(n, _)| n == name).collect();
        assert_eq!(hits.len(), 1, "{name} present exactly once");
        hits[0].1.clone()
    };
    assert_eq!(get("Payload/Demo.app/SC_Info/Demo.sinf"), b"new sinf");
    let written: plist::Dictionary = plist::from_bytes(&get("iTunesMetadata.plist")).unwrap();
    assert_eq!(written, metadata);
    assert!(!dir.path().join("patched.ipa.tmp").exists());
}
