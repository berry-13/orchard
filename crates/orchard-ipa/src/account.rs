use std::path::Path;

use orchard_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// A signed-in App Store account. The caller persists it with [`Account::save`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Apple ID used to sign in.
    pub email: String,
    /// Account holder name, as reported by Apple.
    pub name: String,
    /// Directory services id (`dsPersonId`).
    pub ds_person_id: String,
    /// Token sent as `X-Token` on purchases.
    pub password_token: String,
    /// Store front header value, e.g. `143441-1,29`.
    pub store_front: String,
    /// Store pod the account lives on, when Apple reports one.
    #[serde(default)]
    pub pod: Option<String>,
    /// Machine GUID sent with every request; keep it stable across sign-ins.
    pub guid: String,
}

impl Account {
    /// Lowercase country code of the account's store, when known.
    pub fn country_code(&self) -> Option<&'static str> {
        crate::country_for_store_front(&self.store_front)
    }

    /// Reads an account saved with [`Account::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(path.display().to_string()),
            _ => Error::Io(e),
        })?;
        serde_json::from_slice(&data).map_err(|e| crate::protocol("invalid account file", e))
    }

    /// Writes the account as JSON, readable by the owner only on Unix.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let data =
            serde_json::to_vec_pretty(self).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        write_private(&tmp, &data)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub(crate) use orchard_core::write_private;

/// Generates a random machine GUID: 12 uppercase hex characters, like a MAC address.
pub fn generate_guid() -> String {
    let mut bytes = [0u8; 6];
    getrandom::getrandom(&mut bytes).expect("system random source unavailable");
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_is_twelve_uppercase_hex() {
        let guid = generate_guid();
        assert_eq!(guid.len(), 12);
        assert!(
            guid.chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        );
        assert_ne!(guid, generate_guid());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/account.json");
        let account = Account {
            email: "a@b.c".into(),
            name: "A B".into(),
            ds_person_id: "123".into(),
            password_token: "tok".into(),
            store_front: "143441-1,29".into(),
            pod: Some("25".into()),
            guid: generate_guid(),
        };
        account.save(&path).unwrap();
        assert_eq!(Account::load(&path).unwrap(), account);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(matches!(
            Account::load(dir.path().join("missing.json")),
            Err(Error::NotFound(_))
        ));
    }
}
