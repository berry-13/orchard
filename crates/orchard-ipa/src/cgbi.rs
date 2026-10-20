//! Apple "CgBI" PNG conversion.
//!
//! Xcode crushes bundled PNGs: it inserts a `CgBI` chunk before `IHDR`,
//! stores `IDAT` as raw deflate without the zlib wrapper, and keeps pixels as
//! premultiplied BGRA. Standard decoders reject such files.

use std::io::{Cursor, Read, Write};

use orchard_core::{Error, Result};

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

fn chunks(data: &[u8]) -> Result<Vec<Chunk<'_>>> {
    if !data.starts_with(&PNG_SIGNATURE) {
        return Err(Error::InvalidInput("not a PNG file".into()));
    }
    let mut out = Vec::new();
    let mut at = PNG_SIGNATURE.len();
    while at + 8 <= data.len() {
        let len = u32::from_be_bytes(data[at..at + 4].try_into().expect("slice length")) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().expect("slice length");
        let start = at + 8;
        let end = start
            .checked_add(len)
            .filter(|end| end + 4 <= data.len())
            .ok_or_else(|| Error::Protocol("truncated PNG chunk".into()))?;
        out.push(Chunk {
            kind,
            data: &data[start..end],
        });
        at = end + 4;
        if &kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

/// Width and height from the `IHDR` chunk, for standard and CgBI PNGs.
pub(crate) fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let chunks = chunks(data).ok()?;
    let ihdr = chunks.iter().find(|c| &c.kind == b"IHDR")?.data;
    let w = u32::from_be_bytes(ihdr.get(0..4)?.try_into().ok()?);
    let h = u32::from_be_bytes(ihdr.get(4..8)?.try_into().ok()?);
    Some((w, h))
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Converts an Apple CgBI PNG into a standard RGBA PNG. Standard PNGs are
/// returned unchanged.
pub fn normalize_png(data: &[u8]) -> Result<Vec<u8>> {
    let chunks = chunks(data)?;
    if !chunks.iter().any(|c| &c.kind == b"CgBI") {
        return Ok(data.to_vec());
    }
    let bad = |what: &str| Error::Protocol(format!("invalid CgBI PNG: {what}"));
    let ihdr = chunks
        .iter()
        .find(|c| &c.kind == b"IHDR")
        .map(|c| c.data)
        .filter(|d| d.len() == 13)
        .ok_or_else(|| bad("missing IHDR"))?;
    let (bit_depth, color_type) = (ihdr[8], ihdr[9]);
    let channels = match (bit_depth, color_type) {
        (8, 6) => 4,
        (8, 2) => 3,
        _ => return Err(bad("unsupported pixel format")),
    };

    let deflated: Vec<u8> = chunks
        .iter()
        .filter(|c| &c.kind == b"IDAT")
        .flat_map(|c| c.data.iter().copied())
        .collect();
    let mut filtered = Vec::new();
    flate2::read::DeflateDecoder::new(&deflated[..])
        .read_to_end(&mut filtered)
        .map_err(|e| bad(&e.to_string()))?;
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    zlib.write_all(&filtered)?;
    let zlib = zlib.finish()?;

    // Rewrap as a standard PNG so the png crate handles unfiltering and interlace.
    let mut standard = PNG_SIGNATURE.to_vec();
    write_chunk(&mut standard, b"IHDR", ihdr);
    write_chunk(&mut standard, b"IDAT", &zlib);
    write_chunk(&mut standard, b"IEND", &[]);

    let mut reader = png::Decoder::new(Cursor::new(standard))
        .read_info()
        .map_err(|e| bad(&e.to_string()))?;
    let mut pixels = vec![0u8; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|e| bad(&e.to_string()))?;
    pixels.truncate(frame.buffer_size());

    for px in pixels.chunks_exact_mut(channels) {
        px.swap(0, 2);
        if channels == 4 {
            let a = u32::from(px[3]);
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, frame.width, frame.height);
    encoder.set_color(if channels == 4 {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::Protocol(format!("png encode: {e}")))?;
    writer
        .write_image_data(&pixels)
        .map_err(|e| Error::Protocol(format!("png encode: {e}")))?;
    writer
        .finish()
        .map_err(|e| Error::Protocol(format!("png encode: {e}")))?;
    Ok(out)
}
