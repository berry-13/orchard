//! IPA tooling: App Store search, purchase and download (ipatool-style),
//! IPA inspection, re-signing and installation.

mod account;
mod appstore;
mod cgbi;
mod inspect;
mod ipatool;
mod macho;
mod partial;
mod patch;
mod sign;
mod store;
mod storefront;

pub use account::{Account, generate_guid};
pub use appstore::{AppStore, StoreAccount, StoreBackend};
pub use cgbi::normalize_png;
pub use inspect::{IpaInfo, inspect};
pub use ipatool::{ARGV_EXPOSURE_NOTE, AppRef, Ipatool, StoreIdentity};
pub use partial::{PARTIAL_MAX_AGE, prune_partial_downloads};
pub use patch::{Sinf, patch_ipa};
pub use sign::{
    IdentityInfo, ProvisioningProfile, ResignIssue, ResignOptions, ResignReport, SignedBundle,
    SigningIdentity, check_resign, resign_ipa,
};
pub use store::{
    AppVersions, LookupKey, PaidPurchase, PurchaseConfirmation, StoreApp, StoreClient, StoreConfig,
};
pub use storefront::country_for_store_front;

use orchard_core::Error;

pub(crate) fn protocol(context: &str, e: impl std::fmt::Display) -> Error {
    Error::Protocol(format!("{context}: {e}"))
}

pub(crate) fn zip_error(e: zip::result::ZipError) -> Error {
    match e {
        zip::result::ZipError::Io(e) => Error::Io(e),
        zip::result::ZipError::FileNotFound => Error::NotFound("file in archive".into()),
        e => protocol("invalid ipa archive", e),
    }
}

/// Finds the main app bundle directory, e.g. `Payload/Foo.app/`.
pub(crate) fn app_prefix<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    names
        .filter_map(|name| {
            let mut parts = name.split('/');
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some("Payload"), Some(app), Some("Info.plist"), None) if app.ends_with(".app") => {
                    Some(format!("Payload/{app}/"))
                }
                _ => None,
            }
        })
        .min()
}

/// Reads a string-ish plist value (strings and numbers).
pub(crate) fn plist_string(dict: &plist::Dictionary, key: &str) -> Option<String> {
    match dict.get(key)? {
        plist::Value::String(s) => Some(s.clone()),
        plist::Value::Integer(i) => Some(i.to_string()),
        plist::Value::Real(r) => Some(r.to_string()),
        _ => None,
    }
}
