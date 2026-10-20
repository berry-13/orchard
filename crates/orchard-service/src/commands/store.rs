//! App Store account, search, purchase, download and install.

use std::path::PathBuf;
use std::sync::Arc;

use crate::events::Events;
use orchard_core::Progress;

use crate::commands::app::SettingsStore;
use crate::dto::{StoreAccount, StoreApp, StoreBackend, StoreVersion};
use crate::error::CmdResult;
use crate::events::{TaskId, TaskRegistry, run_task};
use crate::state::{DeviceState, StoreState};

const DEFAULT_COUNTRY: &str = "US";

pub fn to_account(
    account: orchard_ipa::StoreAccount,
    fallback_country: Option<String>,
) -> StoreAccount {
    StoreAccount {
        email: account.email,
        name: account.name,
        country: account
            .country
            .or(fallback_country)
            .unwrap_or_else(|| DEFAULT_COUNTRY.into())
            .to_uppercase(),
        backend: match account.backend {
            orchard_ipa::StoreBackend::Ipatool => StoreBackend::Ipatool,
            orchard_ipa::StoreBackend::Native => StoreBackend::Native,
        },
    }
}

pub fn to_store_app(app: orchard_ipa::StoreApp) -> StoreApp {
    let formatted_price = app.formatted_price.unwrap_or_else(|| {
        if app.price == 0.0 {
            "Free".into()
        } else {
            format!("{:.2}", app.price)
        }
    });
    StoreApp {
        app_id: app.id,
        bundle_id: app.bundle_id,
        name: app.name,
        seller: app.seller.unwrap_or_default(),
        price: app.price,
        formatted_price,
        version: app.version,
        artwork_url: app.artwork_url.unwrap_or_default(),
        average_rating: app.rating,
        rating_count: app.rating_count.unwrap_or(0),
        genre: app.genre.unwrap_or_default(),
        size: app.file_size,
        minimum_os_version: app.minimum_os,
        description: app.release_notes.unwrap_or_default(),
    }
}

/// Newest first, as the versions dropdown shows them.
pub fn to_versions(versions: orchard_ipa::AppVersions) -> Vec<StoreVersion> {
    let mut ids = versions.external_version_ids;
    if !ids.contains(&versions.latest) {
        ids.push(versions.latest);
    }
    ids.into_iter()
        .rev()
        .map(|id| StoreVersion { id, version: None })
        .collect()
}

pub async fn store_account(
    store: &StoreState,
    settings: &SettingsStore,
) -> CmdResult<Option<StoreAccount>> {
    let account = store.store().account().await?;
    Ok(account.map(|a| to_account(a, settings.store_country())))
}

/// Fails with `authCodeRequired` when the account needs a two-factor code;
/// the frontend then retries with `code`.
pub async fn store_sign_in(
    store: &StoreState,
    settings: &SettingsStore,
    email: String,
    password: String,
    code: Option<String>,
    country: String,
    native: Option<bool>,
) -> CmdResult<StoreAccount> {
    let code = code.filter(|c| !c.trim().is_empty());
    let store = store.store();
    let account = if native.unwrap_or(false) {
        store
            .sign_in_native(email.trim(), &password, code.as_deref())
            .await?
    } else {
        store
            .sign_in(email.trim(), &password, code.as_deref())
            .await?
    };
    settings.set_store_country(country.clone()).await?;
    Ok(to_account(account, Some(country)))
}

pub async fn store_sign_out(store: &StoreState) -> CmdResult<()> {
    store.store().sign_out().await?;
    Ok(())
}

pub async fn store_search(
    store: &StoreState,
    term: String,
    country: String,
    limit: u32,
) -> CmdResult<Vec<StoreApp>> {
    let apps = store.store().search(term.trim(), &country, limit).await?;
    Ok(apps.into_iter().map(to_store_app).collect())
}

pub async fn store_versions(store: &StoreState, app_id: u64) -> CmdResult<Vec<StoreVersion>> {
    Ok(to_versions(store.store().list_versions(app_id).await?))
}

/// Gets a free app, or buys a paid one. A paid purchase first returns Apple's
/// confirmation; the frontend shows it and calls again with `confirmation`.
pub async fn store_purchase(
    store: &StoreState,
    app_id: u64,
    price: Option<f64>,
    confirmation: Option<orchard_ipa::PurchaseConfirmation>,
) -> CmdResult<orchard_ipa::PaidPurchase> {
    let store = store.store();
    match price.filter(|p| *p > 0.0) {
        Some(price) => Ok(store
            .purchase_paid(app_id, price, confirmation.as_ref())
            .await?),
        None => {
            store.purchase(app_id).await?;
            Ok(orchard_ipa::PaidPurchase::Purchased)
        }
    }
}

/// Task stage: `download` (bytes). Returns the IPA path.
pub async fn store_download(
    app: Events,
    tasks: &TaskRegistry,
    store: &StoreState,
    task_id: TaskId,
    app_id: u64,
    version_id: Option<String>,
    dest_dir: String,
) -> CmdResult<String> {
    let store = store.store();
    run_task(&app, tasks, task_id, |rep| async move {
        rep.progress(&Progress::new("license", 0, None));
        let path = store
            .download(
                app_id,
                version_id.as_deref(),
                &PathBuf::from(dest_dir),
                rep.callback(),
            )
            .await?;
        Ok(path.display().to_string())
    })
    .await
}

/// Downloads into a temporary folder then installs. Task stages: `download`
/// (bytes), `upload` (bytes), `install` (percent).
pub async fn store_install(
    app: Events,
    tasks: &TaskRegistry,
    store: &StoreState,
    devices: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    app_id: u64,
    version_id: Option<String>,
) -> CmdResult<()> {
    let store = store.store();
    let conn = devices.connection(&udid).await?;
    run_task(&app, tasks, task_id, |rep| async move {
        // Owned by the task future: dropping it (also on abort) removes the folder.
        let temp = tempfile::Builder::new()
            .prefix("orchard-install-")
            .tempdir()?;
        rep.progress(&Progress::new("license", 0, None));
        let ipa = store
            .download(app_id, version_id.as_deref(), temp.path(), rep.callback())
            .await?;
        conn.install_ipa(&ipa, rep.callback()).await?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_newest_first() {
        let v = to_versions(orchard_ipa::AppVersions {
            external_version_ids: vec!["1".into(), "2".into(), "3".into()],
            latest: "3".into(),
        });
        let ids: Vec<_> = v.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["3", "2", "1"]);
    }

    #[test]
    fn account_country_falls_back_to_sign_in_choice() {
        let a = to_account(
            orchard_ipa::StoreAccount {
                email: "a@b.c".into(),
                name: "A".into(),
                country: None,
                backend: orchard_ipa::StoreBackend::Ipatool,
            },
            Some("it".into()),
        );
        assert_eq!(a.country, "IT");
        assert_eq!(a.backend, StoreBackend::Ipatool);
    }
}
