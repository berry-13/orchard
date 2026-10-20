mod common;

use std::io::Write;

use apple_codesign::cryptography::InMemoryPrivateKey;
use apple_codesign::macho_builder::MachOBuilder;
use apple_codesign::{
    CertificateProfile, MachFile, MachoTarget, Platform,
    create_self_signed_code_signing_certificate, verify_macho_data,
};
use cryptographic_message_syntax::{SignedDataBuilder, SignerBuilder};
use orchard_core::{Error, Udid};
use orchard_ipa::{
    IpaInfo, ProvisioningProfile, ResignIssue, ResignOptions, SigningIdentity, check_resign,
    inspect, resign_ipa,
};
use x509_certificate::{CapturedX509Certificate, EcdsaCurve, InMemorySigningKeyPair, KeyAlgorithm};
use zip::write::SimpleFileOptions;

const TEAM: &str = "ABCDE12345";
const DEVICE: &str = "00008030-001A2B3C4D5E6F70";
const PASSWORD: &str = "hunter2";

struct Fixture {
    cert: CapturedX509Certificate,
    key: InMemorySigningKeyPair,
    p12: Vec<u8>,
}

fn fixture(team: &str, days: i64) -> Fixture {
    let (cert, key) = create_self_signed_code_signing_certificate(
        KeyAlgorithm::Ecdsa(EcdsaCurve::Secp256r1),
        CertificateProfile::AppleDevelopment,
        team,
        "Jane Doe",
        "US",
        chrono::Duration::days(days),
    )
    .unwrap();
    let key_der = key.to_pkcs8_one_asymmetric_key_der();
    // Sanity check that the PKCS#8 form round trips through apple-codesign.
    InMemoryPrivateKey::from_pkcs8_der(&*key_der).unwrap();
    let p12 = p12::PFX::new(&cert.encode_der().unwrap(), &key_der, None, PASSWORD, "dev")
        .unwrap()
        .to_der();
    Fixture { cert, key, p12 }
}

struct ProfileSpec<'a> {
    app_id: &'a str,
    team: &'a str,
    devices: &'a [&'a str],
    expires_in_secs: i64,
    certificates: Vec<Vec<u8>>,
}

fn profile_bytes(signer: &Fixture, spec: &ProfileSpec) -> Vec<u8> {
    let now = std::time::SystemTime::now();
    let expires = if spec.expires_in_secs >= 0 {
        now + std::time::Duration::from_secs(spec.expires_in_secs as u64)
    } else {
        now - std::time::Duration::from_secs((-spec.expires_in_secs) as u64)
    };

    let mut ent = plist::Dictionary::new();
    ent.insert(
        "application-identifier".into(),
        format!("{}.{}", spec.team, spec.app_id).into(),
    );
    ent.insert(
        "com.apple.developer.team-identifier".into(),
        spec.team.into(),
    );
    ent.insert("get-task-allow".into(), true.into());
    ent.insert(
        "keychain-access-groups".into(),
        plist::Value::Array(vec![
            format!("{}.*", spec.team).into(),
            "com.apple.token".into(),
        ]),
    );

    let mut dict = plist::Dictionary::new();
    dict.insert("Name".into(), "Orchard Dev".into());
    dict.insert("UUID".into(), "1F2E3D4C-0000-1111-2222-333344445555".into());
    dict.insert("TeamName".into(), "Jane Doe".into());
    dict.insert("AppIDName".into(), "Demo".into());
    dict.insert(
        "TeamIdentifier".into(),
        plist::Value::Array(vec![spec.team.into()]),
    );
    dict.insert(
        "ApplicationIdentifierPrefix".into(),
        plist::Value::Array(vec![spec.team.into()]),
    );
    dict.insert("CreationDate".into(), plist::Date::from(now).into());
    dict.insert("ExpirationDate".into(), plist::Date::from(expires).into());
    dict.insert("Entitlements".into(), ent.into());
    dict.insert(
        "ProvisionedDevices".into(),
        plist::Value::Array(spec.devices.iter().map(|d| (*d).into()).collect()),
    );
    dict.insert(
        "DeveloperCertificates".into(),
        plist::Value::Array(
            spec.certificates
                .iter()
                .map(|c| plist::Value::Data(c.clone()))
                .collect(),
        ),
    );
    let mut xml = Vec::new();
    plist::Value::Dictionary(dict)
        .to_writer_xml(&mut xml)
        .unwrap();

    SignedDataBuilder::default()
        .content_inline(xml)
        .certificate(signer.cert.clone())
        .signer(SignerBuilder::new(&signer.key, signer.cert.clone()))
        .build_der()
        .unwrap()
}

fn spec<'a>(app_id: &'a str, id: &SigningIdentity) -> ProfileSpec<'a> {
    ProfileSpec {
        app_id,
        team: TEAM,
        devices: &[DEVICE],
        expires_in_secs: 7 * 86_400,
        certificates: vec![id.certificate_der().to_vec()],
    }
}

fn ipa_info(bundle_id: &str) -> IpaInfo {
    IpaInfo {
        bundle_id: bundle_id.into(),
        display_name: "Demo".into(),
        version: "1.0".into(),
        build: "1".into(),
        minimum_os: None,
        device_families: vec![],
        file_size: 0,
        executable: "Demo".into(),
        encrypted: Some(false),
        has_itunes_metadata: false,
        has_sinf: false,
        icon_png: None,
    }
}

#[test]
fn reads_p12_identity() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let info = id.info();
    assert_eq!(info.team_id.as_deref(), Some(TEAM));
    assert!(info.common_name.as_deref().unwrap().contains("Jane Doe"));
    let in_29_days = chrono::Utc::now().timestamp() + 29 * 86_400;
    assert!(info.expires_at > in_29_days && info.expires_at < in_29_days + 2 * 86_400);
    assert_eq!(id.certificate_der(), fx.cert.encode_der().unwrap());
    assert!(!format!("{id:?}").contains("key"));

    let err = SigningIdentity::from_p12(&fx.p12, "wrong").unwrap_err();
    assert!(matches!(err, Error::InvalidInput(m) if m.contains("password")));
    assert!(matches!(
        SigningIdentity::from_p12(b"garbage", PASSWORD),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn parses_profile() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let profile =
        ProvisioningProfile::parse(&profile_bytes(&fx, &spec("com.example.*", &id))).unwrap();

    assert_eq!(profile.name, "Orchard Dev");
    assert_eq!(profile.uuid, "1F2E3D4C-0000-1111-2222-333344445555");
    assert_eq!(profile.team_id, TEAM);
    assert_eq!(profile.team_name.as_deref(), Some("Jane Doe"));
    assert_eq!(profile.app_id_name.as_deref(), Some("Demo"));
    assert_eq!(profile.application_identifier, "ABCDE12345.com.example.*");
    assert_eq!(profile.app_id_prefix, TEAM);
    assert_eq!(profile.bundle_id_pattern(), "com.example.*");
    assert!(profile.is_wildcard());
    assert_eq!(profile.devices, [DEVICE]);
    assert!(!profile.provisions_all_devices);
    assert!(profile.expires_at > chrono::Utc::now().timestamp());
    assert!(profile.created_at.is_some());
    assert!(profile.includes_certificate(id.certificate_der()));

    assert!(profile.matches_bundle_id("com.example.demo"));
    assert!(profile.matches_bundle_id("com.example.demo.widget"));
    assert!(!profile.matches_bundle_id("com.example."));
    assert!(!profile.matches_bundle_id("com.other.demo"));
    assert!(profile.includes_device(&Udid(DEVICE.to_lowercase())));
    assert!(!profile.includes_device(&Udid("other".into())));

    let ent = profile.entitlements_for("com.example.demo");
    assert_eq!(
        ent["application-identifier"].as_string(),
        Some("ABCDE12345.com.example.demo")
    );
    assert_eq!(
        ent["com.apple.developer.team-identifier"].as_string(),
        Some(TEAM)
    );
    assert_eq!(ent["get-task-allow"].as_boolean(), Some(true));
    let groups: Vec<_> = ent["keychain-access-groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_string().unwrap())
        .collect();
    assert_eq!(groups, ["ABCDE12345.com.example.demo", "com.apple.token"]);

    let json = serde_json::to_value(&profile).unwrap();
    assert_eq!(json["appIdPrefix"], TEAM);
    assert!(json.get("raw").is_none());

    let exact =
        ProvisioningProfile::parse(&profile_bytes(&fx, &spec("com.example.demo", &id))).unwrap();
    assert!(!exact.is_wildcard());
    assert!(exact.matches_bundle_id("com.example.demo"));
    assert!(!exact.matches_bundle_id("com.example.demo.widget"));

    let any = ProvisioningProfile::parse(&profile_bytes(&fx, &spec("*", &id))).unwrap();
    assert!(any.is_wildcard());
    assert!(any.matches_bundle_id("org.anything"));

    assert!(matches!(
        ProvisioningProfile::parse(b"<plist/>"),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn check_resign_reports_issues() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let good =
        ProvisioningProfile::parse(&profile_bytes(&fx, &spec("com.example.*", &id))).unwrap();
    let udid = Udid(DEVICE.into());

    assert_eq!(
        check_resign(&ipa_info("com.example.demo"), &id, &good, Some(&udid)),
        []
    );
    assert_eq!(
        check_resign(&ipa_info("com.example.demo"), &id, &good, None),
        []
    );

    let mut encrypted = ipa_info("com.example.demo");
    encrypted.encrypted = Some(true);
    let other_device = Udid("ffff".into());
    let issues = check_resign(&encrypted, &id, &good, Some(&other_device));
    assert_eq!(
        issues,
        [
            ResignIssue::Encrypted,
            ResignIssue::DeviceNotProvisioned {
                udid: "ffff".into()
            }
        ]
    );

    let issues = check_resign(&ipa_info("org.other.app"), &id, &good, None);
    assert_eq!(
        issues,
        [ResignIssue::BundleIdNotMatched {
            bundle_id: "org.other.app".into(),
            pattern: "com.example.*".into()
        }]
    );
    let json = serde_json::to_value(&issues[0]).unwrap();
    assert_eq!(json["kind"], "bundleIdNotMatched");
    assert_eq!(json["bundleId"], "org.other.app");

    let mut bad = spec("com.example.*", &id);
    bad.team = "ZZZZZ99999";
    bad.expires_in_secs = -60;
    bad.certificates = vec![b"another cert".to_vec()];
    let bad = ProvisioningProfile::parse(&profile_bytes(&fx, &bad)).unwrap();
    let issues = check_resign(&ipa_info("com.example.demo"), &id, &bad, None);
    assert!(
        matches!(issues[0], ResignIssue::ProfileExpired { expired_at } if expired_at == bad.expires_at)
    );
    assert_eq!(
        issues[1],
        ResignIssue::TeamMismatch {
            certificate: Some(TEAM.into()),
            profile: "ZZZZZ99999".into()
        }
    );
    assert_eq!(issues[2], ResignIssue::CertificateNotInProfile);
    assert_eq!(issues.len(), 3);

    let expired = fixture(TEAM, 0);
    let expired = SigningIdentity::from_p12(&expired.p12, PASSWORD).unwrap();
    let issues = check_resign(&ipa_info("com.example.demo"), &expired, &good, None);
    assert!(matches!(issues[0], ResignIssue::CertificateExpired { .. }));
}

fn ios_macho(file_type: u32) -> Vec<u8> {
    MachOBuilder::new_aarch64(file_type)
        .macho_target(MachoTarget {
            platform: Platform::IOs,
            minimum_os_version: semver::Version::new(15, 0, 0),
            sdk_version: semver::Version::new(17, 0, 0),
        })
        .write_macho()
        .unwrap()
}

fn info_plist(id: &str, exe: &str, package_type: &str, binary: bool) -> Vec<u8> {
    let mut d = plist::Dictionary::new();
    d.insert("CFBundleIdentifier".into(), id.into());
    d.insert("CFBundleExecutable".into(), exe.into());
    d.insert("CFBundleName".into(), exe.into());
    d.insert("CFBundlePackageType".into(), package_type.into());
    d.insert("CFBundleShortVersionString".into(), "1.0".into());
    d.insert("CFBundleVersion".into(), "1".into());
    let mut out = Vec::new();
    let v = plist::Value::Dictionary(d);
    if binary {
        v.to_writer_binary(&mut out).unwrap();
    } else {
        v.to_writer_xml(&mut out).unwrap();
    }
    out
}

fn synthetic_ipa() -> Vec<u8> {
    const MH_EXECUTE: u32 = 2;
    const MH_DYLIB: u32 = 6;
    let app = "Payload/Demo.app/";
    let files: Vec<(String, Vec<u8>)> = vec![
        (
            format!("{app}Info.plist"),
            info_plist("com.example.demo", "Demo", "APPL", true),
        ),
        (format!("{app}Demo"), ios_macho(MH_EXECUTE)),
        (format!("{app}Assets.car"), vec![7u8; 1000]),
        (format!("{app}SC_Info/Demo.sinf"), b"old sinf".to_vec()),
        (
            format!("{app}embedded.mobileprovision"),
            b"old profile".to_vec(),
        ),
        (
            format!("{app}PlugIns/Widget.appex/Info.plist"),
            info_plist("com.example.demo.widget", "Widget", "XPC!", false),
        ),
        (
            format!("{app}PlugIns/Widget.appex/Widget"),
            ios_macho(MH_EXECUTE),
        ),
        (
            format!("{app}Frameworks/Kit.framework/Info.plist"),
            info_plist("com.vendor.kit", "Kit", "FMWK", false),
        ),
        (
            format!("{app}Frameworks/Kit.framework/Kit"),
            ios_macho(MH_DYLIB),
        ),
        ("iTunesMetadata.plist".into(), b"<plist/>".to_vec()),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in files {
        zip.start_file(name, SimpleFileOptions::default().unix_permissions(0o755))
            .unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn entry(entries: &[(String, Vec<u8>)], name: &str) -> Vec<u8> {
    entries
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .1
        .clone()
}

#[test]
fn resigns_synthetic_ipa() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let main_bytes = profile_bytes(&fx, &spec("org.me.demo", &id));
    let widget_bytes = profile_bytes(&fx, &spec("org.me.*", &id));
    let main_profile = ProvisioningProfile::parse(&main_bytes).unwrap();
    let widget_profile = ProvisioningProfile::parse(&widget_bytes).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.ipa");
    let dst = dir.path().join("out").join("signed.ipa");
    std::fs::create_dir(dst.parent().unwrap()).unwrap();
    std::fs::write(&src, synthetic_ipa()).unwrap();

    let mut options = ResignOptions::new(id, main_profile.clone());
    options.extension_profiles = vec![widget_profile];
    options.bundle_id = Some("org.me.demo".into());
    options.display_name = Some("My Demo".into());

    let mut stages = Vec::new();
    let report = resign_ipa(&src, &dst, &options, |p| stages.push(p.stage)).unwrap();
    assert!(stages.contains(&"sign".to_owned()) && stages.contains(&"package".to_owned()));

    assert_eq!(report.bundle_id, "org.me.demo");
    assert_eq!(report.team_id, TEAM);
    let bundles: Vec<_> = report
        .bundles
        .iter()
        .map(|b| {
            (
                b.path.as_str(),
                b.bundle_id.as_str(),
                b.profile_uuid.is_some(),
            )
        })
        .collect();
    assert_eq!(
        bundles,
        [
            ("Payload/Demo.app", "org.me.demo", true),
            (
                "Payload/Demo.app/Frameworks/Kit.framework",
                "com.vendor.kit",
                false
            ),
            (
                "Payload/Demo.app/PlugIns/Widget.appex",
                "org.me.demo.widget",
                true
            ),
        ]
    );
    assert_eq!(
        report.binaries,
        [
            "Payload/Demo.app/Demo",
            "Payload/Demo.app/Frameworks/Kit.framework/Kit",
            "Payload/Demo.app/PlugIns/Widget.appex/Widget",
        ]
    );
    assert_eq!(
        report.removed,
        ["iTunesMetadata.plist", "Payload/Demo.app/SC_Info"]
    );
    assert_eq!(report.output_size, std::fs::metadata(&dst).unwrap().len());
    // No temporary files are left next to the output.
    assert_eq!(std::fs::read_dir(dst.parent().unwrap()).unwrap().count(), 1);

    let entries = common::read_entries(&std::fs::read(&dst).unwrap());
    assert!(
        !entries
            .iter()
            .any(|(n, _)| n.contains("SC_Info") || n == "iTunesMetadata.plist")
    );
    assert_eq!(
        entry(&entries, "Payload/Demo.app/embedded.mobileprovision"),
        main_bytes
    );
    assert_eq!(
        entry(
            &entries,
            "Payload/Demo.app/PlugIns/Widget.appex/embedded.mobileprovision"
        ),
        widget_bytes
    );
    assert!(
        !entries
            .iter()
            .any(|(n, _)| n.starts_with("Payload/Demo.app/Frameworks/Kit.framework/embedded"))
    );
    for name in [
        "Payload/Demo.app/_CodeSignature/CodeResources",
        "Payload/Demo.app/PlugIns/Widget.appex/_CodeSignature/CodeResources",
        "Payload/Demo.app/Frameworks/Kit.framework/_CodeSignature/CodeResources",
    ] {
        entry(&entries, name);
    }

    let info: plist::Dictionary =
        plist::from_bytes(&entry(&entries, "Payload/Demo.app/Info.plist")).unwrap();
    assert_eq!(info["CFBundleIdentifier"].as_string(), Some("org.me.demo"));
    assert_eq!(info["CFBundleDisplayName"].as_string(), Some("My Demo"));
    assert!(entry(&entries, "Payload/Demo.app/Info.plist").starts_with(b"bplist"));

    let expected = [
        (
            "Payload/Demo.app/Demo",
            "org.me.demo",
            Some("ABCDE12345.org.me.demo"),
        ),
        (
            "Payload/Demo.app/PlugIns/Widget.appex/Widget",
            "org.me.demo.widget",
            Some("ABCDE12345.org.me.demo.widget"),
        ),
        (
            "Payload/Demo.app/Frameworks/Kit.framework/Kit",
            "com.vendor.kit",
            None,
        ),
    ];
    for (name, ident, app_id) in expected {
        let data = entry(&entries, name);
        let problems = verify_macho_data(&data);
        assert!(problems.is_empty(), "{name}: {problems:?}");

        let file = MachFile::parse(&data).unwrap();
        let macho = file.nth_macho(0).unwrap();
        let sig = macho.code_signature().unwrap().expect("signature");
        let cd = sig.code_directory().unwrap().expect("code directory");
        assert_eq!(cd.ident, ident, "{name}");
        assert_eq!(cd.team_name.as_deref(), Some(TEAM), "{name}");
        let ent = sig.entitlements().unwrap();
        match app_id {
            Some(app_id) => {
                let xml = ent.expect("entitlements").to_string();
                let ent: plist::Dictionary = plist::from_bytes(xml.as_bytes()).unwrap();
                assert_eq!(ent["application-identifier"].as_string(), Some(app_id));
            }
            None => assert!(ent.is_none(), "{name} has entitlements"),
        }
        assert!(
            sig.signature_data().unwrap().is_some(),
            "{name} is not CMS signed"
        );
    }

    let reread = inspect(&dst).unwrap();
    assert_eq!(reread.bundle_id, "org.me.demo");
    assert_eq!(reread.encrypted, Some(false));
}

#[test]
fn refuses_encrypted_and_unmatched() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let profile = ProvisioningProfile::parse(&profile_bytes(&fx, &spec("*", &id))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("out.ipa");

    let encrypted = dir.path().join("store.ipa");
    std::fs::write(&encrypted, common::build_ipa(&common::IpaSpec::default())).unwrap();
    let options = ResignOptions::new(id.clone(), profile);
    let err = resign_ipa(&encrypted, &dst, &options, |_| {}).unwrap_err();
    assert!(matches!(err, Error::InvalidInput(m) if m.contains("FairPlay")));

    let plain = dir.path().join("plain.ipa");
    std::fs::write(&plain, synthetic_ipa()).unwrap();
    let narrow =
        ProvisioningProfile::parse(&profile_bytes(&fx, &spec("com.example.demo", &id))).unwrap();
    let err = resign_ipa(
        &plain,
        &dst,
        &ResignOptions::new(id.clone(), narrow.clone()),
        |_| {},
    )
    .unwrap_err();
    // The widget extension is not covered by the explicit profile.
    assert!(matches!(err, Error::InvalidInput(m) if m.contains("com.example.demo.widget")));

    let mut renamed = ResignOptions::new(id, narrow);
    renamed.bundle_id = Some("org.other".into());
    let err = resign_ipa(&plain, &dst, &renamed, |_| {}).unwrap_err();
    assert!(matches!(err, Error::InvalidInput(m) if m.contains("org.other")));
    assert!(!dst.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

/// `synthetic_ipa` plus one symlink entry.
#[cfg(unix)]
fn ipa_with_symlink(name: &str, target: &str) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(std::io::Cursor::new(synthetic_ipa())).unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..src.len() {
        zip.raw_copy_file(src.by_index_raw(i).unwrap()).unwrap();
    }
    zip.add_symlink(name, target, SimpleFileOptions::default())
        .unwrap();
    zip.finish().unwrap().into_inner()
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_leaving_the_archive() {
    let fx = fixture(TEAM, 30);
    let id = SigningIdentity::from_p12(&fx.p12, PASSWORD).unwrap();
    let profile = ProvisioningProfile::parse(&profile_bytes(&fx, &spec("*", &id))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("victim");
    std::fs::write(&outside, b"keep").unwrap();
    let dst = dir.path().join("out.ipa");

    for (name, target) in [
        ("Payload/Demo.app/Escape", outside.to_str().unwrap()),
        ("Payload/Demo.app/Up", "../../../../../victim"),
        ("Payload/Demo.app/Dangling", "../../../../../nothing-here"),
    ] {
        let src = dir.path().join("evil.ipa");
        std::fs::write(&src, ipa_with_symlink(name, target)).unwrap();
        let options = ResignOptions::new(id.clone(), profile.clone());
        let err = resign_ipa(&src, &dst, &options, |_| {}).unwrap_err();
        assert!(
            matches!(&err, Error::InvalidInput(m) if m.contains("outside the app")),
            "{name}: {err}"
        );
    }
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    assert!(!dst.exists());

    // A link that stays inside the bundle is fine.
    let src = dir.path().join("ok.ipa");
    std::fs::write(
        &src,
        ipa_with_symlink("Payload/Demo.app/Alias", "Assets.car"),
    )
    .unwrap();
    resign_ipa(&src, &dst, &ResignOptions::new(id, profile), |_| {}).unwrap();
}
