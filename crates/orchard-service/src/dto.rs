//! Data types exchanged with the frontend. Each mirrors a type in
//! `app/src/lib/api.ts` field for field.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Connection {
    Usb,
    Wifi,
}

/// What `import_pairing_file` stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingImport {
    pub kind: PairingKind,
    /// The device UDID (lockdown) or host identifier (Wi-Fi) the record is filed under.
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingKind {
    /// A classic lockdown record, as made over USB.
    Lockdown,
    /// An iOS 27 Wi-Fi (RPPairing) record.
    Wifi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub udid: String,
    pub name: String,
    /// Marketing name, e.g. "iPhone 15 Pro".
    pub model: String,
    /// Hardware identifier, e.g. "iPhone16,1".
    pub product_type: String,
    pub ios_version: String,
    pub connection: Connection,
    pub paired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    #[serde(flatten)]
    pub device: Device,
    pub build_version: String,
    pub serial_number: String,
    /// 0 to 100.
    pub battery_level: Option<u8>,
    pub battery_charging: bool,
    pub storage_total: u64,
    pub storage_used: u64,
    pub wifi_sync_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceEventKind {
    Attached,
    Detached,
    Updated,
}

/// Payload of `orchard://devices`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEvent {
    pub kind: DeviceEventKind,
    pub udid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    User,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub short_version: String,
    /// Static plus dynamic size in bytes.
    pub size: Option<u64>,
    pub kind: AppKind,
    /// The app exposes Documents through file sharing.
    pub file_sharing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpaInfo {
    pub path: String,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub short_version: String,
    pub minimum_os_version: Option<String>,
    pub encrypted: bool,
    pub size: u64,
    /// PNG data URL.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreAccount {
    pub email: String,
    pub name: String,
    /// Two letter storefront country code.
    pub country: String,
    pub backend: StoreBackend,
}

/// Which implementation handles App Store account operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoreBackend {
    Ipatool,
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreApp {
    pub app_id: u64,
    pub bundle_id: String,
    pub name: String,
    pub seller: String,
    /// 0 for free apps.
    pub price: f64,
    pub formatted_price: String,
    pub version: String,
    pub artwork_url: String,
    pub average_rating: Option<f64>,
    pub rating_count: u64,
    pub genre: String,
    pub size: Option<u64>,
    pub minimum_os_version: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreVersion {
    /// External version identifier used by the store.
    pub id: String,
    /// Display version when known.
    pub version: Option<String>,
}

/// Where a file path lives on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileSource {
    /// The media partition (AFC).
    Media,
    /// An app's Documents through house arrest `VendDocuments`.
    AppDocuments { bundle_id: String },
    /// An app's full container through house arrest `VendContainer`.
    AppContainer { bundle_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Absolute path inside the source, "/" separated.
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    /// Stable identifier, the backup directory name.
    pub id: String,
    pub udid: String,
    pub device_name: String,
    pub product_type: String,
    pub ios_version: String,
    /// Unix milliseconds of the last completed backup.
    pub date: i64,
    pub encrypted: bool,
    pub size: u64,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupMode {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DomainKind {
    System,
    App,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDomain {
    /// Raw domain, e.g. "HomeDomain" or "AppDomain-com.example.app".
    pub domain: String,
    pub kind: DomainKind,
    /// App name for app domains, domain otherwise.
    pub label: String,
    pub bundle_id: Option<String>,
    pub file_count: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    pub size: u64,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhotoKind {
    Image,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    /// Path on the device, e.g. "/DCIM/100APPLE/IMG_0001.HEIC".
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: Option<i64>,
    pub kind: PhotoKind,
}

/// Coarse content type of a file, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaCategory {
    Audio,
    Video,
    Book,
    Other,
}

/// An app that receives files through its shared Documents folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTarget {
    pub bundle_id: String,
    pub name: String,
    pub accepts: Vec<MediaCategory>,
    /// True when `accepts` comes from Orchard's table of known apps, false
    /// for the "any file" fallback.
    pub recognized: bool,
}

/// A file in an app's shared folder; `category` is `None` for folders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    #[serde(flatten)]
    pub entry: FileEntry,
    pub category: Option<MediaCategory>,
}

/// Totals of a `send_media` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendSummary {
    pub uploaded: u64,
    /// Already on the device with the same size.
    pub skipped: u64,
    pub bytes: u64,
    /// Device paths that got a numbered name because the name was taken.
    pub renamed: Vec<String>,
    pub skipped_symlinks: u64,
    pub skipped_invalid: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyslogLevel {
    Default,
    Info,
    Debug,
    Error,
    Fault,
    Notice,
}

/// Payload of `orchard://syslog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyslogLine {
    pub udid: String,
    /// Device timestamp as printed by the device.
    pub timestamp: String,
    pub process: String,
    pub pid: Option<u32>,
    pub level: SyslogLevel,
    pub message: String,
}

/// Payload of `orchard://syslog-status`: the device ended the stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyslogEnded {
    pub udid: String,
    /// Error text, `None` when the device closed the stream.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Running,
    Done,
    Error,
}

/// Payload of `orchard://task`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub task_id: String,
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
    pub status: TaskStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Default folder for downloads and exports, `None` to ask each time.
    pub download_dir: Option<String>,
    /// Where Orchard keeps its own data.
    pub data_dir: String,
    /// ipatool when installed (reliable sign-in), native otherwise.
    pub store_backend: StoreBackend,
    /// Privacy note to show at sign-in for this backend, if any.
    pub store_sign_in_note: Option<String>,
    /// Last `.p12` used for re-signing.
    pub signing_p12_path: Option<String>,
    /// Last `.mobileprovision` used for re-signing.
    pub signing_profile_path: Option<String>,
}

/// A loaded signing certificate, kept in app state under `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningIdentityInfo {
    pub id: String,
    pub path: String,
    pub common_name: Option<String>,
    pub team_id: Option<String>,
    /// Unix milliseconds.
    pub expires_at: i64,
}

/// A loaded provisioning profile, kept in app state under `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    pub id: String,
    pub path: String,
    pub name: String,
    pub team_id: String,
    pub team_name: Option<String>,
    pub application_identifier: String,
    /// Bundle id pattern, e.g. `com.example.*`.
    pub bundle_id_pattern: String,
    pub wildcard: bool,
    /// Provisioned device UDIDs.
    pub devices: Vec<String>,
    pub provisions_all_devices: bool,
    /// Unix milliseconds.
    pub expires_at: i64,
}

/// A problem that would make a re-signed app fail; mirrors
/// `orchard_ipa::ResignIssue` with times in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResignIssue {
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
    CertificateNotInProfile,
    BundleIdNotMatched {
        bundle_id: String,
        pattern: String,
    },
    DeviceNotProvisioned {
        udid: String,
    },
}

/// What a re-sign produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResignReport {
    pub bundle_id: String,
    pub team_id: String,
    /// Signed bundles, main app first.
    pub bundles: Vec<String>,
    /// Paths removed from the IPA (App Store metadata).
    pub removed: Vec<String>,
    pub output_size: u64,
    /// Where the re-signed IPA was written.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// Native window material behind the UI ("liquidGlass", "vibrancy" or
    /// "mica"), `None` when the window is opaque.
    pub material: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_source_matches_ts_shape() {
        let s: FileSource =
            serde_json::from_value(json!({"kind": "appDocuments", "bundleId": "a.b"})).unwrap();
        assert_eq!(
            s,
            FileSource::AppDocuments {
                bundle_id: "a.b".into()
            }
        );
        assert_eq!(
            serde_json::to_value(FileSource::Media).unwrap(),
            json!({"kind": "media"})
        );
    }

    #[test]
    fn device_info_flattens_device() {
        let info = DeviceInfo {
            device: Device {
                udid: "u".into(),
                name: "n".into(),
                model: "m".into(),
                product_type: "iPhone16,1".into(),
                ios_version: "18.0".into(),
                connection: Connection::Wifi,
                paired: true,
            },
            build_version: "b".into(),
            serial_number: "s".into(),
            battery_level: Some(50),
            battery_charging: false,
            storage_total: 2,
            storage_used: 1,
            wifi_sync_enabled: true,
        };
        let v = serde_json::to_value(info).unwrap();
        assert_eq!(v["productType"], "iPhone16,1");
        assert_eq!(v["connection"], "wifi");
        assert_eq!(v["wifiSyncEnabled"], true);
    }

    #[test]
    fn task_event_is_camel_case() {
        let v = serde_json::to_value(TaskEvent {
            task_id: "t".into(),
            stage: "upload".into(),
            done: 1,
            total: None,
            status: TaskStatus::Running,
            message: None,
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"taskId": "t", "stage": "upload", "done": 1, "total": null, "status": "running", "message": null})
        );
    }
}
