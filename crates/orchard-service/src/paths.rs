//! Turning names that come from a device or a backup into local paths.

use std::path::{Component, Path, PathBuf};

use orchard_core::Error;

use crate::error::CmdResult;

/// Checks that `name` is one plain file name that cannot leave its folder:
/// not empty, `.` or `..`, no `/`, `\`, `:` or NUL, and exactly one normal
/// path component on this platform.
pub fn safe_name(name: &str) -> CmdResult<&str> {
    let bad_char = name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '\0'));
    let mut components = Path::new(name).components();
    let single = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    );
    if name.is_empty() || name == "." || name == ".." || bad_char || !single {
        return Err(
            Error::InvalidInput(format!("unsafe file name from the device: {name:?}")).into(),
        );
    }
    Ok(name)
}

/// `base` joined with each `/` separated part of `relative`, every part checked
/// with [`safe_name`].
pub fn safe_join(base: &Path, relative: &str) -> CmdResult<PathBuf> {
    let mut path = base.to_path_buf();
    for part in relative.split('/') {
        path.push(safe_name(part)?);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_hostile_names() {
        for bad in [
            "",
            ".",
            "..",
            "/etc/passwd",
            "a/b",
            "..\\x",
            "a\\b",
            "C:",
            "c:evil",
            "nul\0byte",
            "/",
        ] {
            assert!(safe_name(bad).is_err(), "{bad:?}");
        }
        for good in [
            "IMG_0001.HEIC",
            ".hidden",
            "a..b",
            "name with spaces",
            "....",
        ] {
            assert_eq!(safe_name(good).unwrap(), good);
        }
    }

    #[test]
    fn safe_join_checks_every_part() {
        let base = Path::new("/out");
        assert_eq!(
            safe_join(base, "Library/a.db").unwrap(),
            PathBuf::from("/out/Library/a.db")
        );
        assert!(safe_join(base, "Library/../../etc").is_err());
        assert!(safe_join(base, "/etc/passwd").is_err());
        assert!(safe_join(base, "Library//a.db").is_err());
    }
}
