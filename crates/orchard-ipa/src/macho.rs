//! Minimal Mach-O reader: finds `LC_ENCRYPTION_INFO(_64)` in thin and fat binaries.

use std::io::{self, Read};

const FAT_MAGIC: u32 = 0xCAFE_BABE;
const FAT_MAGIC_64: u32 = 0xCAFE_BABF;
const MH_MAGIC: u32 = 0xFEED_FACE;
const MH_MAGIC_64: u32 = 0xFEED_FACF;
const LC_ENCRYPTION_INFO: u32 = 0x21;
const LC_ENCRYPTION_INFO_64: u32 = 0x2C;
const MAX_FAT_ARCHS: u32 = 64;
const MAX_LOAD_COMMANDS_SIZE: u32 = 16 * 1024 * 1024;

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Reads the executable sequentially and reports whether any slice is FairPlay
/// encrypted (`cryptid != 0`).
pub(crate) fn is_encrypted(mut r: impl Read) -> io::Result<bool> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    let be = u32::from_be_bytes(magic);
    if be != FAT_MAGIC && be != FAT_MAGIC_64 {
        return slice_encrypted(&mut r, magic).map(|(encrypted, _)| encrypted);
    }

    let is64 = be == FAT_MAGIC_64;
    let count = read_u32(&mut r, true)?;
    if count > MAX_FAT_ARCHS {
        return Err(invalid("too many fat architectures"));
    }
    let mut pos: u64 = 8;
    let mut offsets = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut entry = [0u8; 32];
        let len = if is64 { 32 } else { 20 };
        r.read_exact(&mut entry[..len])?;
        pos += len as u64;
        let offset = if is64 {
            u64::from_be_bytes(entry[8..16].try_into().expect("slice length"))
        } else {
            u64::from(u32::from_be_bytes(
                entry[8..12].try_into().expect("slice length"),
            ))
        };
        offsets.push(offset);
    }
    offsets.sort_unstable();

    for offset in offsets {
        let skip = offset
            .checked_sub(pos)
            .ok_or_else(|| invalid("overlapping fat slices"))?;
        if io::copy(&mut (&mut r).take(skip), &mut io::sink())? != skip {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        r.read_exact(&mut magic)?;
        let (encrypted, consumed) = slice_encrypted(&mut r, magic)?;
        if encrypted {
            return Ok(true);
        }
        pos = offset + 4 + consumed;
    }
    Ok(false)
}

/// Parses one thin Mach-O whose magic was already read. Returns the
/// encryption flag and the bytes consumed after the magic.
fn slice_encrypted(r: &mut impl Read, magic: [u8; 4]) -> io::Result<(bool, u64)> {
    let (big_endian, is64) = match (u32::from_le_bytes(magic), u32::from_be_bytes(magic)) {
        (MH_MAGIC, _) => (false, false),
        (MH_MAGIC_64, _) => (false, true),
        (_, MH_MAGIC) => (true, false),
        (_, MH_MAGIC_64) => (true, true),
        _ => return Err(invalid("not a Mach-O file")),
    };
    let header_len = if is64 { 28 } else { 24 };
    let mut header = [0u8; 28];
    r.read_exact(&mut header[..header_len])?;
    let field = |i: usize| {
        let bytes: [u8; 4] = header[i * 4..i * 4 + 4].try_into().expect("slice length");
        if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    };
    let ncmds = field(3);
    let sizeofcmds = field(4);
    if sizeofcmds > MAX_LOAD_COMMANDS_SIZE {
        return Err(invalid("load commands too large"));
    }
    let mut cmds = vec![0u8; sizeofcmds as usize];
    r.read_exact(&mut cmds)?;
    let consumed = header_len as u64 + u64::from(sizeofcmds);

    let word = |at: usize| -> Option<u32> {
        let bytes: [u8; 4] = cmds.get(at..at + 4)?.try_into().ok()?;
        Some(if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    };
    let mut at = 0usize;
    for _ in 0..ncmds {
        let (Some(cmd), Some(size)) = (word(at), word(at + 4)) else {
            return Err(invalid("truncated load command"));
        };
        if cmd == LC_ENCRYPTION_INFO || cmd == LC_ENCRYPTION_INFO_64 {
            let cryptid = word(at + 16).ok_or_else(|| invalid("truncated encryption info"))?;
            if cryptid != 0 {
                return Ok((true, consumed));
            }
        }
        if size < 8 {
            return Err(invalid("invalid load command size"));
        }
        at += size as usize;
    }
    Ok((false, consumed))
}

fn read_u32(r: &mut impl Read, big_endian: bool) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(if big_endian {
        u32::from_be_bytes(b)
    } else {
        u32::from_le_bytes(b)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin(is64: bool, cryptid: Option<u32>) -> Vec<u8> {
        let mut cmds = Vec::new();
        // An unrelated command first, to exercise the walk.
        cmds.extend_from_slice(&0x1Bu32.to_le_bytes());
        cmds.extend_from_slice(&24u32.to_le_bytes());
        cmds.extend_from_slice(&[0u8; 16]);
        if let Some(id) = cryptid {
            let (cmd, size) = if is64 {
                (LC_ENCRYPTION_INFO_64, 24u32)
            } else {
                (LC_ENCRYPTION_INFO, 20u32)
            };
            for v in [cmd, size, 0x4000, 0x1000, id] {
                cmds.extend_from_slice(&v.to_le_bytes());
            }
            if is64 {
                cmds.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        let ncmds = if cryptid.is_some() { 2u32 } else { 1 };
        let magic = if is64 { MH_MAGIC_64 } else { MH_MAGIC };
        let mut out = Vec::new();
        for v in [magic, 0x0100_000C, 0, 2, ncmds, cmds.len() as u32, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if is64 {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(&cmds);
        out.extend_from_slice(&[0xAA; 64]);
        out
    }

    fn fat(slices: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&FAT_MAGIC.to_be_bytes());
        out.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut offset = 0x1000u32;
        let mut offsets = Vec::new();
        for s in slices {
            for v in [0x0100_000Cu32, 0, offset, s.len() as u32, 12] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            offsets.push(offset);
            offset += 0x1000;
        }
        for (s, off) in slices.iter().zip(offsets) {
            out.resize(off as usize, 0);
            out.extend_from_slice(s);
        }
        out
    }

    #[test]
    fn thin_binaries() {
        assert!(is_encrypted(&thin(true, Some(1))[..]).unwrap());
        assert!(!is_encrypted(&thin(true, Some(0))[..]).unwrap());
        assert!(!is_encrypted(&thin(true, None)[..]).unwrap());
        assert!(is_encrypted(&thin(false, Some(1))[..]).unwrap());
    }

    #[test]
    fn fat_binaries() {
        let plain = fat(&[thin(false, Some(0)), thin(true, Some(0))]);
        assert!(!is_encrypted(&plain[..]).unwrap());
        let mixed = fat(&[thin(false, Some(0)), thin(true, Some(1))]);
        assert!(is_encrypted(&mixed[..]).unwrap());
    }

    #[test]
    fn rejects_garbage() {
        assert!(is_encrypted(&b"#!/bin/sh\n"[..]).is_err());
    }
}
