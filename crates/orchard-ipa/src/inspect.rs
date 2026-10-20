use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use orchard_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::cgbi::{normalize_png, png_dimensions};
use crate::{app_prefix, plist_string, zip_error};

/// Metadata read from an IPA without extracting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpaInfo {
    pub bundle_id: String,
    /// `CFBundleDisplayName`, falling back to `CFBundleName`.
    pub display_name: String,
    /// `CFBundleShortVersionString`.
    pub version: String,
    /// `CFBundleVersion`.
    pub build: String,
    pub minimum_os: Option<String>,
    /// `iphone`, `ipad`, `tv`, `watch`, `vision`, or the raw number.
    pub device_families: Vec<String>,
    /// Size of the IPA file in bytes.
    pub file_size: u64,
    /// `CFBundleExecutable`.
    pub executable: String,
    /// Whether the main executable is FairPlay encrypted; `None` when it could not be read.
    pub encrypted: Option<bool>,
    /// The IPA carries `iTunesMetadata.plist` at its root.
    pub has_itunes_metadata: bool,
    /// The app bundle carries a `.sinf` license under `SC_Info/`.
    pub has_sinf: bool,
    /// Largest app icon as a standard PNG.
    pub icon_png: Option<Vec<u8>>,
}

/// Reads bundle metadata, encryption state and the app icon from an IPA.
pub fn inspect(path: impl AsRef<Path>) -> Result<IpaInfo> {
    let file = File::open(path.as_ref())?;
    let file_size = file.metadata()?.len();
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(zip_error)?;
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    let prefix = app_prefix(names.iter().map(String::as_str))
        .ok_or_else(|| Error::InvalidInput("no Payload/*.app/Info.plist in archive".into()))?;

    let info = read_entry(&mut zip, &format!("{prefix}Info.plist"))?;
    let info: plist::Dictionary =
        plist::from_bytes(&info).map_err(|e| crate::protocol("invalid Info.plist", e))?;
    let get = |key: &str| plist_string(&info, key);

    let bundle_id = get("CFBundleIdentifier")
        .ok_or_else(|| Error::Protocol("Info.plist has no CFBundleIdentifier".into()))?;
    let executable = get("CFBundleExecutable").unwrap_or_default();
    let encrypted = if executable.is_empty() {
        None
    } else {
        zip.by_name(&format!("{prefix}{executable}"))
            .ok()
            .and_then(|f| crate::macho::is_encrypted(BufReader::new(f)).ok())
    };

    let device_families = match info.get("UIDeviceFamily") {
        Some(plist::Value::Array(values)) => values.iter().filter_map(family_name).collect(),
        Some(value) => family_name(value).into_iter().collect(),
        None => Vec::new(),
    };

    let sc_info = format!("{prefix}SC_Info/");
    let has_sinf = names
        .iter()
        .any(|n| n.starts_with(&sc_info) && n.ends_with(".sinf"));
    let has_itunes_metadata = names.iter().any(|n| n == "iTunesMetadata.plist");
    let icon_png = find_icon(&mut zip, &names, &prefix, &info);

    Ok(IpaInfo {
        display_name: get("CFBundleDisplayName")
            .filter(|s| !s.is_empty())
            .or_else(|| get("CFBundleName"))
            .unwrap_or_else(|| bundle_id.clone()),
        version: get("CFBundleShortVersionString").unwrap_or_default(),
        build: get("CFBundleVersion").unwrap_or_default(),
        minimum_os: get("MinimumOSVersion"),
        bundle_id,
        device_families,
        file_size,
        executable,
        encrypted,
        has_itunes_metadata,
        has_sinf,
        icon_png,
    })
}

fn family_name(value: &plist::Value) -> Option<String> {
    let n = match value {
        plist::Value::Integer(i) => i.as_signed()?,
        plist::Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    Some(
        match n {
            1 => "iphone",
            2 => "ipad",
            3 => "tv",
            4 => "watch",
            7 => "vision",
            other => return Some(other.to_string()),
        }
        .to_owned(),
    )
}

pub(crate) fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut file = zip.by_name(name).map_err(|e| match e {
        zip::result::ZipError::FileNotFound => Error::NotFound(name.to_owned()),
        e => zip_error(e),
    })?;
    let mut data = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut data)?;
    Ok(data)
}

fn icon_names(info: &plist::Dictionary) -> Vec<String> {
    fn push_files(names: &mut Vec<String>, value: Option<&plist::Value>) {
        if let Some(plist::Value::Array(files)) = value {
            names.extend(
                files
                    .iter()
                    .filter_map(|v| v.as_string().map(str::to_owned)),
            );
        }
    }
    let mut names = Vec::new();
    for key in ["CFBundleIcons", "CFBundleIcons~ipad"] {
        let primary = info
            .get(key)
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("CFBundlePrimaryIcon"))
            .and_then(|v| v.as_dictionary());
        if let Some(primary) = primary {
            push_files(&mut names, primary.get("CFBundleIconFiles"));
            if let Some(name) = primary.get("CFBundleIconName").and_then(|v| v.as_string()) {
                names.push(name.to_owned());
            }
        }
    }
    push_files(&mut names, info.get("CFBundleIconFiles"));
    if let Some(name) = plist_string(info, "CFBundleIconFile") {
        names.push(name);
    }
    names
        .into_iter()
        .map(|n| n.trim_end_matches(".png").to_owned())
        .filter(|n| !n.is_empty())
        .collect()
}

fn find_icon<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    names: &[String],
    prefix: &str,
    info: &plist::Dictionary,
) -> Option<Vec<u8>> {
    let pngs: Vec<&str> = names
        .iter()
        .filter_map(|n| n.strip_prefix(prefix))
        .filter(|n| !n.contains('/') && n.to_ascii_lowercase().ends_with(".png"))
        .collect();
    let wanted = icon_names(info);
    let mut candidates: Vec<&str> = pngs
        .iter()
        .copied()
        .filter(|f| wanted.iter().any(|w| f.starts_with(w.as_str())))
        .collect();
    if candidates.is_empty() {
        candidates = pngs
            .iter()
            .copied()
            .filter(|f| f.starts_with("AppIcon"))
            .collect();
    }
    candidates.sort_unstable();

    let mut best: Option<(u64, Vec<u8>)> = None;
    for name in candidates {
        let Ok(data) = read_entry(zip, &format!("{prefix}{name}")) else {
            continue;
        };
        let Some((w, h)) = png_dimensions(&data) else {
            continue;
        };
        let area = u64::from(w) * u64::from(h);
        if best.as_ref().is_none_or(|(a, _)| area > *a) {
            best = Some((area, data));
        }
    }
    normalize_png(&best?.1).ok()
}
