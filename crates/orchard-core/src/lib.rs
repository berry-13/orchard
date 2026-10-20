//! Shared types used across Orchard crates.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Unique device identifier as reported by usbmuxd / lockdownd.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Udid(pub String);

impl fmt::Display for Udid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Orchard's data directory: pairing records, the App Store account, backups.
///
/// `ORCHARD_DATA_DIR` overrides the platform default (for example
/// `~/.local/share/orchard` on Linux).
pub fn data_dir() -> PathBuf {
    std::env::var_os("ORCHARD_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("orchard")))
        .unwrap_or_else(|| PathBuf::from(".orchard"))
}

/// Writes a file only the current user can read (0600 from creation on Unix),
/// for secrets such as pairing records and App Store tokens.
#[cfg(unix)]
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

/// Progress of a long running operation (transfer, install, backup).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Short machine readable stage, e.g. `upload`, `install`, `download`.
    pub stage: String,
    /// Completed units, bytes when `total` is a byte count, percent otherwise.
    pub done: u64,
    /// Total units, `None` when unknown.
    pub total: Option<u64>,
}

impl Progress {
    pub fn new(stage: impl Into<String>, done: u64, total: Option<u64>) -> Self {
        Self {
            stage: stage.into(),
            done,
            total,
        }
    }
}

/// Why an App Store authentication attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The account has two-factor authentication; retry with the code.
    CodeRequired,
    /// Wrong Apple ID, password or code. Carries Apple's message.
    BadCredentials(String),
    /// The stored password token is no longer accepted; sign in again.
    SessionExpired,
    /// No signed-in account.
    SignedOut,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodeRequired => f.write_str("two-factor code required"),
            Self::BadCredentials(msg) => write!(f, "sign-in failed: {msg}"),
            Self::SessionExpired => f.write_str("session expired, sign in again"),
            Self::SignedOut => f.write_str("not signed in"),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    NotImplemented(&'static str),
    Io(std::io::Error),
    /// Transport or service failure while talking to a device.
    Device(String),
    /// Malformed data from a device, a server or a file.
    Protocol(String),
    InvalidInput(String),
    NotFound(String),
    /// HTTP or connectivity failure talking to Apple's servers.
    Network(String),
    Auth(AuthError),
    /// The account does not own the app; it has to be purchased (free apps too).
    LicenseRequired,
    /// Any other App Store failure, with Apple's message.
    Store(String),
    /// The backup is encrypted and the password is missing or wrong.
    BackupPassword,
}

impl Error {
    /// Stable machine readable kind, used by the app to pick UI states.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "notImplemented",
            Self::Io(_) => "io",
            Self::Device(_) => "device",
            Self::Protocol(_) => "protocol",
            Self::InvalidInput(_) => "invalidInput",
            Self::NotFound(_) => "notFound",
            Self::Network(_) => "network",
            Self::Auth(AuthError::CodeRequired) => "authCodeRequired",
            Self::Auth(AuthError::BadCredentials(_)) => "authBadCredentials",
            Self::Auth(AuthError::SessionExpired) => "authSessionExpired",
            Self::Auth(AuthError::SignedOut) => "authSignedOut",
            Self::LicenseRequired => "licenseRequired",
            Self::Store(_) => "store",
            Self::BackupPassword => "backupPassword",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented(what) => write!(f, "not implemented: {what}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Device(msg)
            | Self::Protocol(msg)
            | Self::InvalidInput(msg)
            | Self::Network(msg)
            | Self::Store(msg) => f.write_str(msg),
            Self::NotFound(what) => write!(f, "not found: {what}"),
            Self::Auth(e) => e.fmt(f),
            Self::LicenseRequired => f.write_str("this Apple ID does not own the app yet"),
            Self::BackupPassword => f.write_str("backup password is missing or wrong"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<AuthError> for Error {
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_have_distinct_kinds() {
        assert_eq!(
            Error::from(AuthError::CodeRequired).kind(),
            "authCodeRequired"
        );
        assert_eq!(Error::LicenseRequired.kind(), "licenseRequired");
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orchard-core-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn progress_serializes_camel_case() {
        let json = serde_json::to_string(&Progress::new("upload", 5, Some(10))).unwrap();
        assert_eq!(json, r#"{"stage":"upload","done":5,"total":10}"#);
    }
}
