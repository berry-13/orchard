//! Client authentication.
//!
//! A client proves it may connect with a one-time connect code shown by the
//! server (at start, or by `orchard-server code`) and gets a long random
//! token for every later request. Only hashes of codes and tokens are stored.
//! The code and the tokens live in files so a second `orchard-server code`
//! or `orchard-server revoke` process acts on the running server: the token
//! file is the source of truth, reloaded whenever it changes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use orchard_core::{Error, Result, write_private};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How long a connect code can be redeemed.
pub const CODE_LIFETIME_SECS: u64 = 15 * 60;
/// Wrong guesses after which a code stops working.
const MAX_FAILURES: u32 = 10;
/// No 0/O or 1/I, so a code read off a terminal is typed right.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Serialize, Deserialize)]
struct CodeFile {
    hash: String,
    expires: u64,
    failures: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct TokenEntry {
    hash: String,
    client: String,
    created: u64,
}

/// The token file as last read, with the modification time and size it had.
struct Tokens {
    stamp: Option<(SystemTime, u64)>,
    entries: Vec<TokenEntry>,
}

pub struct Auth {
    dir: PathBuf,
    tokens: Mutex<Tokens>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
    Ok(bytes)
}

/// Uppercase, without the separators people type.
fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn code_path(dir: &Path) -> PathBuf {
    dir.join("connect-code.json")
}

fn tokens_path(dir: &Path) -> PathBuf {
    dir.join("tokens.json")
}

fn stamp(dir: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(tokens_path(dir)).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn load_tokens(dir: &Path) -> Result<Vec<TokenEntry>> {
    match std::fs::read(tokens_path(dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::Io(e.into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_vec_pretty(value).map_err(|e| Error::Io(e.into()))?;
    write_private(path, &json)
}

impl Auth {
    /// Loads the tokens issued so far from `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let tokens = Tokens {
            stamp: stamp(dir),
            entries: load_tokens(dir)?,
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            tokens: Mutex::new(tokens),
        })
    }

    /// The tokens, reloaded when another process (`orchard-server revoke`)
    /// changed the file. An unreadable file counts as no tokens.
    fn tokens(&self) -> MutexGuard<'_, Tokens> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let current = stamp(&self.dir);
        if current != tokens.stamp {
            tokens.entries = load_tokens(&self.dir).unwrap_or_default();
            tokens.stamp = current;
        }
        tokens
    }

    /// Issues a new connect code, replacing any earlier one, formatted as
    /// `XXXX-XXXX`.
    pub fn new_code(dir: &Path) -> Result<String> {
        std::fs::create_dir_all(dir)?;
        let code: String = random_bytes::<8>()?
            .iter()
            .map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char)
            .collect();
        write_json(
            &code_path(dir),
            &CodeFile {
                hash: hash(&code),
                expires: now() + CODE_LIFETIME_SECS,
                failures: 0,
            },
        )?;
        Ok(format!("{}-{}", &code[..4], &code[4..]))
    }

    /// Forgets every issued token, signing out all clients.
    pub fn revoke_all(dir: &Path) -> Result<usize> {
        let count = load_tokens(dir).map_or(0, |t| t.len());
        match std::fs::remove_file(tokens_path(dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(count),
        }
    }

    /// Whether any client was ever connected.
    pub fn has_clients(&self) -> bool {
        !self.tokens().entries.is_empty()
    }

    /// Exchanges the current connect code for a token. A code works once,
    /// expires after [`CODE_LIFETIME_SECS`] and dies after too many wrong
    /// guesses.
    pub fn redeem(&self, code: &str, client: &str) -> Result<String> {
        let mut tokens = self.tokens();
        let path = code_path(&self.dir);
        let rejected = || {
            Error::Auth(orchard_core::AuthError::BadCredentials(
                "wrong or expired connect code".into(),
            ))
        };
        let mut file: CodeFile = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| rejected())?,
            Err(_) => return Err(rejected()),
        };
        if file.expires < now() || file.failures >= MAX_FAILURES {
            let _ = std::fs::remove_file(&path);
            return Err(rejected());
        }
        if hash(&normalize(code)) != file.hash {
            file.failures += 1;
            write_json(&path, &file)?;
            return Err(rejected());
        }
        std::fs::remove_file(&path)?;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
        tokens.entries.push(TokenEntry {
            hash: hash(&token),
            client: client.chars().take(64).collect(),
            created: now(),
        });
        write_json(&tokens_path(&self.dir), &tokens.entries)?;
        tokens.stamp = stamp(&self.dir);
        Ok(token)
    }

    /// Whether `token` was issued by [`Auth::redeem`] and not revoked.
    pub fn check(&self, token: &str) -> bool {
        let hashed = hash(token);
        self.tokens().entries.iter().any(|e| e.hash == hashed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_single_use_and_tokens_persist() {
        let dir = tempfile::tempdir().unwrap();
        let code = Auth::new_code(dir.path()).unwrap();
        let auth = Auth::open(dir.path()).unwrap();
        assert!(auth.redeem("WRONG-CODE", "ipad").is_err());
        let token = auth.redeem(&code.to_lowercase(), "ipad").unwrap();
        assert!(auth.check(&token));
        assert!(!auth.check("not-a-token"));
        assert!(auth.redeem(&code, "ipad").is_err(), "code works once");
        assert!(Auth::open(dir.path()).unwrap().check(&token));
        assert_eq!(Auth::revoke_all(dir.path()).unwrap(), 1);
        assert!(!Auth::open(dir.path()).unwrap().check(&token));
    }

    #[test]
    fn revoking_from_another_process_reaches_the_running_server() {
        let dir = tempfile::tempdir().unwrap();
        let running = Auth::open(dir.path()).unwrap();
        let first = running
            .redeem(&Auth::new_code(dir.path()).unwrap(), "a")
            .unwrap();
        assert!(running.check(&first));

        // `orchard-server revoke` runs as its own process.
        assert_eq!(Auth::revoke_all(dir.path()).unwrap(), 1);
        assert!(!running.check(&first), "revoked token still accepted");

        // A later connection must not write the revoked token back.
        let second = running
            .redeem(&Auth::new_code(dir.path()).unwrap(), "b")
            .unwrap();
        assert!(running.check(&second));
        assert!(!running.check(&first));
        assert!(!Auth::open(dir.path()).unwrap().check(&first));
    }

    #[test]
    fn code_dies_after_too_many_guesses() {
        let dir = tempfile::tempdir().unwrap();
        let code = Auth::new_code(dir.path()).unwrap();
        let auth = Auth::open(dir.path()).unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(auth.redeem("AAAA-AAAA", "x").is_err());
        }
        assert!(auth.redeem(&code, "x").is_err());
    }
}
