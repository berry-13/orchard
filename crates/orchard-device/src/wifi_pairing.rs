//! Wi-Fi pairing (RPPairing), the pairing iOS 27 starts on the device itself:
//! a host announces `_remotepairing-pairable-host._tcp` on the local network,
//! the user taps "Pair with ..." in Developer Mode settings and types the PIN
//! the host shows. [`PairingSession`] is that host; records also come from
//! apps such as StikPair. A record holds this host's identity for the device's
//! remote pairing service, not a lockdown pairing record.

use std::fs;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use idevice::ReadWrite;
use idevice::remote_pairing::{
    PAIRABLE_HOST_SERVICE_TYPE, PairableHost, PairableHostInfo, PeerDevice, RpPairingFile,
    RpPairingSocket,
};
use orchard_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::error::device_error;

/// A stored Wi-Fi pairing, named by the host identifier inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiPairing {
    pub identifier: String,
}

/// The device a [`PairingSession`] paired with, saved next to its record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub name: String,
    pub model: String,
    pub udid: String,
    /// The pairing record it belongs to.
    pub identifier: String,
    /// Where the device was last reached, set by "Add Wi-Fi device".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
    /// The port its remote pairing service answered on there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// How a pairing host must be announced over Bonjour so the device lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairingAnnouncement {
    /// `_remotepairing-pairable-host._tcp`
    pub service_type: String,
    /// The service instance name, which is the record's host identifier.
    pub service_name: String,
    pub txt: Vec<(String, String)>,
}

/// One device-initiated pairing: announce [`Self::announcement`] on the
/// device's network, then hand the connection the device opens to
/// [`Self::accept`].
pub struct PairingSession {
    info: PairableHostInfo,
    file: RpPairingFile,
}

impl PairingSession {
    /// `name` is what the device shows as "Pair with <name>".
    pub fn new(name: &str) -> Self {
        // iOS lists only computers, so the model must be a Mac identifier.
        Self {
            info: PairableHostInfo::generate(name, "Mac17,7"),
            file: RpPairingFile::generate(name),
        }
    }

    pub fn announcement(&self) -> PairingAnnouncement {
        let service_name = self.file.identifier().to_owned();
        PairingAnnouncement {
            service_type: PAIRABLE_HOST_SERVICE_TYPE
                .trim_end_matches(".local.")
                .to_owned(),
            txt: self.info.mdns_txt_records(&service_name),
            service_name,
        }
    }

    /// Runs the pairing over `stream`, the connection the device opened.
    /// `on_pin` gets the six digit code to show; the user types it on the
    /// device. The record is saved to `store` on success.
    pub async fn accept<S: ReadWrite + 'static>(
        mut self,
        stream: S,
        store: &WifiPairingStore,
        on_pin: impl FnOnce(String) + Send,
    ) -> Result<PairedDevice> {
        let mut host = PairableHost::new(RpPairingSocket::new_device(stream), self.info.clone());
        let peer = host
            .accept(&mut self.file, |pin| {
                on_pin(pin);
                async {}
            })
            .await
            .map_err(|e| device_error("Wi-Fi pairing", e))?;
        let device = PairedDevice {
            name: peer.name,
            model: peer.model,
            udid: peer.remotepairing_udid,
            identifier: self.file.identifier().to_owned(),
            address: None,
            port: None,
        };
        store.save(&self.file, &device)?;
        Ok(device)
    }
}

/// Wi-Fi pairing records, one `<identifier>.plist` each under
/// `<data_dir>/wifi-pairing`.
#[derive(Debug, Clone)]
pub struct WifiPairingStore {
    dir: PathBuf,
}

/// Whether `bytes` parse as a Wi-Fi pairing record.
pub fn is_wifi_pairing(bytes: &[u8]) -> bool {
    RpPairingFile::from_bytes(bytes).is_ok()
}

impl WifiPairingStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("wifi-pairing"),
        }
    }

    fn path_for(&self, identifier: &str) -> Result<PathBuf> {
        let valid = !identifier.is_empty()
            && identifier.len() <= 128
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && !identifier.starts_with('.');
        if !valid {
            return Err(Error::InvalidInput(format!(
                "invalid pairing identifier {identifier:?}"
            )));
        }
        Ok(self.dir.join(format!("{identifier}.plist")))
    }

    /// Checks `bytes` parse and stores them, replacing a record with the same
    /// identifier. Readable by the current user only on Unix.
    pub fn import(&self, bytes: &[u8]) -> Result<WifiPairing> {
        let record = RpPairingFile::from_bytes(bytes)
            .map_err(|e| device_error("parse Wi-Fi pairing record", e))?;
        let identifier = record.identifier().to_owned();
        let path = self.path_for(&identifier)?;
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("plist.tmp");
        orchard_core::write_private(&tmp, &record.to_bytes())?;
        fs::rename(&tmp, &path)?;
        Ok(WifiPairing { identifier })
    }

    /// Saves a record made by [`PairingSession`] with the device it pairs.
    fn save(&self, record: &RpPairingFile, device: &PairedDevice) -> Result<()> {
        let path = self.path_for(record.identifier())?;
        fs::create_dir_all(&self.dir)?;
        self.save_device(device)?;
        let tmp = path.with_extension("plist.tmp");
        orchard_core::write_private(&tmp, &record.to_bytes())?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// The device a record pairs, when it was made by [`PairingSession`].
    pub fn device(&self, identifier: &str) -> Result<Option<PairedDevice>> {
        let path = self.path_for(identifier)?.with_extension("json");
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Error::Io(e.into())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save_device(&self, device: &PairedDevice) -> Result<()> {
        let path = self.path_for(&device.identifier)?.with_extension("json");
        let meta = serde_json::to_vec_pretty(device).map_err(|e| Error::Io(e.into()))?;
        orchard_core::write_private(&path, &meta)
    }

    /// Remembers where `device` answers, so it is listed and reached there.
    pub fn set_route(&self, device: &PairedDevice, address: IpAddr, port: u16) -> Result<()> {
        self.save_device(&PairedDevice {
            address: Some(address),
            port: Some(port),
            ..device.clone()
        })
    }

    /// Every device paired by [`PairingSession`], with its record.
    pub fn devices(&self) -> Result<Vec<(PairedDevice, RpPairingFile)>> {
        let mut devices = Vec::new();
        for pairing in self.list()? {
            if let Some(device) = self.device(&pairing.identifier)?
                && let Some(record) = self.load(&pairing.identifier)?
            {
                devices.push((device, record));
            }
        }
        Ok(devices)
    }

    /// The device paired with one of these records that published this
    /// `_remotepairing._tcp` advertisement, by its `identifier` and `authTag`
    /// TXT values. Other devices on the network advertise too.
    pub fn advertised(&self, identifier: &str, auth_tag: &str) -> Result<Option<PairedDevice>> {
        Ok(self
            .devices()?
            .into_iter()
            .find(|(_, record)| {
                record
                    .alt_irk()
                    .is_some_and(|irk| PeerDevice::validate_auth_tag(irk, identifier, auth_tag))
            })
            .map(|(device, _)| device))
    }

    /// The device with `udid` and its record.
    pub fn find(&self, udid: &str) -> Result<Option<(PairedDevice, RpPairingFile)>> {
        Ok(self.devices()?.into_iter().find(|(d, _)| d.udid == udid))
    }

    pub fn load(&self, identifier: &str) -> Result<Option<RpPairingFile>> {
        let path = self.path_for(identifier)?;
        match fs::read(&path) {
            Ok(bytes) => RpPairingFile::from_bytes(&bytes)
                .map(Some)
                .map_err(|e| device_error(&format!("pairing record {}", path.display()), e)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list(&self) -> Result<Vec<WifiPairing>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut list: Vec<WifiPairing> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let identifier = name.strip_suffix(".plist")?.to_owned();
                Some(WifiPairing { identifier })
            })
            .collect();
        list.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_lists_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let store = WifiPairingStore::new(dir.path());
        let record = RpPairingFile::generate("orchard-test");
        let bytes = record.to_bytes();
        assert!(is_wifi_pairing(&bytes));

        let imported = store.import(&bytes).unwrap();
        assert_eq!(imported.identifier, record.identifier());
        assert_eq!(store.list().unwrap(), vec![imported.clone()]);
        let loaded = store.load(&imported.identifier).unwrap().unwrap();
        assert_eq!(loaded.public_key_bytes(), record.public_key_bytes());
    }

    #[test]
    fn recognizes_the_paired_device_advertising() {
        use base64::Engine;
        use idevice::remote_pairing::compute_auth_tag;

        let dir = tempfile::tempdir().unwrap();
        let store = WifiPairingStore::new(dir.path());
        let mut record = RpPairingFile::generate("orchard-test");
        let irk = [7u8; 16];
        record.alt_irk = Some(irk.to_vec());
        let device = PairedDevice {
            name: "iPhone".into(),
            model: "iPhone19,2".into(),
            udid: "U1".into(),
            identifier: record.identifier().to_owned(),
            address: None,
            port: None,
        };
        store.save(&record, &device).unwrap();

        let tag =
            base64::engine::general_purpose::STANDARD.encode(compute_auth_tag(&irk, "SERVICE-ID"));
        let found = store.advertised("SERVICE-ID", &tag).unwrap();
        assert_eq!(found.map(|d| d.udid).as_deref(), Some("U1"));
        assert!(store.advertised("OTHER-ID", &tag).unwrap().is_none());
        assert!(
            store
                .advertised("SERVICE-ID", "not base64")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_other_files_and_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = WifiPairingStore::new(dir.path());
        assert!(store.import(b"not a plist").is_err());
        assert!(!is_wifi_pairing(b"<plist></plist>"));
        assert!(store.load("../escape").is_err());
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    // The pairing itself runs against a real device: idevice has no client
    // that opens the way a device does (handshake without pair-verify).
    #[test]
    fn announces_what_the_device_looks_for() {
        let session = PairingSession::new("Orchard");
        let a = session.announcement();
        assert_eq!(a.service_type, "_remotepairing-pairable-host._tcp");
        let txt: std::collections::HashMap<_, _> = a.txt.iter().cloned().collect();
        assert_eq!(txt["name"], "Orchard");
        assert_eq!(txt["identifier"], a.service_name);
        assert!(txt["model"].starts_with("Mac"), "iOS lists only computers");
        assert!(!txt["authTag"].is_empty());
    }
}
