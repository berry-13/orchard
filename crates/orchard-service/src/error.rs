//! Error type returned by every command.

use serde::Serialize;

/// Serialized as `{ kind, message }`; `kind` matches `orchard_core::Error::kind()`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }

    /// The task was cancelled through `cancel_task`.
    pub fn cancelled() -> Self {
        Self::new("cancelled", "Cancelled")
    }
}

impl From<orchard_core::Error> for CommandError {
    fn from(e: orchard_core::Error) -> Self {
        Self::new(e.kind(), e.to_string())
    }
}

impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        orchard_core::Error::Io(e).into()
    }
}

pub type CmdResult<T> = Result<T, CommandError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_kind_and_message() {
        let e: CommandError = orchard_core::Error::LicenseRequired.into();
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["kind"], "licenseRequired");
        assert!(json["message"].as_str().unwrap().contains("does not own"));
    }
}
