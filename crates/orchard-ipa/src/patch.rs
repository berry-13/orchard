use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use orchard_core::{Error, Result};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::inspect::read_entry;
use crate::{app_prefix, plist_string, zip_error};

/// A FairPlay license blob returned by the store with a download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sinf {
    pub id: i64,
    pub data: Vec<u8>,
}

/// Copies `src` to `dst` adding `iTunesMetadata.plist` (when `metadata` is
/// given) and the sinfs. Existing entries are copied raw, without
/// recompression. `dst` is written through a temporary file and renamed.
///
/// Sinfs go to the paths listed in `SC_Info/Manifest.plist` (`SinfPaths`),
/// or to `SC_Info/<CFBundleExecutable>.sinf` when there is no manifest.
pub fn patch_ipa(
    src: &Path,
    dst: &Path,
    metadata: Option<&plist::Dictionary>,
    sinfs: &[Sinf],
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(src)?)).map_err(zip_error)?;
    let new_entries = sinf_entries(&mut zip, sinfs)?;
    let mut replaced: HashSet<&str> = new_entries.iter().map(|(n, _)| n.as_str()).collect();
    if metadata.is_some() {
        replaced.insert("iTunesMetadata.plist");
    }

    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut out = zip::ZipWriter::new(BufWriter::new(File::create(&tmp)?));
        for i in 0..zip.len() {
            let entry = zip.by_index_raw(i).map_err(zip_error)?;
            if replaced.contains(entry.name()) {
                continue;
            }
            out.raw_copy_file(entry).map_err(zip_error)?;
        }
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if let Some(metadata) = metadata {
            let mut data = Vec::new();
            plist::Value::Dictionary(metadata.clone())
                .to_writer_binary(&mut data)
                .map_err(|e| crate::protocol("encode iTunesMetadata.plist", e))?;
            out.start_file("iTunesMetadata.plist", options)
                .map_err(zip_error)?;
            out.write_all(&data)?;
        }
        for (name, data) in &new_entries {
            out.start_file(name.as_str(), options).map_err(zip_error)?;
            out.write_all(data)?;
        }
        let file = out
            .finish()
            .map_err(zip_error)?
            .into_inner()
            .map_err(|e| e.into_error())?;
        file.sync_all()?;
        std::fs::rename(&tmp, dst)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn sinf_entries<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    sinfs: &[Sinf],
) -> Result<Vec<(String, Vec<u8>)>> {
    if sinfs.is_empty() {
        return Ok(Vec::new());
    }
    let prefix = app_prefix(zip.file_names())
        .ok_or_else(|| Error::InvalidInput("no Payload/*.app/Info.plist in archive".into()))?;

    match read_entry(zip, &format!("{prefix}SC_Info/Manifest.plist")) {
        Ok(manifest) => {
            let manifest: plist::Dictionary = plist::from_bytes(&manifest)
                .map_err(|e| crate::protocol("invalid SC_Info/Manifest.plist", e))?;
            let paths = manifest
                .get("SinfPaths")
                .and_then(|v| v.as_array())
                .ok_or_else(|| Error::Protocol("Manifest.plist has no SinfPaths".into()))?;
            Ok(paths
                .iter()
                .filter_map(|p| p.as_string())
                .zip(sinfs)
                .map(|(path, sinf)| (format!("{prefix}{path}"), sinf.data.clone()))
                .collect())
        }
        Err(Error::NotFound(_)) => {
            let info = read_entry(zip, &format!("{prefix}Info.plist"))?;
            let info: plist::Dictionary =
                plist::from_bytes(&info).map_err(|e| crate::protocol("invalid Info.plist", e))?;
            let exe = plist_string(&info, "CFBundleExecutable")
                .ok_or_else(|| Error::Protocol("Info.plist has no CFBundleExecutable".into()))?;
            Ok(vec![(
                format!("{prefix}SC_Info/{exe}.sinf"),
                sinfs[0].data.clone(),
            )])
        }
        Err(e) => Err(e),
    }
}
