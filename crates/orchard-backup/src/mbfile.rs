//! Decoding of the NSKeyedArchiver `MBFile` blobs stored in `Manifest.db`.

use orchard_core::{Error, Result};
use plist::{Dictionary, Value};

/// The fields of an `MBFile` record Orchard uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MbFile {
    pub size: u64,
    pub modified: Option<i64>,
    pub mode: u32,
    /// Symlink target.
    pub target: Option<String>,
    /// Wrapped per file key, prefixed with its little endian protection class.
    pub encryption_key: Option<Vec<u8>>,
}

pub(crate) fn decode(blob: &[u8]) -> Result<MbFile> {
    let archive: Value = plist::from_bytes(blob).map_err(|e| bad(&format!("MBFile: {e}")))?;
    let archive = archive
        .as_dictionary()
        .ok_or_else(|| bad("MBFile is not a dictionary"))?;
    let objects = archive
        .get("$objects")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("MBFile has no $objects"))?;
    let root = archive
        .get("$top")
        .and_then(Value::as_dictionary)
        .and_then(|top| top.get("root"))
        .ok_or_else(|| bad("MBFile has no root"))?;
    let root = resolve(objects, root)
        .as_dictionary()
        .ok_or_else(|| bad("MBFile root is not a dictionary"))?;

    let int = |key: &str| {
        root.get(key)
            .map(|v| resolve(objects, v))
            .and_then(Value::as_signed_integer)
    };
    Ok(MbFile {
        size: int("Size").unwrap_or(0).max(0) as u64,
        modified: int("LastModified"),
        mode: int("Mode").unwrap_or(0) as u32,
        target: root
            .get("Target")
            .map(|v| resolve(objects, v))
            .and_then(Value::as_string)
            .map(str::to_owned),
        encryption_key: root.get("EncryptionKey").and_then(|v| data(objects, v)),
    })
}

/// Follows a `UID` reference into `$objects`; other values are returned unchanged.
fn resolve<'a>(objects: &'a [Value], value: &'a Value) -> &'a Value {
    match value {
        Value::Uid(uid) => objects.get(uid.get() as usize).unwrap_or(value),
        _ => value,
    }
}

/// Reads an `NSData`, either inline or archived as `{ NS.data = <data> }`.
fn data(objects: &[Value], value: &Value) -> Option<Vec<u8>> {
    match resolve(objects, value) {
        Value::Data(bytes) => Some(bytes.clone()),
        Value::Dictionary(dict) => ns_data(objects, dict),
        _ => None,
    }
}

fn ns_data(objects: &[Value], dict: &Dictionary) -> Option<Vec<u8>> {
    match resolve(objects, dict.get("NS.data")?) {
        Value::Data(bytes) => Some(bytes.clone()),
        _ => None,
    }
}

fn bad(msg: &str) -> Error {
    Error::Protocol(msg.to_owned())
}
