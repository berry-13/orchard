//! One App Store entry point for the CLI and the app.
//!
//! Apple now requires sign-in requests to carry a SAP signature that only
//! Apple's own code can produce. When ipatool is installed it does that, so
//! account operations go through it; otherwise Orchard's native client is
//! used, which Apple may reject at sign-in. Search and lookup are public APIs
//! and always native.

use std::path::{Path, PathBuf};

use orchard_core::{AuthError, Error, Progress, Result};
use serde::{Deserialize, Serialize};

use crate::{
    Account, AppVersions, Ipatool, LookupKey, PaidPurchase, PurchaseConfirmation, StoreApp,
    StoreClient, StoreIdentity,
};

/// Which implementation handles account operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoreBackend {
    Ipatool,
    Native,
}

/// The signed-in account as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreAccount {
    pub email: String,
    pub name: String,
    /// Lowercase store country, when known.
    pub country: Option<String>,
    pub backend: StoreBackend,
}

pub struct AppStore {
    client: StoreClient,
    ipatool: Option<Ipatool>,
    account_path: PathBuf,
}

impl AppStore {
    /// Uses ipatool when it is found, keeping state under `data_dir`.
    pub fn new(data_dir: &Path) -> Result<Self> {
        Ok(Self {
            client: StoreClient::new()?.with_partial_dir(data_dir.join("partial-downloads")),
            ipatool: Ipatool::detect(data_dir),
            account_path: data_dir.join("account.json"),
        })
    }

    /// Always uses the native client.
    pub fn native(client: StoreClient, data_dir: &Path) -> Self {
        Self {
            client,
            ipatool: None,
            account_path: data_dir.join("account.json"),
        }
    }

    pub fn backend(&self) -> StoreBackend {
        if self.ipatool.is_some() {
            StoreBackend::Ipatool
        } else {
            StoreBackend::Native
        }
    }

    pub fn client(&self) -> &StoreClient {
        &self.client
    }

    /// The signed-in account, `None` when signed out.
    pub async fn account(&self) -> Result<Option<StoreAccount>> {
        match &self.ipatool {
            Some(ipatool) => Ok(ipatool.info().await?.map(Self::from_identity)),
            None => Ok(self.native_account().ok().map(|a| Self::from_native(&a))),
        }
    }

    /// Signs in; fails with [`AuthError::CodeRequired`] when a 2FA code is needed.
    pub async fn sign_in(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<StoreAccount> {
        match &self.ipatool {
            Some(ipatool) => Ok(Self::from_identity(
                ipatool.login(email, password, code).await?,
            )),
            None => self.sign_in_native(email, password, code).await,
        }
    }

    /// Signs in with Orchard's own client even when ipatool is installed;
    /// needed for paid purchases, which ipatool does not support.
    pub async fn sign_in_native(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<StoreAccount> {
        let guid = self.native_account().ok().map(|a| a.guid);
        let account = self
            .client
            .sign_in(email, password, code, guid.as_deref())
            .await?;
        if let Some(dir) = self.account_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        account.save(&self.account_path)?;
        Ok(Self::from_native(&account))
    }

    /// Whether a native account is stored, as needed by [`AppStore::purchase_paid`].
    pub fn has_native_account(&self) -> bool {
        self.native_account().is_ok()
    }

    pub async fn sign_out(&self) -> Result<()> {
        match &self.ipatool {
            Some(ipatool) => ipatool.logout().await,
            None => match std::fs::remove_file(&self.account_path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        }
    }

    pub async fn search(&self, term: &str, country: &str, limit: u32) -> Result<Vec<StoreApp>> {
        self.client.search(term, country, limit).await
    }

    pub async fn lookup(&self, key: &LookupKey, country: &str) -> Result<Option<StoreApp>> {
        self.client.lookup(key, country).await
    }

    /// Acquires a free app for the account.
    pub async fn purchase(&self, app_id: u64) -> Result<()> {
        match &self.ipatool {
            Some(ipatool) => ipatool.purchase(&LookupKey::Id(app_id)).await,
            None => self.client.purchase(&self.native_account()?, app_id).await,
        }
    }

    /// Buys a paid app; see [`StoreClient::purchase_paid`]. ipatool cannot buy
    /// paid apps, so this always uses Orchard's own client and its account.
    pub async fn purchase_paid(
        &self,
        app_id: u64,
        price: f64,
        confirmation: Option<&PurchaseConfirmation>,
    ) -> Result<PaidPurchase> {
        let account = self
            .native_account()
            .map_err(|e| match (e, &self.ipatool) {
                (Error::Auth(AuthError::SignedOut), Some(_)) => Error::Store(
                    "paid apps need a sign-in with Orchard's built-in App Store client; \
                 ipatool only handles free apps"
                        .into(),
                ),
                (e, _) => e,
            })?;
        self.client
            .purchase_paid(&account, app_id, price, confirmation)
            .await
    }

    pub async fn list_versions(&self, app_id: u64) -> Result<AppVersions> {
        match &self.ipatool {
            Some(ipatool) => ipatool.list_versions(&LookupKey::Id(app_id)).await,
            None => {
                self.client
                    .list_versions(&self.native_account()?, app_id)
                    .await
            }
        }
    }

    /// Downloads an installable IPA (license acquired when needed) into
    /// `dest_dir`. Both backends resume interrupted downloads: ipatool on its
    /// own, the native client from its partial download directory.
    pub async fn download(
        &self,
        app_id: u64,
        version: Option<&str>,
        dest_dir: &Path,
        progress: impl FnMut(Progress) + Send,
    ) -> Result<PathBuf> {
        match &self.ipatool {
            Some(ipatool) => {
                ipatool
                    .download(&LookupKey::Id(app_id), version, dest_dir, progress)
                    .await
            }
            None => {
                let account = self.native_account()?;
                self.client
                    .download_app(&account, app_id, version, dest_dir, progress)
                    .await
            }
        }
    }

    fn native_account(&self) -> Result<Account> {
        Account::load(&self.account_path).map_err(|e| match e {
            Error::NotFound(_) => AuthError::SignedOut.into(),
            e => e,
        })
    }

    fn from_native(account: &Account) -> StoreAccount {
        StoreAccount {
            email: account.email.clone(),
            name: account.name.clone(),
            country: account.country_code().map(str::to_owned),
            backend: StoreBackend::Native,
        }
    }

    fn from_identity(identity: StoreIdentity) -> StoreAccount {
        StoreAccount {
            email: identity.email,
            name: identity.name,
            country: None,
            backend: StoreBackend::Ipatool,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_backend_reports_signed_out_without_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = AppStore::native(StoreClient::new().unwrap(), dir.path());
        assert_eq!(store.backend(), StoreBackend::Native);
        assert_eq!(store.account().await.unwrap(), None);
        assert!(matches!(
            store.purchase(1).await,
            Err(Error::Auth(AuthError::SignedOut))
        ));
        store.sign_out().await.unwrap();
    }
}
