use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use idevice::core_device::AppServiceClient;
use idevice::installation_proxy::InstallationProxyClient;
use idevice::springboardservices::SpringBoardServicesClient;
use orchard_core::{Error, Progress, Result};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};

use crate::connect::DeviceConnection;
use crate::error::device_error;
use crate::files::{DeviceFiles, FileEntry, FileLocation, normalize_path};
use crate::info::{string, uint};

const PUBLIC_STAGING: &str = "PublicStaging";

/// Attributes requested from installation_proxy `Browse`.
const ATTRIBUTES: &[&str] = &[
    "CFBundleIdentifier",
    "CFBundleDisplayName",
    "CFBundleName",
    "CFBundleVersion",
    "CFBundleShortVersionString",
    "ApplicationType",
    "StaticDiskUsage",
    "DynamicDiskUsage",
    "Path",
    "Container",
    "UIFileSharingEnabled",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    User,
    System,
    /// `Internal`, `Hidden` and anything newer.
    Other,
}

/// An installed app as reported by installation_proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub bundle_id: String,
    /// Display name, falling back to the bundle name, then the bundle id.
    pub name: String,
    /// `CFBundleVersion`, the build number.
    pub version: Option<String>,
    /// `CFBundleShortVersionString`, the marketing version.
    pub short_version: Option<String>,
    pub kind: AppKind,
    /// App bundle size in bytes.
    pub static_size: Option<u64>,
    /// Documents and data size in bytes.
    pub dynamic_size: Option<u64>,
    pub path: Option<String>,
    pub container: Option<String>,
    /// Whether the Documents folder is shared (Files and iTunes file sharing).
    pub file_sharing: bool,
}

impl AppInfo {
    /// Builds an entry from one installation_proxy app dictionary.
    pub(crate) fn from_plist(value: &Value) -> Option<Self> {
        let dict = value.as_dictionary()?;
        let bundle_id = string(dict, "CFBundleIdentifier")?;
        let name = string(dict, "CFBundleDisplayName")
            .or_else(|| string(dict, "CFBundleName"))
            .unwrap_or_else(|| bundle_id.clone());
        let kind = match dict.get("ApplicationType").and_then(Value::as_string) {
            Some("User") => AppKind::User,
            Some("System") => AppKind::System,
            _ => AppKind::Other,
        };
        Some(Self {
            name,
            version: string(dict, "CFBundleVersion"),
            short_version: string(dict, "CFBundleShortVersionString"),
            kind,
            static_size: uint(dict, "StaticDiskUsage"),
            dynamic_size: uint(dict, "DynamicDiskUsage"),
            path: string(dict, "Path"),
            container: string(dict, "Container"),
            file_sharing: dict
                .get("UIFileSharingEnabled")
                .and_then(Value::as_boolean)
                .unwrap_or(false),
            bundle_id,
        })
    }
}

/// Staged packages written by [`DeviceConnection::install_ipa`], complete or partial.
fn is_orchard_package(name: &str) -> bool {
    name.starts_with("orchard-") && (name.ends_with(".ipa") || name.ends_with(".ipa.orchard-part"))
}

/// Packages staged by installs running in this process, as (udid, device path).
static ACTIVE_PACKAGES: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

/// Marks a staged package as in use until dropped, including on cancellation.
struct StagedPackage {
    key: (String, String),
}

impl StagedPackage {
    fn register(udid: &str, package: &str) -> Self {
        let key = (udid.to_string(), normalize_path(package));
        if let Ok(mut active) = ACTIVE_PACKAGES.lock() {
            active.insert(key.clone());
        }
        Self { key }
    }
}

impl Drop for StagedPackage {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_PACKAGES.lock() {
            active.remove(&self.key);
        }
    }
}

/// An Orchard package (or its partial upload) not used by a running install.
fn is_stale_package(udid: &str, entry: &FileEntry) -> bool {
    if !is_orchard_package(&entry.name) {
        return false;
    }
    let package = entry
        .path
        .strip_suffix(".orchard-part")
        .unwrap_or(&entry.path);
    let key = (udid.to_string(), normalize_path(package));
    ACTIVE_PACKAGES
        .lock()
        .map(|active| !active.contains(&key))
        .unwrap_or(false)
}

fn browse_options(include_system: bool) -> Value {
    let mut options = Dictionary::new();
    options.insert(
        "ApplicationType".into(),
        if include_system { "Any" } else { "User" }.into(),
    );
    options.insert(
        "ReturnAttributes".into(),
        Value::Array(ATTRIBUTES.iter().map(|a| (*a).into()).collect()),
    );
    Value::Dictionary(options)
}

fn parse_app_list(values: &[Value]) -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> = values.iter().filter_map(AppInfo::from_plist).collect();
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.bundle_id.clone()));
    apps
}

impl DeviceConnection {
    /// Lists installed apps sorted by name. User apps only unless `include_system`.
    pub async fn list_apps(&self, include_system: bool) -> Result<Vec<AppInfo>> {
        let mut proxy = self.service::<InstallationProxyClient>().await?;
        let values = proxy
            .browse(Some(browse_options(include_system)))
            .await
            .map_err(|e| device_error("list apps", e))?;
        Ok(parse_app_list(&values))
    }

    /// The home screen icon of an app as PNG bytes.
    pub async fn app_icon(&self, bundle_id: &str) -> Result<Vec<u8>> {
        let mut springboard = self.service::<SpringBoardServicesClient>().await?;
        springboard
            .get_icon_pngdata(bundle_id.to_string())
            .await
            .map_err(|e| device_error(&format!("icon of {bundle_id}"), e))
    }

    /// Installs or upgrades an `.ipa`.
    ///
    /// Uploads it to `PublicStaging` (stage `upload`, bytes) then asks
    /// installation_proxy to install it (stage `install`, percent).
    pub async fn install_ipa(
        &self,
        ipa: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<()> {
        let progress: &mut (dyn FnMut(Progress) + Send) = &mut progress;
        if !tokio::fs::metadata(ipa).await?.is_file() {
            return Err(Error::InvalidInput(format!(
                "{} is not a file",
                ipa.display()
            )));
        }
        let package = format!("{PUBLIC_STAGING}/orchard-{}.ipa", uuid::Uuid::new_v4());
        let udid = &self.udid().0;
        let _staged = StagedPackage::register(udid, &package);
        let mut files = self.files(FileLocation::Media).await?;
        files.mkdir_all(PUBLIC_STAGING).await?;
        // Packages from interrupted installs cannot be cleaned up when their
        // future is dropped, so sweep them before staging a new one. Packages
        // of installs still running in this process are left alone.
        if let Ok(entries) = files.list_dir(PUBLIC_STAGING).await {
            for entry in entries.iter().filter(|e| is_stale_package(udid, e)) {
                let _ = files.remove(&entry.path, false).await;
            }
        }
        let result = self
            .upload_and_install(&mut files, ipa, &package, progress)
            .await;
        let _ = files.remove(&package, false).await;
        result?;
        progress(Progress::new("install", 100, Some(100)));
        Ok(())
    }

    async fn upload_and_install(
        &self,
        files: &mut DeviceFiles,
        ipa: &Path,
        package: &str,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<()> {
        files.upload_dyn(ipa, package, progress).await?;
        let report = Mutex::new(progress);
        let mut proxy = self.service::<InstallationProxyClient>().await?;
        proxy
            .upgrade_with_callback(
                package,
                None,
                |(percent, ())| {
                    if let Ok(mut report) = report.lock() {
                        report(Progress::new("install", percent, Some(100)));
                    }
                    std::future::ready(())
                },
                (),
            )
            .await
            .map_err(|e| device_error(&format!("install {}", ipa.display()), e))
    }

    /// Opens the App Store on the device at app `app_id`'s page, where its
    /// owner gets it with their own Apple ID. Needs a Wi-Fi pairing tunnel:
    /// launching apps is a CoreDevice service.
    pub async fn open_store_page(&self, app_id: u64) -> Result<()> {
        let stream = self
            .tunnel_connect("com.apple.coredevice.appservice")
            .await?;
        let mut apps = AppServiceClient::new(stream)
            .await
            .map_err(|e| device_error("app service", e))?;
        let mut options = Dictionary::new();
        // FrontBoard opens this URL in the launched app.
        options.insert(
            "__PayloadURL".into(),
            Value::String(format!("itms-apps://apps.apple.com/app/id{app_id}")),
        );
        apps.launch_application(
            "com.apple.AppStore",
            &[],
            true,
            false,
            None,
            Some(options),
            None,
        )
        .await
        .map_err(|e| device_error("open the App Store", e))?;
        Ok(())
    }

    pub async fn uninstall_app(&self, bundle_id: &str) -> Result<()> {
        let mut proxy = self.service::<InstallationProxyClient>().await?;
        proxy
            .uninstall(bundle_id, None)
            .await
            .map_err(|e| device_error(&format!("uninstall {bundle_id}"), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::FileKind;

    /// Shape of a `Browse` reply's `CurrentList`.
    const BROWSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>ApplicationType</key><string>User</string>
    <key>CFBundleDisplayName</key><string>Signal</string>
    <key>CFBundleIdentifier</key><string>org.whispersystems.signal</string>
    <key>CFBundleName</key><string>Signal</string>
    <key>CFBundleShortVersionString</key><string>7.30</string>
    <key>CFBundleVersion</key><string>7.30.0.12</string>
    <key>Container</key><string>/private/var/mobile/Containers/Data/Application/0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0</string>
    <key>DynamicDiskUsage</key><integer>412876800</integer>
    <key>Path</key><string>/private/var/containers/Bundle/Application/11111111-2222-3333-4444-555555555555/Signal.app</string>
    <key>StaticDiskUsage</key><integer>187654144</integer>
    <key>UIFileSharingEnabled</key><false/>
  </dict>
  <dict>
    <key>ApplicationType</key><string>System</string>
    <key>CFBundleIdentifier</key><string>com.apple.mobilesafari</string>
    <key>CFBundleName</key><string>Safari</string>
    <key>CFBundleShortVersionString</key><string>18.1</string>
    <key>CFBundleVersion</key><string>8620.1.16.10.11</string>
    <key>Path</key><string>/Applications/MobileSafari.app</string>
  </dict>
  <dict>
    <key>ApplicationType</key><string>User</string>
    <key>CFBundleIdentifier</key><string>com.example.notes</string>
    <key>UIFileSharingEnabled</key><true/>
  </dict>
  <dict>
    <key>ApplicationType</key><string>Internal</string>
    <key>CFBundleIdentifier</key><string>com.apple.Diagnostics</string>
    <key>CFBundleDisplayName</key><string></string>
    <key>CFBundleName</key><string>Diagnostics</string>
  </dict>
  <dict>
    <key>CFBundleName</key><string>No identifier</string>
  </dict>
</array>
</plist>"#;

    fn apps() -> Vec<AppInfo> {
        let values: Vec<Value> = plist::from_bytes(BROWSE.as_bytes()).unwrap();
        parse_app_list(&values)
    }

    #[test]
    fn parses_browse_results_sorted_by_name() {
        let apps = apps();
        let ids: Vec<_> = apps.iter().map(|a| a.bundle_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "com.example.notes",
                "com.apple.Diagnostics",
                "com.apple.mobilesafari",
                "org.whispersystems.signal"
            ]
        );
    }

    #[test]
    fn converts_all_fields() {
        let apps = apps();
        let signal = apps.iter().find(|a| a.name == "Signal").unwrap();
        assert_eq!(signal.kind, AppKind::User);
        assert_eq!(signal.version.as_deref(), Some("7.30.0.12"));
        assert_eq!(signal.short_version.as_deref(), Some("7.30"));
        assert_eq!(signal.static_size, Some(187_654_144));
        assert_eq!(signal.dynamic_size, Some(412_876_800));
        assert!(signal.path.as_deref().unwrap().ends_with("Signal.app"));
        assert!(!signal.file_sharing);

        let safari = apps.iter().find(|a| a.name == "Safari").unwrap();
        assert_eq!(safari.kind, AppKind::System);
        assert_eq!(safari.static_size, None);

        let notes = apps
            .iter()
            .find(|a| a.bundle_id == "com.example.notes")
            .unwrap();
        assert_eq!(notes.name, "com.example.notes");
        assert!(notes.file_sharing);

        let diag = apps
            .iter()
            .find(|a| a.bundle_id == "com.apple.Diagnostics")
            .unwrap();
        assert_eq!(diag.name, "Diagnostics");
        assert_eq!(diag.kind, AppKind::Other);
    }

    #[test]
    fn recognises_staged_packages() {
        assert!(is_orchard_package("orchard-6f1c.ipa"));
        assert!(is_orchard_package("orchard-6f1c.ipa.orchard-part"));
        assert!(!is_orchard_package("idevice.ipa"));
        assert!(!is_orchard_package("orchard-notes.txt"));
    }

    fn staged(path: &str) -> FileEntry {
        FileEntry {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            kind: FileKind::File,
            size: 1,
            modified: None,
            link_target: None,
        }
    }

    #[test]
    fn running_installs_are_not_swept() {
        let mine = "PublicStaging/orchard-1111.ipa";
        let guard = StagedPackage::register("DEV-A", mine);
        assert!(!is_stale_package(
            "DEV-A",
            &staged("/PublicStaging/orchard-1111.ipa")
        ));
        assert!(!is_stale_package(
            "DEV-A",
            &staged("/PublicStaging/orchard-1111.ipa.orchard-part")
        ));
        // Same path on another device, or another package: stale.
        assert!(is_stale_package(
            "DEV-B",
            &staged("/PublicStaging/orchard-1111.ipa")
        ));
        assert!(is_stale_package(
            "DEV-A",
            &staged("/PublicStaging/orchard-2222.ipa")
        ));
        assert!(!is_stale_package(
            "DEV-A",
            &staged("/PublicStaging/idevice.ipa")
        ));
        drop(guard);
        assert!(is_stale_package(
            "DEV-A",
            &staged("/PublicStaging/orchard-1111.ipa")
        ));
    }

    #[test]
    fn browse_options_request_sizes() {
        let options = browse_options(false);
        let options = options.as_dictionary().unwrap();
        assert_eq!(options["ApplicationType"].as_string(), Some("User"));
        let attrs = options["ReturnAttributes"].as_array().unwrap();
        assert!(
            attrs
                .iter()
                .any(|a| a.as_string() == Some("StaticDiskUsage"))
        );
        assert_eq!(
            browse_options(true).as_dictionary().unwrap()["ApplicationType"].as_string(),
            Some("Any")
        );
    }
}
