use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use idevice::IdeviceError;
use idevice::pairing_file::PairingFile;
use orchard_core::{Error, Result, Udid};
use serde::{Deserialize, Serialize};

use crate::connect::{DeviceConnection, Route};
use crate::error::device_error;
use crate::mux::mux_connect;

const WIRELESS_DOMAIN: &str = "com.apple.mobile.wireless_lockdown";
const WIFI_KEY: &str = "EnableWifiConnections";

/// Result of a pairing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairOutcome {
    /// Paired; the record was saved.
    Paired,
    /// The Trust dialog is still waiting for the user.
    TrustPending,
    /// The user tapped Don't Trust.
    Denied,
    /// The device is locked; unlock it and retry.
    PasscodeRequired,
}

/// Summary of a stored pairing record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPairing {
    pub udid: Udid,
    pub host_id: String,
    /// Device Wi-Fi MAC, used to match mDNS announcements.
    pub wifi_mac_address: String,
}

/// Pairing records kept by Orchard, one `<udid>.plist` per device under
/// `<data_dir>/pairing`.
#[derive(Debug, Clone)]
pub struct PairingStore {
    dir: PathBuf,
}

impl PairingStore {
    /// Uses `<data_dir>/pairing`. The directory is created on first save.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("pairing"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, udid: &Udid) -> Result<PathBuf> {
        let valid = !udid.0.is_empty()
            && udid
                .0
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !valid {
            return Err(Error::InvalidInput(format!("invalid UDID {udid:?}")));
        }
        Ok(self.dir.join(format!("{}.plist", udid.0)))
    }

    /// Saves `record` for `udid`, replacing any previous one. Readable by the
    /// current user only on Unix.
    pub fn save(&self, udid: &Udid, record: &PairingFile) -> Result<()> {
        let path = self.path_for(udid)?;
        let mut record = record.clone();
        record.udid.get_or_insert_with(|| udid.0.clone());
        let bytes = record
            .serialize()
            .map_err(|e| device_error("serialize pairing record", e))?;
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("plist.tmp");
        orchard_core::write_private(&tmp, &bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Imports a pairing plist from elsewhere (for example a usbmuxd or
    /// iTunes `Lockdown` record) after checking it parses.
    pub fn import(&self, udid: &Udid, bytes: &[u8]) -> Result<PairingFile> {
        let record =
            PairingFile::from_bytes(bytes).map_err(|e| device_error("parse pairing record", e))?;
        self.save(udid, &record)?;
        Ok(record)
    }

    /// Loads the record for `udid`, `None` if there is none.
    pub fn load(&self, udid: &Udid) -> Result<Option<PairingFile>> {
        let path = self.path_for(udid)?;
        match fs::read(&path) {
            Ok(bytes) => PairingFile::from_bytes(&bytes)
                .map(Some)
                .map_err(|e| device_error(&format!("pairing record {}", path.display()), e)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes the record for `udid`. Returns whether one existed.
    pub fn remove(&self, udid: &Udid) -> Result<bool> {
        match fs::remove_file(self.path_for(udid)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Lists stored records, skipping files that do not parse.
    pub fn list(&self) -> Result<Vec<StoredPairing>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "plist") {
                continue;
            }
            let Some(udid) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(record) = PairingFile::read_from_file(&path) else {
                continue;
            };
            out.push(StoredPairing {
                udid: Udid(udid.to_string()),
                host_id: record.host_id,
                wifi_mac_address: record.wifi_mac_address,
            });
        }
        out.sort_by(|a, b| a.udid.0.cmp(&b.udid.0));
        Ok(out)
    }
}

impl DeviceConnection {
    /// Pairs with the device over USB, showing the Trust dialog.
    ///
    /// Waits up to `wait` for the user to answer, resending the same request.
    /// Keep `wait` long (30 to 60 seconds): a later call starts a new request.
    /// On success the record is saved to `store` and handed to usbmuxd.
    pub async fn pair(
        &mut self,
        store: &PairingStore,
        host_name: &str,
        wait: Duration,
    ) -> Result<PairOutcome> {
        let Route::Mux { addr, .. } = self.provider.route.clone() else {
            return Err(Error::InvalidInput("pairing needs a USB connection".into()));
        };
        let system_buid = mux_connect(&addr)
            .await?
            .get_buid()
            .await
            .map_err(|e| device_error("usbmuxd buid", e))?;
        let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();

        let mut lockdown = self.lockdown_plain().await?;
        let attempt =
            tokio::time::timeout(wait, lockdown.pair(host_id, system_buid, Some(host_name))).await;
        let mut record = match attempt {
            Err(_) | Ok(Err(IdeviceError::PairingDialogResponsePending)) => {
                return Ok(PairOutcome::TrustPending);
            }
            Ok(Err(IdeviceError::UserDeniedPairing)) => return Ok(PairOutcome::Denied),
            Ok(Err(IdeviceError::PasswordProtected)) => return Ok(PairOutcome::PasscodeRequired),
            Ok(Err(e)) => return Err(device_error("pair", e)),
            Ok(Ok(record)) => record,
        };
        record.udid = Some(self.udid().0.clone());
        store.save(self.udid(), &record)?;
        if let Ok(bytes) = record.clone().serialize()
            && let Ok(mut mux) = mux_connect(&addr).await
        {
            let _ = mux.save_pair_record(&self.udid().0, bytes).await;
        }
        self.provider.pairing = Some(record);
        Ok(PairOutcome::Paired)
    }

    /// Checks that the current record is accepted by the device.
    pub async fn is_paired(&self) -> Result<bool> {
        let Some(pairing) = &self.provider.pairing else {
            return Ok(false);
        };
        let mut lockdown = self.lockdown_plain().await?;
        match lockdown.start_session(pairing).await {
            Ok(_) => Ok(true),
            Err(IdeviceError::InvalidHostID | IdeviceError::Rustls(_)) => Ok(false),
            Err(e) => Err(device_error("validate pairing", e)),
        }
    }

    /// Copies the record usbmuxd holds for this device into `store`.
    pub async fn import_usbmuxd_record(&self, store: &PairingStore) -> Result<()> {
        let Route::Mux { addr, .. } = &self.provider.route else {
            return Err(Error::InvalidInput("not a usbmuxd connection".into()));
        };
        let record = crate::mux::read_pair_record(addr, self.udid())
            .await?
            .ok_or_else(|| Error::NotFound(format!("usbmuxd pair record for {}", self.udid())))?;
        store.save(self.udid(), &record)
    }

    /// Removes this host's pairing from the device, `store` and usbmuxd.
    pub async fn unpair(&mut self, store: &PairingStore) -> Result<()> {
        let host_id = self.require_pairing()?.host_id.clone();
        let mut lockdown = self.lockdown_plain().await?;
        lockdown
            .unpair(host_id)
            .await
            .map_err(|e| device_error("unpair", e))?;
        store.remove(self.udid())?;
        if let Route::Mux { addr, .. } = &self.provider.route
            && let Ok(mut mux) = mux_connect(addr).await
        {
            let _ = mux.delete_pair_record(&self.udid().0).await;
        }
        self.provider.pairing = None;
        Ok(())
    }

    /// Turns the device's Wi-Fi lockdown (Wi-Fi sync and Wi-Fi connections) on or off.
    pub async fn set_wifi_connections(&self, enabled: bool) -> Result<()> {
        let mut lockdown = self.lockdown().await?;
        lockdown
            .set_value(
                WIFI_KEY,
                plist::Value::Boolean(enabled),
                Some(WIRELESS_DOMAIN),
            )
            .await
            .map_err(|e| device_error("enable Wi-Fi connections", e))
    }

    /// Whether Wi-Fi connections are enabled on the device.
    pub async fn wifi_connections_enabled(&self) -> Result<bool> {
        let mut lockdown = self.lockdown().await?;
        let value = lockdown
            .get_value(Some(WIFI_KEY), Some(WIRELESS_DOMAIN))
            .await
            .map_err(|e| device_error("read Wi-Fi connections", e))?;
        Ok(value.as_boolean().unwrap_or(false))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const PEM: &str = "-----BEGIN CERTIFICATE-----\nAAECAwQFBgcICQ==\n-----END CERTIFICATE-----\n";
    const KEY: &str = "-----BEGIN PRIVATE KEY-----\nAAECAwQFBgcICQ==\n-----END PRIVATE KEY-----\n";

    /// A structurally valid pairing plist with placeholder key material.
    pub(crate) fn fixture(host_id: &str, mac: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>DeviceCertificate</key><data>{cert}</data>
  <key>HostCertificate</key><data>{cert}</data>
  <key>RootCertificate</key><data>{cert}</data>
  <key>HostPrivateKey</key><data>{key}</data>
  <key>RootPrivateKey</key><data>{key}</data>
  <key>SystemBUID</key><string>5B4A1F0C-31C1-4C1E-9D25-1F0D7C3E5A11</string>
  <key>HostID</key><string>{host_id}</string>
  <key>EscrowBag</key><data>AQID</data>
  <key>WiFiMACAddress</key><string>{mac}</string>
</dict>
</plist>"#,
            cert = b64(PEM.as_bytes()),
            key = b64(KEY.as_bytes()),
        )
        .into_bytes()
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn round_trips_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let udid = Udid("00008110-000A1B2C3D4E5F60".into());
        assert!(store.load(&udid).unwrap().is_none());
        assert!(store.list().unwrap().is_empty());

        let imported = store
            .import(&udid, &fixture("HOST-1", "aa:bb:cc:dd:ee:ff"))
            .unwrap();
        let loaded = store.load(&udid).unwrap().unwrap();
        assert_eq!(loaded.host_id, "HOST-1");
        assert_eq!(loaded.udid.as_deref(), Some(udid.0.as_str()));
        assert_eq!(loaded.device_certificate, imported.device_certificate);
        assert_eq!(loaded.host_private_key, imported.host_private_key);
        assert_eq!(loaded.escrow_bag, Some(vec![1, 2, 3]));
        assert!(
            dir.path()
                .join("pairing")
                .join(format!("{}.plist", udid.0))
                .exists()
        );

        assert_eq!(
            store.list().unwrap(),
            vec![StoredPairing {
                udid: udid.clone(),
                host_id: "HOST-1".into(),
                wifi_mac_address: "aa:bb:cc:dd:ee:ff".into(),
            }]
        );
        assert!(store.remove(&udid).unwrap());
        assert!(!store.remove(&udid).unwrap());
        assert!(store.load(&udid).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn records_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let udid = Udid("ABC".into());
        store.import(&udid, &fixture("H", "m")).unwrap();
        let mode = fs::metadata(store.dir().join("ABC.plist"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn rejects_path_like_udids() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        for bad in ["../etc", "", "a/b", "a.b"] {
            assert_eq!(
                store.load(&Udid(bad.into())).unwrap_err().kind(),
                "invalidInput"
            );
        }
    }

    #[test]
    fn garbage_is_not_imported() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        assert!(store.import(&Udid("A".into()), b"not a plist").is_err());
        assert!(store.list().unwrap().is_empty());
    }
}
