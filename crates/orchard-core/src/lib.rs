//! Shared types used across Orchard crates.

use std::fmt;

/// Unique device identifier as reported by usbmuxd / lockdownd.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Udid(pub String);

impl fmt::Display for Udid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug)]
pub enum Error {
    NotImplemented(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented(what) => write!(f, "not implemented: {what}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
