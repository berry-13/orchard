//! Backup keybag parsing, password unlock and the AES primitives used by encrypted backups.

use std::collections::HashMap;
use std::io::{Read, Write};

use aes::Aes256;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecryptMut, KeyIvInit};
use orchard_core::{Error, Result};

/// Class key is wrapped with the key derived from the backup password.
const WRAP_PASSCODE: u32 = 2;

/// A parsed `BackupKeyBag` from `Manifest.plist`.
#[derive(Debug)]
pub(crate) struct Keybag {
    salt: Vec<u8>,
    iterations: u32,
    /// Salt and iteration count of the extra PBKDF2-SHA256 round added in iOS 10.2.
    dp_salt: Option<Vec<u8>>,
    dp_iterations: Option<u32>,
    classes: Vec<ClassKey>,
}

#[derive(Debug, Default)]
struct ClassKey {
    class: Option<u32>,
    wrap: u32,
    wrapped: Option<Vec<u8>>,
}

impl Keybag {
    /// Parses the TLV stream: 4 byte tag, 4 byte big endian length, value.
    pub(crate) fn parse(data: &[u8]) -> Result<Self> {
        let mut bag = Keybag {
            salt: Vec::new(),
            iterations: 0,
            dp_salt: None,
            dp_iterations: None,
            classes: Vec::new(),
        };
        let mut current: Option<ClassKey> = None;
        let mut seen_bag_uuid = false;
        let mut rest = data;
        while !rest.is_empty() {
            if rest.len() < 8 {
                return Err(bad("truncated keybag"));
            }
            let tag = &rest[..4];
            let len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
            let value = rest
                .get(8..8 + len)
                .ok_or_else(|| bad("truncated keybag value"))?;
            rest = &rest[8 + len..];
            match tag {
                b"UUID" if !seen_bag_uuid => seen_bag_uuid = true,
                b"UUID" => {
                    if let Some(key) = current.take() {
                        bag.classes.push(key);
                    }
                    current = Some(ClassKey::default());
                }
                b"SALT" if current.is_none() => bag.salt = value.to_vec(),
                b"ITER" if current.is_none() => bag.iterations = be_u32(value)?,
                b"DPSL" => bag.dp_salt = Some(value.to_vec()),
                b"DPIC" => bag.dp_iterations = Some(be_u32(value)?),
                b"CLAS" => {
                    if let Some(key) = current.as_mut() {
                        key.class = Some(be_u32(value)?);
                    }
                }
                b"WRAP" => {
                    if let Some(key) = current.as_mut() {
                        key.wrap = be_u32(value)?;
                    }
                }
                b"WPKY" => {
                    if let Some(key) = current.as_mut() {
                        key.wrapped = Some(value.to_vec());
                    }
                }
                _ => {}
            }
        }
        if let Some(key) = current {
            bag.classes.push(key);
        }
        if bag.salt.is_empty() || bag.iterations == 0 {
            return Err(bad("keybag has no SALT/ITER"));
        }
        Ok(bag)
    }

    /// Derives the password key and unwraps every password protected class key.
    pub(crate) fn unlock(&self, password: &str) -> Result<ClassKeys> {
        let mut key = [0u8; 32];
        match (&self.dp_salt, self.dp_iterations) {
            (Some(dp_salt), Some(dp_iterations)) => {
                let mut stage = [0u8; 32];
                pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                    password.as_bytes(),
                    dp_salt,
                    dp_iterations,
                    &mut stage,
                );
                pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&stage, &self.salt, self.iterations, &mut key);
            }
            _ => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
                password.as_bytes(),
                &self.salt,
                self.iterations,
                &mut key,
            ),
        }

        let mut keys = HashMap::new();
        for class in &self.classes {
            let (Some(id), Some(wrapped)) = (class.class, &class.wrapped) else {
                continue;
            };
            if class.wrap & WRAP_PASSCODE == 0 {
                continue;
            }
            let unwrapped = unwrap_key(&key, wrapped).ok_or(Error::BackupPassword)?;
            keys.insert(id, unwrapped);
        }
        if keys.is_empty() {
            return Err(bad("keybag has no password protected class keys"));
        }
        Ok(ClassKeys(keys))
    }
}

/// Unwrapped protection class keys of an unlocked backup.
#[derive(Debug, Clone)]
pub(crate) struct ClassKeys(HashMap<u32, [u8; 32]>);

impl ClassKeys {
    /// Unwraps a per item key: 4 byte little endian protection class, then the wrapped key.
    pub(crate) fn unwrap_item_key(&self, blob: &[u8]) -> Result<[u8; 32]> {
        if blob.len() < 4 + 40 {
            return Err(bad("encryption key blob is too short"));
        }
        let class = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
        let class_key = self
            .0
            .get(&class)
            .ok_or_else(|| bad(&format!("no key for protection class {class}")))?;
        unwrap_key(class_key, &blob[4..]).ok_or_else(|| bad("could not unwrap item key"))
    }
}

/// RFC 3394 AES key unwrap of a 256 bit key.
fn unwrap_key(kek: &[u8; 32], wrapped: &[u8]) -> Option<[u8; 32]> {
    if wrapped.len() != 40 {
        return None;
    }
    let mut out = [0u8; 32];
    aes_kw::KekAes256::from(*kek)
        .unwrap(wrapped, &mut out)
        .ok()?;
    Some(out)
}

/// Decrypts AES-256-CBC (zero IV) data from `reader` into `writer`, keeping only the first
/// `size` plaintext bytes. Calls `on_bytes` with the number of bytes written so far.
pub(crate) fn decrypt_stream(
    key: &[u8; 32],
    mut reader: impl Read,
    mut writer: impl Write,
    size: u64,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<u64> {
    let mut cipher = cbc::Decryptor::<Aes256>::new(key.into(), &[0u8; 16].into());
    let mut buf = vec![0u8; 64 * 1024];
    let mut filled = 0;
    let mut written = 0u64;
    loop {
        let read = reader.read(&mut buf[filled..])?;
        filled += read;
        let whole = filled - filled % 16;
        if read == 0 && filled != whole {
            return Err(bad("encrypted file length is not a multiple of 16"));
        }
        for block in buf[..whole].as_chunks_mut::<16>().0 {
            cipher.decrypt_block_mut(GenericArray::from_mut_slice(block));
        }
        let keep = (size - written).min(whole as u64) as usize;
        writer.write_all(&buf[..keep])?;
        written += keep as u64;
        on_bytes(written);
        buf.copy_within(whole..filled, 0);
        filled -= whole;
        if read == 0 {
            break;
        }
    }
    if written < size {
        return Err(bad("encrypted file is shorter than its recorded size"));
    }
    Ok(written)
}

/// Decrypts a whole AES-256-CBC (zero IV) buffer and strips PKCS#7 padding when present.
pub(crate) fn decrypt_all(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    decrypt_stream(key, data, &mut out, data.len() as u64, &mut |_| {})?;
    if let Some(&pad) = out.last() {
        let pad = pad as usize;
        if (1..=16).contains(&pad)
            && pad <= out.len()
            && out[out.len() - pad..].iter().all(|&b| b as usize == pad)
        {
            out.truncate(out.len() - pad);
        }
    }
    Ok(out)
}

fn be_u32(value: &[u8]) -> Result<u32> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| bad("keybag integer is not 4 bytes"))?;
    Ok(u32::from_be_bytes(bytes))
}

fn bad(msg: &str) -> Error {
    Error::Protocol(msg.to_owned())
}
