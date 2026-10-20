//! Sideload re-signing with the user's own development identity, in the spirit of
//! `zsign` and `isign`, on top of the pure Rust `apple-codesign` crate.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use apple_codesign::cryptography::{InMemoryPrivateKey, parse_pfx_data};
use apple_codesign::{
    AppleCertificate, AppleCodesignError, BundleSigner, SettingsScope, SigningSettings,
};
use cryptographic_message_syntax::SignedData;
use orchard_core::{Error, Progress, Result, Udid};
use serde::{Deserialize, Serialize};
use x509_certificate::CapturedX509Certificate;

use crate::{IpaInfo, plist_string, zip_error};

const PROFILE_NAME: &str = "embedded.mobileprovision";

/// Public facts about a signing certificate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityInfo {
    /// Subject common name, e.g. `Apple Development: Jane Doe (ABCDE12345)`.
    pub common_name: Option<String>,
    /// Team identifier from the subject organizational unit.
    pub team_id: Option<String>,
    /// Unix seconds after which the certificate is no longer valid.
    pub expires_at: i64,
}

/// A code signing certificate with its private key, read from a `.p12` export.
#[derive(Clone)]
pub struct SigningIdentity {
    certificate: CapturedX509Certificate,
    key: InMemoryPrivateKey,
    info: IdentityInfo,
}

impl std::fmt::Debug for SigningIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningIdentity")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl SigningIdentity {
    /// Decodes a PKCS#12 bundle holding the certificate and its private key.
    pub fn from_p12(bytes: &[u8], password: &str) -> Result<Self> {
        let (certificate, key) = parse_pfx_data(bytes, password).map_err(|e| match e {
            AppleCodesignError::PfxBadPassword => {
                Error::InvalidInput("wrong password for the .p12 file".into())
            }
            e => Error::InvalidInput(format!("invalid .p12 file: {e}")),
        })?;
        let info = IdentityInfo {
            common_name: certificate.subject_common_name(),
            team_id: certificate.apple_team_id(),
            expires_at: certificate.validity_not_after().timestamp(),
        };
        Ok(Self {
            certificate,
            key,
            info,
        })
    }

    pub fn info(&self) -> &IdentityInfo {
        &self.info
    }

    /// The certificate in DER form.
    pub fn certificate_der(&self) -> &[u8] {
        self.certificate.constructed_data()
    }
}

/// A decoded `.mobileprovision` file.
///
/// The raw file and the developer certificates are not serialized; parse the
/// file again to sign with a profile that crossed a serialization boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningProfile {
    pub name: String,
    pub uuid: String,
    pub team_id: String,
    pub team_name: Option<String>,
    pub app_id_name: Option<String>,
    /// `application-identifier` entitlement, e.g. `ABCDE12345.com.example.*`.
    pub application_identifier: String,
    /// The part of `application_identifier` before the bundle id pattern.
    pub app_id_prefix: String,
    pub entitlements: plist::Dictionary,
    /// UDIDs of the provisioned devices.
    pub devices: Vec<String>,
    /// Enterprise profiles run on any device.
    pub provisions_all_devices: bool,
    /// Unix seconds.
    pub created_at: Option<i64>,
    /// Unix seconds.
    pub expires_at: i64,
    #[serde(skip)]
    certificates: Vec<Vec<u8>>,
    #[serde(skip)]
    raw: Vec<u8>,
}

impl ProvisioningProfile {
    /// Decodes the CMS envelope and the property list inside it. The envelope
    /// signature is not checked against Apple's roots; the device does that.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let signed = SignedData::parse_ber(bytes)
            .map_err(|e| Error::InvalidInput(format!("not a provisioning profile: {e}")))?;
        let content = signed
            .signed_content()
            .ok_or_else(|| Error::InvalidInput("provisioning profile has no content".into()))?;
        let dict: plist::Dictionary = plist::from_bytes(content)
            .map_err(|e| Error::InvalidInput(format!("invalid provisioning profile plist: {e}")))?;

        let missing = |key: &str| Error::InvalidInput(format!("provisioning profile has no {key}"));
        let first_string = |key: &str| {
            dict.get(key)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_string())
                .map(str::to_owned)
        };
        let date = |key: &str| {
            dict.get(key)
                .and_then(|v| v.as_date())
                .map(|d| unix(d.into()))
        };

        let entitlements = dict
            .get("Entitlements")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .ok_or_else(|| missing("Entitlements"))?;
        let team_id = first_string("TeamIdentifier").ok_or_else(|| missing("TeamIdentifier"))?;
        let application_identifier = plist_string(&entitlements, "application-identifier")
            .ok_or_else(|| missing("application-identifier"))?;
        let app_id_prefix = first_string("ApplicationIdentifierPrefix")
            .filter(|p| application_identifier.starts_with(&format!("{p}.")))
            .or_else(|| {
                application_identifier
                    .split_once('.')
                    .map(|(p, _)| p.to_owned())
            })
            .unwrap_or_else(|| team_id.clone());
        let devices = match dict.get("ProvisionedDevices") {
            Some(plist::Value::Array(values)) => values
                .iter()
                .filter_map(|v| v.as_string().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        let certificates = match dict.get("DeveloperCertificates") {
            Some(plist::Value::Array(values)) => values
                .iter()
                .filter_map(|v| v.as_data().map(<[u8]>::to_vec))
                .collect(),
            _ => Vec::new(),
        };

        Ok(Self {
            name: plist_string(&dict, "Name").unwrap_or_default(),
            uuid: plist_string(&dict, "UUID").ok_or_else(|| missing("UUID"))?,
            team_name: plist_string(&dict, "TeamName"),
            app_id_name: plist_string(&dict, "AppIDName"),
            provisions_all_devices: dict
                .get("ProvisionsAllDevices")
                .and_then(|v| v.as_boolean())
                .unwrap_or(false),
            created_at: date("CreationDate"),
            expires_at: date("ExpirationDate").ok_or_else(|| missing("ExpirationDate"))?,
            team_id,
            application_identifier,
            app_id_prefix,
            entitlements,
            devices,
            certificates,
            raw: bytes.to_vec(),
        })
    }

    /// The bundle id pattern, e.g. `com.example.*` or `*`.
    pub fn bundle_id_pattern(&self) -> &str {
        self.application_identifier
            .strip_prefix(&self.app_id_prefix)
            .and_then(|s| s.strip_prefix('.'))
            .unwrap_or(&self.application_identifier)
    }

    /// Whether the app id ends in a wildcard and covers more than one bundle id.
    pub fn is_wildcard(&self) -> bool {
        self.bundle_id_pattern().ends_with('*')
    }

    pub fn matches_bundle_id(&self, bundle_id: &str) -> bool {
        let pattern = self.bundle_id_pattern();
        match pattern.strip_suffix('*') {
            Some(prefix) => bundle_id.starts_with(prefix) && bundle_id.len() > prefix.len(),
            None => pattern == bundle_id,
        }
    }

    pub fn includes_device(&self, udid: &Udid) -> bool {
        self.provisions_all_devices || self.devices.iter().any(|d| d.eq_ignore_ascii_case(&udid.0))
    }

    /// Whether the certificate is one of the profile's developer certificates.
    pub fn includes_certificate(&self, der: &[u8]) -> bool {
        self.certificates.iter().any(|c| c == der)
    }

    /// Entitlements to sign `bundle_id` with: the profile's, with the app id and
    /// the default keychain group bound to the bundle id.
    pub fn entitlements_for(&self, bundle_id: &str) -> plist::Dictionary {
        let mut ent = self.entitlements.clone();
        let app_id = format!("{}.{bundle_id}", self.app_id_prefix);
        ent.insert("application-identifier".into(), app_id.clone().into());
        ent.insert(
            "com.apple.developer.team-identifier".into(),
            self.team_id.clone().into(),
        );
        if let Some(plist::Value::Array(groups)) = ent.get_mut("keychain-access-groups") {
            let wildcard = format!("{}.*", self.app_id_prefix);
            for group in groups.iter_mut() {
                if group.as_string() == Some(wildcard.as_str()) {
                    *group = app_id.clone().into();
                }
            }
        }
        ent
    }
}

/// Something that would make a re-signed app fail to install or launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResignIssue {
    /// The main executable is FairPlay encrypted and cannot run re-signed.
    Encrypted,
    CertificateExpired {
        expired_at: i64,
    },
    ProfileExpired {
        expired_at: i64,
    },
    TeamMismatch {
        certificate: Option<String>,
        profile: String,
    },
    /// The certificate is not one of the profile's developer certificates.
    CertificateNotInProfile,
    BundleIdNotMatched {
        bundle_id: String,
        pattern: String,
    },
    DeviceNotProvisioned {
        udid: String,
    },
}

/// Lists problems with signing `ipa_info` using `identity` and `profile`, and
/// installing the result on `udid`. To check a bundle id change, pass an
/// `IpaInfo` with the new `bundle_id`.
pub fn check_resign(
    ipa_info: &IpaInfo,
    identity: &SigningIdentity,
    profile: &ProvisioningProfile,
    udid: Option<&Udid>,
) -> Vec<ResignIssue> {
    let now = unix(SystemTime::now());
    let mut issues = Vec::new();
    if ipa_info.encrypted == Some(true) {
        issues.push(ResignIssue::Encrypted);
    }
    if identity.info.expires_at <= now {
        issues.push(ResignIssue::CertificateExpired {
            expired_at: identity.info.expires_at,
        });
    }
    if profile.expires_at <= now {
        issues.push(ResignIssue::ProfileExpired {
            expired_at: profile.expires_at,
        });
    }
    if identity.info.team_id.as_deref() != Some(profile.team_id.as_str()) {
        issues.push(ResignIssue::TeamMismatch {
            certificate: identity.info.team_id.clone(),
            profile: profile.team_id.clone(),
        });
    }
    if !profile.certificates.is_empty() && !profile.includes_certificate(identity.certificate_der())
    {
        issues.push(ResignIssue::CertificateNotInProfile);
    }
    if !profile.matches_bundle_id(&ipa_info.bundle_id) {
        issues.push(ResignIssue::BundleIdNotMatched {
            bundle_id: ipa_info.bundle_id.clone(),
            pattern: profile.bundle_id_pattern().to_owned(),
        });
    }
    if let Some(udid) = udid
        && !profile.includes_device(udid)
    {
        issues.push(ResignIssue::DeviceNotProvisioned {
            udid: udid.0.clone(),
        });
    }
    issues
}

/// How to re-sign an IPA.
#[derive(Debug, Clone)]
pub struct ResignOptions {
    pub identity: SigningIdentity,
    /// Profile for the main app.
    pub profile: ProvisioningProfile,
    /// Profiles for nested apps and extensions, picked by bundle id. A nested
    /// bundle no profile here matches falls back to `profile` when it matches.
    pub extension_profiles: Vec<ProvisioningProfile>,
    /// New bundle id for the main app; nested bundle ids sharing the old one as
    /// prefix are rewritten to the new prefix.
    pub bundle_id: Option<String>,
    /// New `CFBundleDisplayName` for the main app.
    pub display_name: Option<String>,
    /// Remove `SC_Info/` folders and `iTunesMetadata.plist` left by the App Store.
    pub strip_store_metadata: bool,
}

impl ResignOptions {
    pub fn new(identity: SigningIdentity, profile: ProvisioningProfile) -> Self {
        Self {
            identity,
            profile,
            extension_profiles: Vec::new(),
            bundle_id: None,
            display_name: None,
            strip_store_metadata: true,
        }
    }
}

/// A bundle signed by [`resign_ipa`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedBundle {
    /// Path inside the IPA, e.g. `Payload/Demo.app/PlugIns/Widget.appex`.
    pub path: String,
    pub bundle_id: String,
    /// Profile embedded in the bundle; `None` for frameworks.
    pub profile_uuid: Option<String>,
}

/// What [`resign_ipa`] did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResignReport {
    pub bundle_id: String,
    pub team_id: String,
    /// Main app first, then nested bundles.
    pub bundles: Vec<SignedBundle>,
    /// Paths inside the IPA of every signed Mach-O file.
    pub binaries: Vec<String>,
    /// Paths inside the IPA that were removed.
    pub removed: Vec<String>,
    pub output_size: u64,
}

/// Re-signs `src` into `dst` (written atomically) with the identity and
/// profiles in `options`. Refuses FairPlay encrypted apps.
pub fn resign_ipa(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    options: &ResignOptions,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<ResignReport> {
    let (src, dst) = (src.as_ref(), dst.as_ref());
    let info = crate::inspect(src)?;
    if info.encrypted == Some(true) {
        return Err(Error::InvalidInput(
            "the app is FairPlay encrypted (an App Store download) and cannot be re-signed; \
             use a decrypted IPA"
                .into(),
        ));
    }
    let new_id = options
        .bundle_id
        .clone()
        .unwrap_or_else(|| info.bundle_id.clone());
    if !options.profile.matches_bundle_id(&new_id) {
        return Err(Error::InvalidInput(format!(
            "provisioning profile {} ({}) does not cover bundle id {new_id}",
            options.profile.name, options.profile.application_identifier
        )));
    }

    let parent = dst
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let work = tempfile::Builder::new()
        .prefix(".orchard-resign")
        .tempdir_in(parent)?;
    let root = work.path().join("ipa");

    progress(Progress::new("extract", 0, None));
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(src)?)).map_err(zip_error)?;
    zip.extract(&root).map_err(zip_error)?;
    drop(zip);
    check_symlinks(&root)?;
    let app_dir = find_app(&root)?;

    let mut removed = Vec::new();
    if options.strip_store_metadata {
        for name in ["iTunesMetadata.plist", "iTunesArtwork", "META-INF"] {
            let path = root.join(name);
            if remove_path(&path)? {
                removed.push(rel(&root, &path));
            }
        }
        for dir in walk(&app_dir)? {
            if dir.file_name().is_some_and(|n| n == "SC_Info") && remove_path(&dir)? {
                removed.push(rel(&root, &dir));
            }
        }
    }

    let mut bundles = Vec::new();
    let mut settings = SigningSettings::default();
    settings.set_signing_key(&options.identity.key, options.identity.certificate.clone());
    settings.chain_apple_certificates();
    settings.set_team_id(options.profile.team_id.clone());

    let main_info = app_dir.join("Info.plist");
    rewrite_info_plist(&main_info, |dict| {
        dict.insert("CFBundleIdentifier".into(), new_id.clone().into());
        if let Some(name) = &options.display_name {
            dict.insert("CFBundleDisplayName".into(), name.clone().into());
        }
        retarget_companion_ids(dict, &info.bundle_id, &new_id);
    })?;
    write_regular(&app_dir.join(PROFILE_NAME), &options.profile.raw)?;
    set_entitlements(
        &mut settings,
        SettingsScope::Main,
        &options.profile.entitlements_for(&new_id),
    )?;
    bundles.push(SignedBundle {
        path: rel(&root, &app_dir),
        bundle_id: new_id.clone(),
        profile_uuid: Some(options.profile.uuid.clone()),
    });

    for dir in nested_bundles(&app_dir)? {
        let kind = dir.extension().and_then(|e| e.to_str()).unwrap_or_default();
        let plist_path = dir.join("Info.plist");
        let mut bundle_id = None;
        rewrite_info_plist(&plist_path, |dict| {
            let old = plist_string(dict, "CFBundleIdentifier").unwrap_or_default();
            let id = retarget(&old, &info.bundle_id, &new_id);
            if kind != "framework" {
                dict.insert("CFBundleIdentifier".into(), id.clone().into());
                retarget_companion_ids(dict, &info.bundle_id, &new_id);
            }
            bundle_id = Some(if kind == "framework" { old } else { id });
        })?;
        let bundle_id = bundle_id.unwrap_or_default();
        let scope_path = rel(&app_dir, &dir);
        let mut profile_uuid = None;
        if kind != "framework" {
            let profile = options
                .extension_profiles
                .iter()
                .filter(|p| p.matches_bundle_id(&bundle_id))
                .min_by_key(|p| p.is_wildcard())
                .or_else(|| Some(&options.profile).filter(|p| p.matches_bundle_id(&bundle_id)))
                .ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "no provisioning profile covers nested bundle {bundle_id} ({scope_path})"
                    ))
                })?;
            write_regular(&dir.join(PROFILE_NAME), &profile.raw)?;
            set_entitlements(
                &mut settings,
                SettingsScope::Path(scope_path),
                &profile.entitlements_for(&bundle_id),
            )?;
            profile_uuid = Some(profile.uuid.clone());
        }
        bundles.push(SignedBundle {
            path: rel(&root, &dir),
            bundle_id,
            profile_uuid,
        });
    }

    progress(Progress::new("sign", 0, Some(1)));
    let mut signer = BundleSigner::new_from_path(&app_dir).map_err(sign_error)?;
    signer.collect_nested_bundles().map_err(sign_error)?;
    signer
        .write_signed_bundle(&app_dir, &settings)
        .map_err(sign_error)?;
    progress(Progress::new("sign", 1, Some(1)));

    let entries = walk(&root)?;
    let binaries = entries
        .iter()
        .filter(|p| !p.is_symlink() && p.is_file() && p.starts_with(&app_dir) && is_macho(p))
        .map(|p| rel(&root, p))
        .collect();

    let tmp = tempfile::Builder::new()
        .prefix(".orchard-resign")
        .suffix(".ipa")
        .tempfile_in(parent)?;
    write_zip(&root, &entries, tmp.as_file(), &mut progress)?;
    let output_size = tmp.as_file().metadata()?.len();
    tmp.persist(dst).map_err(|e| Error::Io(e.error))?;

    Ok(ResignReport {
        bundle_id: new_id,
        team_id: options.profile.team_id.clone(),
        bundles,
        binaries,
        removed,
        output_size,
    })
}

fn unix(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

fn sign_error(e: AppleCodesignError) -> Error {
    match e {
        AppleCodesignError::Io(e) => Error::Io(e),
        e => crate::protocol("code signing failed", e),
    }
}

fn set_entitlements(
    settings: &mut SigningSettings,
    scope: SettingsScope,
    entitlements: &plist::Dictionary,
) -> Result<()> {
    let mut xml = Vec::new();
    plist::Value::Dictionary(entitlements.clone())
        .to_writer_xml(&mut xml)
        .map_err(|e| crate::protocol("cannot encode entitlements", e))?;
    let xml =
        String::from_utf8(xml).map_err(|e| crate::protocol("cannot encode entitlements", e))?;
    settings
        .set_entitlements_xml(scope, xml)
        .map_err(sign_error)
}

/// Replaces the `old` bundle id prefix of `id` with `new`.
fn retarget(id: &str, old: &str, new: &str) -> String {
    if old == new {
        return id.to_owned();
    }
    match id.strip_prefix(old) {
        Some(rest) if rest.is_empty() || rest.starts_with('.') => format!("{new}{rest}"),
        _ => id.to_owned(),
    }
}

/// Watch apps and their extensions refer to their parent by bundle id.
fn retarget_companion_ids(dict: &mut plist::Dictionary, old: &str, new: &str) {
    if let Some(plist::Value::String(id)) = dict.get_mut("WKCompanionAppBundleIdentifier") {
        *id = retarget(id, old, new);
    }
    let attributes = dict
        .get_mut("NSExtension")
        .and_then(|v| v.as_dictionary_mut())
        .and_then(|d| d.get_mut("NSExtensionAttributes"))
        .and_then(|v| v.as_dictionary_mut());
    if let Some(plist::Value::String(id)) =
        attributes.and_then(|d| d.get_mut("WKAppBundleIdentifier"))
    {
        *id = retarget(id, old, new);
    }
}

/// Applies `edit` to an Info.plist, keeping its binary or XML encoding.
fn rewrite_info_plist(path: &Path, edit: impl FnOnce(&mut plist::Dictionary)) -> Result<()> {
    let data = fs::read(path)?;
    let mut dict: plist::Dictionary = plist::from_bytes(&data)
        .map_err(|e| crate::protocol(&format!("invalid {}", path.display()), e))?;
    let before = dict.clone();
    edit(&mut dict);
    if dict == before {
        return Ok(());
    }
    let value = plist::Value::Dictionary(dict);
    let mut out = Vec::new();
    let written = if data.starts_with(b"bplist") {
        value.to_writer_binary(&mut out)
    } else {
        value.to_writer_xml(&mut out)
    };
    written.map_err(|e| crate::protocol("cannot encode Info.plist", e))?;
    write_regular(path, &out)
}

/// Rejects archives whose symlinks dangle or resolve outside `root`, so later
/// writes into the extracted tree cannot land elsewhere on disk.
fn check_symlinks(root: &Path) -> Result<()> {
    let real_root = fs::canonicalize(root)?;
    for path in walk(root)? {
        if !path.is_symlink() {
            continue;
        }
        let inside = fs::canonicalize(&path).is_ok_and(|target| target.starts_with(&real_root));
        if !inside {
            return Err(Error::InvalidInput(format!(
                "archive symlink {} points outside the app",
                rel(root, &path)
            )));
        }
    }
    Ok(())
}

/// Writes `path`, refusing to follow a symlink there.
fn write_regular(path: &Path, data: &[u8]) -> Result<()> {
    if path.is_symlink() {
        return Err(Error::InvalidInput(format!(
            "{} is a symlink",
            path.display()
        )));
    }
    fs::write(path, data)?;
    Ok(())
}

fn find_app(root: &Path) -> Result<PathBuf> {
    let mut apps: Vec<PathBuf> = fs::read_dir(root.join("Payload"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "app") && p.join("Info.plist").is_file())
        .collect();
    apps.sort();
    apps.into_iter()
        .next()
        .ok_or_else(|| Error::InvalidInput("no Payload/*.app in archive".into()))
}

/// Nested apps, extensions and frameworks with an identifier, deepest last.
fn nested_bundles(app_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = walk(app_dir)?
        .into_iter()
        .filter(|p| {
            !p.is_symlink()
                && p.is_dir()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "app" | "appex" | "framework"))
                && fs::read(p.join("Info.plist"))
                    .ok()
                    .and_then(|d| plist::from_bytes::<plist::Dictionary>(&d).ok())
                    .is_some_and(|d| plist_string(&d, "CFBundleIdentifier").is_some())
        })
        .collect();
    found.sort_by_key(|p| (p.components().count(), p.clone()));
    Ok(found)
}

/// Every path under `dir` (files, directories and symlinks), sorted, without
/// following symlinks.
fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path.clone());
            }
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn remove_path(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    Ok(true)
}

/// `path` relative to `base`, with `/` separators.
fn rel(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn is_macho(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && matches!(
            u32::from_be_bytes(magic),
            0xFEED_FACE | 0xFEED_FACF | 0xCEFA_EDFE | 0xCFFA_EDFE | 0xCAFE_BABE | 0xCAFE_BABF
        )
}

fn write_zip(
    root: &Path,
    entries: &[PathBuf],
    out: &File,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    use zip::write::SimpleFileOptions;

    let total = entries.len() as u64;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(out));
    let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (i, path) in entries.iter().enumerate() {
        let name = rel(root, path);
        let meta = fs::symlink_metadata(path)?;
        if meta.is_symlink() {
            let target = fs::read_link(path)?;
            zip.add_symlink(name, target.to_string_lossy(), deflate)
                .map_err(zip_error)?;
        } else if meta.is_dir() {
            zip.add_directory(name, deflate.unix_permissions(0o755))
                .map_err(zip_error)?;
        } else {
            let mode = if is_macho(path) { 0o755 } else { 0o644 };
            zip.start_file(name, deflate.unix_permissions(mode))
                .map_err(zip_error)?;
            std::io::copy(&mut File::open(path)?, &mut zip)?;
        }
        progress(Progress::new("package", i as u64 + 1, Some(total)));
    }
    zip.finish().map_err(zip_error)?.flush()?;
    Ok(())
}
