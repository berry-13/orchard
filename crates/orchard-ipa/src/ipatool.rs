//! Bridge to the `ipatool` CLI for App Store sign-in, purchase and download.
//!
//! Apple now requires sign-in requests signed by its own `storeagent` code,
//! which ipatool runs under emulation. Until Orchard does the same, the
//! account-bound operations go through ipatool; search and lookup stay on
//! [`crate::StoreClient`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use orchard_core::{AuthError, Error, Progress, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{AppVersions, LookupKey};

/// Environment variable that overrides the ipatool binary location.
const BINARY_ENV: &str = "ORCHARD_IPATOOL";
const PASSPHRASE_FILE: &str = "ipatool-passphrase";

/// An app to operate on, by App Store id (`-i`) or bundle id (`-b`).
pub type AppRef = LookupKey;

/// Exposure of secrets passed to ipatool, for the app to show before sign-in.
///
/// ipatool reads the Apple ID password and the keychain passphrase only from
/// flags or from a masked prompt that requires a terminal; it has no
/// environment variable, file or piped stdin option. Both therefore travel on
/// the command line, and on Linux `/proc/<pid>/cmdline` is readable by every
/// local user (unless `/proc` is mounted with `hidepid`) while the process runs.
pub const ARGV_EXPOSURE_NOTE: &str = "Your Apple ID password is passed to ipatool on its command \
line. While sign-in runs, other users on this computer can see it in the process list.";

/// The account ipatool is signed in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreIdentity {
    pub email: String,
    pub name: String,
}

/// A located ipatool binary plus the directory holding its keychain passphrase.
///
/// ipatool keeps the account in its own keychain (`~/.ipatool` file backend on
/// Linux and Windows), unlocked with a passphrase Orchard generates once and
/// stores at `<data_dir>/ipatool-passphrase`. The passphrase is passed on
/// every call's command line; see [`ARGV_EXPOSURE_NOTE`].
#[derive(Debug, Clone)]
pub struct Ipatool {
    binary: PathBuf,
    data_dir: PathBuf,
}

/// Parsed JSON log lines of one ipatool run.
struct Run {
    lines: Vec<serde_json::Map<String, Value>>,
}

impl Run {
    fn success(&self) -> Option<&serde_json::Map<String, Value>> {
        self.lines
            .iter()
            .find(|l| l.get("success").and_then(Value::as_bool) == Some(true))
    }

    fn error(&self) -> Option<String> {
        self.lines
            .iter()
            .filter(|l| l.get("success").and_then(Value::as_bool) == Some(false))
            .find_map(|l| l.get("error").and_then(Value::as_str).map(str::to_owned))
    }

    fn has_message(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| {
            l.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains(needle))
        })
    }
}

fn str_field(line: &serde_json::Map<String, Value>, key: &str) -> String {
    line.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Maps an ipatool error message to the closest Orchard error.
fn classify(message: String) -> Error {
    let lower = message.to_lowercase();
    if lower.contains("license is required") {
        Error::LicenseRequired
    } else if lower.contains("password token is expired") || lower.contains("token expired") {
        AuthError::SessionExpired.into()
    } else if lower.contains("failed to get account") {
        AuthError::SignedOut.into()
    } else {
        Error::Store(message)
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_owned()]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

impl Ipatool {
    /// Finds ipatool via `ORCHARD_IPATOOL`, then `PATH`.
    pub fn detect(data_dir: &Path) -> Option<Self> {
        let binary = std::env::var_os(BINARY_ENV)
            .map(PathBuf::from)
            .filter(|p| is_executable(p))
            .or_else(|| find_on_path("ipatool"))?;
        Some(Self::with_binary(binary, data_dir))
    }

    /// Uses a specific ipatool binary.
    pub fn with_binary(binary: impl Into<PathBuf>, data_dir: &Path) -> Self {
        Self {
            binary: binary.into(),
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Path of the ipatool binary in use.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// The ipatool version string, e.g. `2.2.0`.
    pub async fn version(&self) -> Result<String> {
        let out = self.command([OsString::from("--version")]).output().await?;
        if !out.status.success() {
            return Err(Error::Store(format!(
                "ipatool --version failed: {}",
                out.status
            )));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let text = text.trim();
        Ok(text
            .rsplit_once(' ')
            .map_or(text, |(_, version)| version)
            .to_owned())
    }

    /// Reads the keychain passphrase, creating it on first use.
    pub fn passphrase(&self) -> Result<String> {
        let path = self.data_dir.join(PASSPHRASE_FILE);
        match std::fs::read_to_string(&path) {
            Ok(existing) if !existing.trim().is_empty() => return Ok(existing.trim().to_owned()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut bytes = [0u8; 24];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let passphrase: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::create_dir_all(&self.data_dir)?;
        crate::account::write_private(&path, passphrase.as_bytes())?;
        Ok(passphrase)
    }

    fn command(&self, args: impl IntoIterator<Item = OsString>) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.binary);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Runs a subcommand in JSON, non-interactive mode and collects its log lines.
    async fn run(&self, args: &[&str]) -> Result<Run> {
        let mut argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        argv.extend(
            [
                "--format",
                "json",
                "--non-interactive",
                "--keychain-passphrase",
            ]
            .map(OsString::from),
        );
        argv.push(self.passphrase()?.into());
        let out = self
            .command(argv)
            .output()
            .await
            .map_err(|e| Error::Store(format!("cannot run {}: {e}", self.binary.display())))?;
        let lines = [&out.stdout, &out.stderr]
            .into_iter()
            .flat_map(|stream| stream.split(|b| *b == b'\n'))
            .filter_map(|line| serde_json::from_slice::<serde_json::Map<String, Value>>(line).ok())
            .collect();
        Ok(Run { lines })
    }

    /// Runs a subcommand and returns its success line, mapping failures.
    async fn run_ok(&self, args: &[&str]) -> Result<serde_json::Map<String, Value>> {
        let run = self.run(args).await?;
        if let Some(line) = run.success() {
            return Ok(line.clone());
        }
        Err(classify(run.error().unwrap_or_else(|| {
            format!("ipatool {} produced no result", args.join(" "))
        })))
    }

    fn app_args(app: &AppRef) -> [String; 2] {
        match app {
            LookupKey::Id(id) => ["-i".into(), id.to_string()],
            LookupKey::BundleId(bundle_id) => ["-b".into(), bundle_id.clone()],
        }
    }

    /// Signs ipatool in. `code` is the two-factor code.
    ///
    /// The password is passed as a command line argument, since ipatool has
    /// no other non-interactive input for it. While the call runs it is
    /// readable by all local users on Linux; see [`ARGV_EXPOSURE_NOTE`].
    pub async fn login(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<StoreIdentity> {
        let mut args = vec!["auth", "login", "-e", email, "-p", password];
        if let Some(code) = code {
            args.extend(["--auth-code", code]);
        }
        let run = self.run(&args).await?;
        if let Some(line) = run.success() {
            return Ok(StoreIdentity {
                email: str_field(line, "email"),
                name: str_field(line, "name"),
            });
        }
        if run.has_message("2FA code is required") {
            return Err(AuthError::CodeRequired.into());
        }
        let message = run
            .error()
            .unwrap_or_else(|| "ipatool auth login produced no result".into());
        let lower = message.to_lowercase();
        if ["password", "credential", "login", "2fa", "verification"]
            .iter()
            .any(|w| lower.contains(w))
        {
            return Err(AuthError::BadCredentials(message).into());
        }
        Err(classify(message))
    }

    /// The signed-in account, `None` when ipatool is signed out.
    pub async fn info(&self) -> Result<Option<StoreIdentity>> {
        match self.run_ok(&["auth", "info"]).await {
            Ok(line) => Ok(Some(StoreIdentity {
                email: str_field(&line, "email"),
                name: str_field(&line, "name"),
            })),
            Err(Error::Auth(AuthError::SignedOut)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Signs ipatool out.
    pub async fn logout(&self) -> Result<()> {
        self.run_ok(&["auth", "revoke"]).await.map(|_| ())
    }

    /// Acquires a free app; succeeds when it is already owned.
    pub async fn purchase(&self, app: &AppRef) -> Result<()> {
        let [flag, value] = Self::app_args(app);
        self.run_ok(&["purchase", &flag, &value]).await.map(|_| ())
    }

    /// Lists external version ids. ipatool does not report which is current;
    /// `latest` is the last id, as Apple orders them oldest first.
    pub async fn list_versions(&self, app: &AppRef) -> Result<AppVersions> {
        let [flag, value] = Self::app_args(app);
        let line = self.run_ok(&["list-versions", &flag, &value]).await?;
        let external_version_ids: Vec<String> = line
            .get("externalVersionIdentifiers")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol("ipatool returned no version identifiers".into()))?
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect();
        let latest = external_version_ids.last().cloned().unwrap_or_default();
        Ok(AppVersions {
            external_version_ids,
            latest,
        })
    }

    /// Downloads an app into `dest_dir`, acquiring the free license when
    /// needed, and returns the IPA path. ipatool names the file
    /// `<bundleId>_<appId>_<version>.ipa` and already adds the metadata and
    /// sinfs. ipatool reports no byte progress in JSON mode, so `progress`
    /// only sees the start and the end.
    pub async fn download(
        &self,
        app: &AppRef,
        version: Option<&str>,
        dest_dir: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<PathBuf> {
        tokio::fs::create_dir_all(dest_dir).await?;
        let dest = dest_dir
            .to_str()
            .ok_or_else(|| Error::InvalidInput("destination path is not UTF-8".into()))?;
        let [flag, value] = Self::app_args(app);
        let mut args = vec![
            "download",
            flag.as_str(),
            value.as_str(),
            "--purchase",
            "-o",
            dest,
        ];
        if let Some(version) = version {
            args.extend(["--external-version-id", version]);
        }
        progress(Progress::new("download", 0, None));
        let line = self.run_ok(&args).await?;
        let output = PathBuf::from(str_field(&line, "output"));
        if output.as_os_str().is_empty() {
            return Err(Error::Protocol("ipatool reported no output path".into()));
        }
        let size = tokio::fs::metadata(&output).await?.len();
        progress(Progress::new("download", size, Some(size)));
        Ok(output)
    }
}
