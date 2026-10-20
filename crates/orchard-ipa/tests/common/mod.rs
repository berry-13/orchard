#![allow(dead_code)]

use std::io::Write;

use zip::write::SimpleFileOptions;

pub const EXECUTABLE: &str = "Demo";

/// A thin arm64 Mach-O with an `LC_ENCRYPTION_INFO_64` command.
pub fn macho(cryptid: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [0xFEED_FACFu32, 0x0100_000C, 0, 2, 1, 24, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0x2Cu32, 24, 0x4000, 0x1000, cryptid, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0x5A; 256]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Crushes RGBA pixels into an Apple CgBI PNG: premultiplied BGRA, raw deflate.
pub fn crush_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut filtered = Vec::new();
    for row in rgba.chunks_exact(width as usize * 4) {
        filtered.push(0);
        for px in row.as_chunks::<4>().0 {
            let a = u32::from(px[3]);
            let pm = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
            filtered.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
        }
    }
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    deflate.write_all(&filtered).unwrap();
    let idat = deflate.finish().unwrap();

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, b"CgBI", &[0x50, 0x00, 0x20, 0x06]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A standard RGBA PNG.
pub fn plain_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(rgba).unwrap();
    w.finish().unwrap();
    out
}

/// Decodes a standard PNG to (width, height, RGBA pixels).
pub fn decode_png(data: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Pixels used for the large test icon: opaque, transparent and half alpha.
pub fn icon_pixels() -> Vec<u8> {
    let mut px = Vec::new();
    for i in 0..16u8 {
        let alpha = match i % 4 {
            0 => 255,
            1 => 0,
            2 => 128,
            _ => 200,
        };
        let rgb = if alpha == 0 {
            [0, 0, 0]
        } else {
            [200, i * 10, 40]
        };
        px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
    }
    px
}

pub struct IpaSpec {
    pub encrypted: bool,
    pub manifest_sinf_paths: Option<Vec<String>>,
    pub with_metadata: bool,
}

impl Default for IpaSpec {
    fn default() -> Self {
        Self {
            encrypted: true,
            manifest_sinf_paths: None,
            with_metadata: false,
        }
    }
}

pub fn info_plist() -> Vec<u8> {
    let mut primary = plist::Dictionary::new();
    primary.insert(
        "CFBundleIconFiles".into(),
        plist::Value::Array(vec!["AppIcon60x60".into()]),
    );
    let mut icons = plist::Dictionary::new();
    icons.insert("CFBundlePrimaryIcon".into(), primary.into());

    let mut info = plist::Dictionary::new();
    info.insert("CFBundleIdentifier".into(), "com.example.demo".into());
    info.insert("CFBundleDisplayName".into(), "Demo App".into());
    info.insert("CFBundleName".into(), "Demo".into());
    info.insert("CFBundleShortVersionString".into(), "1.2.3".into());
    info.insert("CFBundleVersion".into(), "45".into());
    info.insert("CFBundleExecutable".into(), EXECUTABLE.into());
    info.insert("MinimumOSVersion".into(), "15.0".into());
    info.insert(
        "UIDeviceFamily".into(),
        plist::Value::Array(vec![1u64.into(), 2u64.into()]),
    );
    info.insert("CFBundleIcons".into(), icons.into());
    let mut out = Vec::new();
    plist::Value::Dictionary(info)
        .to_writer_binary(&mut out)
        .unwrap();
    out
}

/// Builds a small IPA in memory.
pub fn build_ipa(spec: &IpaSpec) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflated = SimpleFileOptions::default();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let app = "Payload/Demo.app/";

    zip.add_directory("Payload/", stored).unwrap();
    zip.add_directory(app, stored).unwrap();
    zip.start_file(format!("{app}Info.plist"), deflated)
        .unwrap();
    zip.write_all(&info_plist()).unwrap();
    zip.start_file(format!("{app}{EXECUTABLE}"), deflated)
        .unwrap();
    zip.write_all(&macho(u32::from(spec.encrypted))).unwrap();
    zip.start_file(format!("{app}AppIcon60x60@2x.png"), stored)
        .unwrap();
    zip.write_all(&crush_png(&icon_pixels(), 4, 4)).unwrap();
    zip.start_file(format!("{app}AppIcon60x60.png"), stored)
        .unwrap();
    zip.write_all(&plain_png(&[1, 2, 3, 255].repeat(4), 2, 2))
        .unwrap();
    zip.start_file(format!("{app}Assets.car"), deflated)
        .unwrap();
    zip.write_all(&[7u8; 4096]).unwrap();
    // A nested bundle that must not be mistaken for the main app.
    zip.start_file(format!("{app}PlugIns/Ext.appex/Info.plist"), deflated)
        .unwrap();
    zip.write_all(b"not a plist").unwrap();
    if let Some(paths) = &spec.manifest_sinf_paths {
        let mut manifest = plist::Dictionary::new();
        manifest.insert(
            "SinfPaths".into(),
            plist::Value::Array(paths.iter().map(|p| p.as_str().into()).collect()),
        );
        zip.start_file(format!("{app}SC_Info/Manifest.plist"), deflated)
            .unwrap();
        plist::Value::Dictionary(manifest)
            .to_writer_xml(&mut zip)
            .unwrap();
    }
    if spec.with_metadata {
        zip.start_file("iTunesMetadata.plist", deflated).unwrap();
        zip.write_all(b"<plist/>").unwrap();
        zip.start_file(format!("{app}SC_Info/{EXECUTABLE}.sinf"), stored)
            .unwrap();
        zip.write_all(b"old sinf").unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Every entry of a zip with its decompressed bytes.
pub fn read_entries(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut f = zip.by_index(i).unwrap();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            (f.name().to_owned(), buf)
        })
        .collect()
}
