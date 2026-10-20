//! Wi-Fi pairing records (RPPairing) from the pairing iOS 27 starts on the
//! device itself ("Pair with ..." in Developer Mode settings), as exported by
//! apps such as StikPair. They hold this host's identity for the device's
//! remote pairing service, not a lockdown pairing record.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use idevice::remote_pairing::RpPairingFile;
use orchard_core::{Error, Result};

use crate::error::device_error;

/// A stored Wi-Fi pairing, named by the host identifier inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiPairing {
    pub identifier: String,
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
    fn rejects_other_files_and_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = WifiPairingStore::new(dir.path());
        assert!(store.import(b"not a plist").is_err());
        assert!(!is_wifi_pairing(b"<plist></plist>"));
        assert!(store.load("../escape").is_err());
    }
}
