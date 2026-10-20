//! App Store client compatible with ipatool's protocol.

use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use orchard_core::{AuthError, Error, Progress, Result};
use plist::{Dictionary, Value};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderMap, LOCATION};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::partial::{PARTIAL_MAX_AGE, Partial, Sidecar, hex, prune_partial_downloads};

use crate::account::{Account, generate_guid};
use crate::patch::{Sinf, patch_ipa};
use crate::plist_string;

const DEFAULT_BUY_URL: &str = "https://buy.itunes.apple.com";
const PATH_PURCHASE: &str = "/WebObjects/MZFinance.woa/wa/buyProduct";
const PATH_DOWNLOAD: &str = "/WebObjects/MZFinance.woa/wa/volumeStoreDownloadProduct";

const FAILURE_INVALID_CREDENTIALS: &str = "-5000";
const FAILURE_PASSWORD_TOKEN_EXPIRED: &str = "2034";
const FAILURE_SIGN_IN_REQUIRED: &str = "2042";
const FAILURE_LICENSE_NOT_FOUND: &str = "9610";
const FAILURE_TEMPORARILY_UNAVAILABLE: &str = "2059";
const FAILURE_LICENSE_ALREADY_EXISTS: &str = "5002";
const FAILURE_DEVICE_VERIFICATION_FAILED: &str = "1008";

const MESSAGE_BAD_LOGIN: &str = "MZFinance.BadLogin.Configurator_message";
const MESSAGE_ACCOUNT_DISABLED: &str = "Your account is disabled.";
const MESSAGE_SUBSCRIPTION_REQUIRED: &str = "Subscription Required";
const MESSAGE_PASSWORD_CHANGED: &str = "Your password has changed.";

const MAX_AUTH_REDIRECTS: usize = 4;

/// Endpoints and identity used by [`StoreClient`]. Override the URLs to point
/// at a mock server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreConfig {
    /// Sign-in endpoint; `?guid=` is appended.
    pub auth_url: String,
    /// Base of the purchase and download endpoints. When this is Apple's
    /// default host, the account's pod is used (`p<pod>-buy.itunes.apple.com`).
    pub buy_url: String,
    /// Base of the public iTunes Search API.
    pub search_url: String,
    pub user_agent: String,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            auth_url: "https://auth.itunes.apple.com/auth/v1/native/fast".into(),
            buy_url: DEFAULT_BUY_URL.into(),
            search_url: "https://itunes.apple.com".into(),
            user_agent: "Configurator/2.15 (Macintosh; OS X 11.0.0; 16G29) AppleWebKit/2603.3.8"
                .into(),
        }
    }
}

/// An app from the iTunes Search API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreApp {
    pub id: u64,
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub price: f64,
    pub formatted_price: Option<String>,
    pub artwork_url: Option<String>,
    pub seller: Option<String>,
    pub file_size: Option<u64>,
    pub rating: Option<f64>,
    pub rating_count: Option<u64>,
    pub genre: Option<String>,
    pub minimum_os: Option<String>,
    pub release_notes: Option<String>,
}

/// How to look an app up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LookupKey {
    Id(u64),
    BundleId(String),
}

/// A price confirmation Apple asks for before charging the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseConfirmation {
    /// Apple's text, for example "Are you sure you want to buy ... for $4.99?".
    pub message: String,
    /// Label of the button that confirms, such as "Buy".
    pub ok_label: String,
    /// Opaque parameters Apple expects back to complete the purchase.
    pub buy_params: String,
}

/// Result of a paid purchase attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum PaidPurchase {
    /// The account now owns the app (or already did).
    Purchased,
    /// Show `confirmation` to the user; call again with it to pay.
    NeedsConfirmation(PurchaseConfirmation),
}

/// Versions of an app the store can still deliver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersions {
    /// External version ids, oldest first, as Apple returns them.
    pub external_version_ids: Vec<String>,
    /// The current version's external id.
    pub latest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ITunesResults {
    #[serde(default)]
    results: Vec<ITunesApp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ITunesApp {
    track_id: u64,
    #[serde(default)]
    bundle_id: String,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    version: String,
    price: Option<f64>,
    formatted_price: Option<String>,
    artwork_url512: Option<String>,
    artwork_url100: Option<String>,
    artwork_url60: Option<String>,
    seller_name: Option<String>,
    file_size_bytes: Option<serde_json::Value>,
    average_user_rating: Option<f64>,
    user_rating_count: Option<u64>,
    primary_genre_name: Option<String>,
    minimum_os_version: Option<String>,
    release_notes: Option<String>,
}

impl From<ITunesApp> for StoreApp {
    fn from(a: ITunesApp) -> Self {
        Self {
            id: a.track_id,
            bundle_id: a.bundle_id,
            name: a.track_name,
            version: a.version,
            price: a.price.unwrap_or(0.0),
            formatted_price: a.formatted_price,
            artwork_url: a.artwork_url512.or(a.artwork_url100).or(a.artwork_url60),
            seller: a.seller_name,
            file_size: a.file_size_bytes.and_then(|v| match v {
                serde_json::Value::String(s) => s.parse().ok(),
                v => v.as_u64(),
            }),
            rating: a.average_user_rating,
            rating_count: a.user_rating_count,
            genre: a.primary_genre_name,
            minimum_os: a.minimum_os_version,
            release_notes: a.release_notes,
        }
    }
}

/// App Store client: sign-in, search, purchase of free apps and IPA download.
#[derive(Debug, Clone)]
pub struct StoreClient {
    config: StoreConfig,
    /// Talks to Apple's API; redirects are handled by hand.
    api: reqwest::Client,
    /// Fetches IPAs from the CDN, following redirects.
    cdn: reqwest::Client,
    /// Where partial downloads are kept; `<dest_dir>/.orchard-partial` when `None`.
    partial_dir: Option<PathBuf>,
}

/// Outcome of one CDN request for a partial download.
enum Fetch {
    /// The whole file is on disk; carries its MD5 and whether it was resumed.
    Done { digest: String, resumed: bool },
    /// The URL was rejected (expired); a fresh download item is needed.
    Expired,
    /// The partial cannot be continued; start again from zero.
    Restart,
}

/// Parses `bytes <start>-<end>/<size>` into the start and the full size.
fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, size) = rest.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, size.trim().parse().ok()))
}

fn network(e: reqwest::Error) -> Error {
    Error::Network(e.to_string())
}

fn plist_body(dict: Dictionary) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    Value::Dictionary(dict)
        .to_writer_xml(&mut body)
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok(body)
}

fn parse_plist(body: &[u8]) -> Result<Dictionary> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Dictionary::new());
    }
    plist::from_bytes(body).map_err(|e| crate::protocol("invalid store response", e))
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn dict<I: IntoIterator<Item = (&'static str, Value)>>(entries: I) -> Dictionary {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

impl StoreClient {
    /// A client for Apple's production endpoints.
    pub fn new() -> Result<Self> {
        Self::with_config(StoreConfig::default())
    }

    /// A client for custom endpoints.
    pub fn with_config(config: StoreConfig) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let build = |redirects: reqwest::redirect::Policy| {
            reqwest::Client::builder()
                .user_agent(config.user_agent.clone())
                .redirect(redirects)
                .build()
                .map_err(network)
        };
        Ok(Self {
            api: build(reqwest::redirect::Policy::none())?,
            cdn: build(reqwest::redirect::Policy::default())?,
            config,
            partial_dir: None,
        })
    }

    /// Keeps partial downloads in `dir` instead of `<dest_dir>/.orchard-partial`.
    pub fn with_partial_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.partial_dir = Some(dir.into());
        self
    }

    /// Signs in with an Apple ID. `code` is the two-factor code, appended to
    /// the password as ipatool does; `guid` should be the one from a previous
    /// [`Account`] so Apple sees the same machine, a new one is generated when
    /// `None`.
    ///
    /// Fails with [`AuthError::CodeRequired`] when the account needs a code.
    pub async fn sign_in(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
        guid: Option<&str>,
    ) -> Result<Account> {
        let guid = guid.map_or_else(generate_guid, str::to_owned);
        let code: String = code
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let mut url = reqwest::Url::parse(&self.config.auth_url)
            .map_err(|e| Error::InvalidInput(format!("auth url: {e}")))?;
        url.query_pairs_mut().append_pair("guid", &guid);
        let mut attempt = 1;
        let mut redirects = 0;

        loop {
            let body = plist_body(dict([
                ("appleId", Value::String(email.to_owned())),
                ("attempt", Value::String(attempt.to_string())),
                ("guid", Value::String(guid.clone())),
                ("password", Value::String(format!("{password}{code}"))),
                ("rmp", Value::String("0".into())),
                ("why", Value::String("signIn".into())),
            ]))?;
            let resp = self
                .api
                .post(url.clone())
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
                .map_err(network)?;
            let status = resp.status();
            let headers = resp.headers().clone();

            if status.is_redirection() {
                let location = header(&headers, LOCATION.as_str())
                    .ok_or_else(|| Error::Protocol("sign-in redirect without location".into()))?;
                redirects += 1;
                if redirects > MAX_AUTH_REDIRECTS {
                    return Err(Error::Protocol("too many sign-in redirects".into()));
                }
                url = url
                    .join(location.trim())
                    .map_err(|e| Error::Protocol(format!("invalid sign-in redirect: {e}")))?;
                continue;
            }

            let data = parse_plist(&resp.bytes().await.map_err(network)?)?;
            let failure = plist_string(&data, "failureType").unwrap_or_default();
            let message = plist_string(&data, "customerMessage").unwrap_or_default();

            if attempt == 1 && failure == FAILURE_INVALID_CREDENTIALS {
                attempt += 1;
                continue;
            }
            if failure.is_empty() && message == MESSAGE_BAD_LOGIN {
                return Err(if code.is_empty() {
                    AuthError::CodeRequired.into()
                } else {
                    AuthError::BadCredentials("verification failed, try a fresh code".into()).into()
                });
            }
            if failure.is_empty() && message == MESSAGE_ACCOUNT_DISABLED {
                return Err(AuthError::BadCredentials(message).into());
            }
            if !failure.is_empty() {
                let message = if message.is_empty() {
                    format!("sign-in failed ({failure})")
                } else {
                    message
                };
                return Err(AuthError::BadCredentials(message).into());
            }
            if !status.is_success() {
                return Err(Error::Store(format!("sign-in failed with HTTP {status}")));
            }

            let missing = |what: &str| Error::Protocol(format!("sign-in response has no {what}"));
            let ds_person_id =
                plist_string(&data, "dsPersonId").ok_or_else(|| missing("dsPersonId"))?;
            let password_token =
                plist_string(&data, "passwordToken").ok_or_else(|| missing("passwordToken"))?;
            let store_front = header(&headers, "x-set-apple-store-front")
                .ok_or_else(|| missing("store front header"))?;
            let info = data.get("accountInfo").and_then(Value::as_dictionary);
            let address = info
                .and_then(|i| i.get("address"))
                .and_then(Value::as_dictionary);
            let name_part = |key: &str| {
                address
                    .and_then(|a| plist_string(a, key))
                    .unwrap_or_default()
            };
            let name = [name_part("firstName"), name_part("lastName")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");

            return Ok(Account {
                email: info
                    .and_then(|i| plist_string(i, "appleId"))
                    .unwrap_or_else(|| email.to_owned()),
                name,
                ds_person_id,
                password_token,
                store_front,
                pod: header(&headers, "pod"),
                guid,
            });
        }
    }

    /// Searches apps by term in a store country (two letter code, e.g. `US`).
    pub async fn search(&self, term: &str, country: &str, limit: u32) -> Result<Vec<StoreApp>> {
        let limit = limit.to_string();
        self.itunes(
            "/search",
            &[
                ("entity", "software"),
                ("media", "software"),
                ("limit", &limit),
                ("term", term),
                ("country", country),
            ],
        )
        .await
    }

    /// Looks up one app by id or bundle id. `None` when the store does not have it.
    pub async fn lookup(&self, key: &LookupKey, country: &str) -> Result<Option<StoreApp>> {
        let (param, value) = match key {
            LookupKey::Id(id) => ("id", id.to_string()),
            LookupKey::BundleId(bundle_id) => ("bundleId", bundle_id.clone()),
        };
        let apps = self
            .itunes(
                "/lookup",
                &[
                    ("entity", "software"),
                    (param, &value),
                    ("country", country),
                ],
            )
            .await?;
        Ok(apps.into_iter().next())
    }

    async fn itunes(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<StoreApp>> {
        let resp = self
            .api
            .get(format!("{}{path}", self.config.search_url))
            .query(query)
            .send()
            .await
            .map_err(network)?;
        if !resp.status().is_success() {
            return Err(Error::Store(format!(
                "search failed with HTTP {}",
                resp.status()
            )));
        }
        let body = resp.bytes().await.map_err(network)?;
        let results: ITunesResults = serde_json::from_slice(&body)
            .map_err(|e| crate::protocol("invalid search response", e))?;
        Ok(results.results.into_iter().map(StoreApp::from).collect())
    }

    fn buy_base(&self, account: &Account) -> String {
        match &account.pod {
            Some(pod) if self.config.buy_url == DEFAULT_BUY_URL => {
                format!("https://p{pod}-buy.itunes.apple.com")
            }
            _ => self.config.buy_url.clone(),
        }
    }

    /// Acquires a free app for the account ("Get"). Succeeds when the account
    /// already owns it. Paid apps cannot be bought this way.
    pub async fn purchase(&self, account: &Account, app_id: u64) -> Result<()> {
        if self.buy(account, app_id, "STDQ").await? {
            return Ok(());
        }
        // Apple Arcade titles are only offered with the GAME pricing parameter.
        if self.buy(account, app_id, "GAME").await? {
            return Ok(());
        }
        Err(Error::Store("this app is temporarily unavailable".into()))
    }

    /// Buys a paid app at `price` (in the store's currency, as listed by
    /// search or lookup), charging the account's payment method.
    ///
    /// Apple first answers with a confirmation dialog; show it and call again
    /// with that confirmation to complete the purchase. Free apps should use
    /// [`StoreClient::purchase`].
    pub async fn purchase_paid(
        &self,
        account: &Account,
        app_id: u64,
        price: f64,
        confirmation: Option<&PurchaseConfirmation>,
    ) -> Result<PaidPurchase> {
        let body = match confirmation {
            Some(confirmation) => {
                let url = reqwest::Url::parse(&format!("https://buy/?{}", confirmation.buy_params))
                    .map_err(|e| Error::InvalidInput(format!("buy parameters: {e}")))?;
                let mut body: Dictionary = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
                    .collect();
                body.insert("guid".into(), Value::String(account.guid.clone()));
                body
            }
            None => {
                if !(price.is_finite() && price > 0.0) {
                    return Err(Error::InvalidInput(format!("invalid price {price}")));
                }
                // Apple prices in thousandths of the currency unit: 4.99 is "4990".
                let milli = (price * 1000.0).round() as u64;
                let mut body = Self::buy_body(account, app_id, "STDQ");
                body.insert("price".into(), Value::String(milli.to_string()));
                body
            }
        };
        let (status, data) = self.send_buy(account, body).await?;
        if let Some(dialog) = data.get("dialog").and_then(Value::as_dictionary) {
            let message = plist_string(dialog, "message")
                .or_else(|| plist_string(dialog, "explanation"))
                .unwrap_or_default();
            let buy_params = dialog
                .get("okButtonAction")
                .and_then(Value::as_dictionary)
                .and_then(|action| plist_string(action, "buyParams"));
            return match buy_params {
                Some(buy_params) if confirmation.is_none() => {
                    Ok(PaidPurchase::NeedsConfirmation(PurchaseConfirmation {
                        message,
                        ok_label: plist_string(dialog, "okButtonString")
                            .unwrap_or_else(|| "Buy".into()),
                        buy_params,
                    }))
                }
                _ => Err(Error::Store(if message.is_empty() {
                    "the purchase needs an action Orchard cannot complete".into()
                } else {
                    message
                })),
            };
        }
        match Self::buy_outcome(status, &data)? {
            true => Ok(PaidPurchase::Purchased),
            false => Err(Error::Store("this app is temporarily unavailable".into())),
        }
    }

    /// Returns `false` when Apple reports the item as temporarily unavailable.
    async fn buy(&self, account: &Account, app_id: u64, pricing: &str) -> Result<bool> {
        let (status, data) = self
            .send_buy(account, Self::buy_body(account, app_id, pricing))
            .await?;
        Self::buy_outcome(status, &data)
    }

    fn buy_body(account: &Account, app_id: u64, pricing: &str) -> Dictionary {
        dict([
            ("appExtVrsId", Value::String("0".into())),
            ("hasAskedToFulfillPreorder", Value::String("true".into())),
            ("buyWithoutAuthorization", Value::String("true".into())),
            ("hasDoneAgeCheck", Value::String("true".into())),
            ("guid", Value::String(account.guid.clone())),
            ("needDiv", Value::String("0".into())),
            ("origPage", Value::String(format!("Software-{app_id}"))),
            ("origPageLocation", Value::String("Buy".into())),
            ("price", Value::String("0".into())),
            ("pricingParameters", Value::String(pricing.into())),
            ("productType", Value::String("C".into())),
            ("salableAdamId", Value::Integer(app_id.into())),
        ])
    }

    async fn send_buy(
        &self,
        account: &Account,
        body: Dictionary,
    ) -> Result<(StatusCode, Dictionary)> {
        let resp = self
            .api
            .post(format!("{}{PATH_PURCHASE}", self.buy_base(account)))
            .header(CONTENT_TYPE, "application/x-apple-plist")
            .header("iCloud-DSID", &account.ds_person_id)
            .header("X-Dsid", &account.ds_person_id)
            .header("X-Apple-Store-Front", &account.store_front)
            .header("X-Token", &account.password_token)
            .body(plist_body(body)?)
            .send()
            .await
            .map_err(network)?;
        let status = resp.status();
        let data = parse_plist(&resp.bytes().await.map_err(network)?).unwrap_or_default();
        Ok((status, data))
    }

    fn buy_outcome(status: StatusCode, data: &Dictionary) -> Result<bool> {
        let failure = plist_string(data, "failureType").unwrap_or_default();
        let message = plist_string(data, "customerMessage").unwrap_or_default();

        if failure == FAILURE_TEMPORARILY_UNAVAILABLE {
            return Ok(false);
        }
        if message == MESSAGE_SUBSCRIPTION_REQUIRED {
            return Err(Error::Store(
                "an Apple Arcade subscription is required".into(),
            ));
        }
        if [
            FAILURE_PASSWORD_TOKEN_EXPIRED,
            FAILURE_SIGN_IN_REQUIRED,
            FAILURE_DEVICE_VERIFICATION_FAILED,
        ]
        .contains(&failure.as_str())
            || message == MESSAGE_PASSWORD_CHANGED
        {
            return Err(AuthError::SessionExpired.into());
        }
        if failure == FAILURE_LICENSE_ALREADY_EXISTS {
            return Ok(true);
        }
        if !failure.is_empty() {
            return Err(Error::Store(if message.is_empty() {
                format!("purchase failed ({failure})")
            } else {
                message
            }));
        }
        // ipatool treats a bare 500 as "already purchased".
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            return Ok(true);
        }
        let doc = plist_string(data, "jingleDocType").unwrap_or_default();
        let code = data
            .get("status")
            .and_then(Value::as_signed_integer)
            .unwrap_or(0);
        if doc != "purchaseSuccess" || code != 0 {
            return Err(Error::Store(format!("purchase failed (HTTP {status})")));
        }
        Ok(true)
    }

    /// Asks for the download ticket of an app; returns the first item.
    async fn download_item(
        &self,
        account: &Account,
        app_id: u64,
        version: Option<&str>,
    ) -> Result<Dictionary> {
        let mut payload = dict([
            ("creditDisplay", Value::String(String::new())),
            ("guid", Value::String(account.guid.clone())),
            ("salableAdamId", Value::Integer(app_id.into())),
            ("serialNumber", Value::String("0".into())),
        ]);
        if let Some(version) = version {
            payload.insert("externalVersionId".into(), Value::String(version.into()));
        }
        let mut url = reqwest::Url::parse(&format!("{}{PATH_DOWNLOAD}", self.buy_base(account)))
            .map_err(|e| Error::InvalidInput(format!("buy url: {e}")))?;
        url.query_pairs_mut().append_pair("guid", &account.guid);
        let resp = self
            .api
            .post(url)
            .header(CONTENT_TYPE, "application/x-apple-plist")
            .header("iCloud-DSID", &account.ds_person_id)
            .header("X-Dsid", &account.ds_person_id)
            .body(plist_body(payload)?)
            .send()
            .await
            .map_err(network)?;
        let data = parse_plist(&resp.bytes().await.map_err(network)?)?;
        let failure = plist_string(&data, "failureType").unwrap_or_default();
        let message = plist_string(&data, "customerMessage").unwrap_or_default();
        let item = data
            .get("songList")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_dictionary)
            .cloned();

        if [
            FAILURE_PASSWORD_TOKEN_EXPIRED,
            FAILURE_SIGN_IN_REQUIRED,
            FAILURE_DEVICE_VERIFICATION_FAILED,
            FAILURE_LICENSE_ALREADY_EXISTS,
        ]
        .contains(&failure.as_str())
        {
            return Err(AuthError::SessionExpired.into());
        }
        if failure == FAILURE_LICENSE_NOT_FOUND {
            return Err(Error::LicenseRequired);
        }
        if !message.is_empty() && (!failure.is_empty() || item.is_none()) {
            return Err(Error::Store(message));
        }
        if !failure.is_empty() {
            return Err(Error::Store(format!("download failed ({failure})")));
        }
        item.ok_or_else(|| Error::Protocol("download response has no items".into()))
    }

    /// Lists the external version ids the store can deliver for an app.
    /// The account must own the app.
    pub async fn list_versions(&self, account: &Account, app_id: u64) -> Result<AppVersions> {
        let item = self.download_item(account, app_id, None).await?;
        let metadata = item
            .get("metadata")
            .and_then(Value::as_dictionary)
            .ok_or_else(|| Error::Protocol("download item has no metadata".into()))?;
        let external_version_ids = metadata
            .get("softwareVersionExternalIdentifiers")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol("metadata has no version identifiers".into()))?
            .iter()
            .filter_map(|v| match v {
                Value::Integer(i) => Some(i.to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let latest = plist_string(metadata, "softwareVersionExternalIdentifier")
            .ok_or_else(|| Error::Protocol("metadata has no latest version".into()))?;
        Ok(AppVersions {
            external_version_ids,
            latest,
        })
    }

    /// Downloads an app the account owns into `dest_dir` and returns the IPA
    /// path, named `<bundleId>_<appId>_<version>.ipa`. `version` is an
    /// external version id from [`StoreClient::list_versions`]; `None` means
    /// the latest. The IPA is MD5-verified when Apple sends a hash, then
    /// patched with `iTunesMetadata.plist` and the sinfs so it installs.
    ///
    /// Interrupted downloads (errors or a dropped future) keep their bytes in
    /// the partial directory (see [`StoreClient::with_partial_dir`]) and the
    /// next download of the same item resumes with an HTTP range request,
    /// fetching a fresh link when the stored one has expired.
    ///
    /// Fails with [`Error::LicenseRequired`] when the account does not own it.
    pub async fn download(
        &self,
        account: &Account,
        app_id: u64,
        version: Option<&str>,
        dest_dir: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<PathBuf> {
        let item = self.download_item(account, app_id, version).await?;
        let url = plist_string(&item, "URL")
            .ok_or_else(|| Error::Protocol("download item has no URL".into()))?;
        let md5 = plist_string(&item, "md5");
        let sinfs: Vec<Sinf> = item
            .get("sinfs")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_dictionary)
                    .filter_map(|s| {
                        Some(Sinf {
                            id: s.get("id").and_then(Value::as_signed_integer).unwrap_or(0),
                            data: s.get("sinf")?.as_data()?.to_vec(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut metadata = item
            .get("metadata")
            .and_then(Value::as_dictionary)
            .cloned()
            .unwrap_or_default();

        let bundle_id = plist_string(&metadata, "softwareVersionBundleId").unwrap_or_default();
        let app_version = plist_string(&metadata, "bundleShortVersionString").unwrap_or_default();
        let app_id_text = app_id.to_string();
        let file_name = [bundle_id.as_str(), &app_id_text, &app_version]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.replace(['/', '\\'], "_"))
            .collect::<Vec<_>>()
            .join("_");
        tokio::fs::create_dir_all(dest_dir).await?;
        let dest = dest_dir.join(format!("{file_name}.ipa"));
        let partial_dir = self
            .partial_dir
            .clone()
            .unwrap_or_else(|| dest_dir.join(".orchard-partial"));
        tokio::fs::create_dir_all(&partial_dir).await?;
        let _ = prune_partial_downloads(&partial_dir, PARTIAL_MAX_AGE);
        let partial = Partial::new(&partial_dir, app_id, version, md5.as_deref(), &url);

        // A resumed file that fails the MD5 check is downloaded once more from zero.
        let mut url = url;
        for attempt in 0.. {
            let (digest, resumed) = self
                .fetch_partial(
                    account,
                    app_id,
                    version,
                    &mut url,
                    md5.as_deref(),
                    &partial,
                    &mut progress,
                )
                .await?;
            match &md5 {
                Some(expected) if !digest.eq_ignore_ascii_case(expected.trim()) => {
                    partial.remove().await;
                    if !(resumed && attempt == 0) {
                        return Err(Error::Protocol(format!(
                            "downloaded IPA is corrupt (md5 {digest}, expected {expected})"
                        )));
                    }
                }
                _ => break,
            }
        }

        progress(Progress::new("patch", 0, Some(1)));
        metadata.insert("apple-id".into(), Value::String(account.email.clone()));
        metadata.insert("userName".into(), Value::String(account.email.clone()));
        let (src, dst) = (partial.data.clone(), dest.clone());
        tokio::task::spawn_blocking(move || patch_ipa(&src, &dst, Some(&metadata), &sinfs))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        partial.remove().await;
        progress(Progress::new("patch", 1, Some(1)));
        Ok(dest)
    }

    /// Completes the partial file, refreshing an expired URL once through a
    /// new download item. Returns the MD5 of the whole file and whether
    /// earlier bytes were reused.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_partial(
        &self,
        account: &Account,
        app_id: u64,
        version: Option<&str>,
        url: &mut String,
        md5: Option<&str>,
        partial: &Partial,
        progress: &mut (impl FnMut(Progress) + Send),
    ) -> Result<(String, bool)> {
        let mut refreshed = false;
        let mut restarted = false;
        loop {
            match self.fetch_range(url, md5, partial, progress).await? {
                Fetch::Done { digest, resumed } => return Ok((digest, resumed)),
                Fetch::Restart if !restarted => {
                    restarted = true;
                    partial.remove().await;
                }
                Fetch::Expired if !refreshed => {
                    refreshed = true;
                    let item = self.download_item(account, app_id, version).await?;
                    let fresh_md5 = plist_string(&item, "md5");
                    if fresh_md5.as_deref().map(str::trim) != md5.map(str::trim) {
                        partial.remove().await;
                        return Err(Error::Store(
                            "the app changed while downloading, try again".into(),
                        ));
                    }
                    *url = plist_string(&item, "URL")
                        .ok_or_else(|| Error::Protocol("download item has no URL".into()))?;
                }
                Fetch::Expired => {
                    return Err(Error::Network("the download link was rejected".into()));
                }
                Fetch::Restart => {
                    return Err(Error::Network(
                        "the server cannot resume this download".into(),
                    ));
                }
            }
        }
    }

    /// One request: resumes with `Range` when bytes are on disk, restarts
    /// from zero when the server answers 200.
    async fn fetch_range(
        &self,
        url: &str,
        md5: Option<&str>,
        partial: &Partial,
        progress: &mut (impl FnMut(Progress) + Send),
    ) -> Result<Fetch> {
        let sidecar = partial.sidecar().await;
        let matches = sidecar.as_ref().is_some_and(|s| match (md5, &s.md5) {
            (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
            _ => s.url == url,
        });
        let mut offset = if matches { partial.len().await } else { 0 };
        let known_size = sidecar.as_ref().and_then(|s| s.size);
        if known_size.is_some_and(|size| offset > size) {
            offset = 0;
        }

        let mut hasher = Md5::new();
        if offset > 0 && known_size == Some(offset) {
            partial.hash_existing(&mut hasher).await?;
            progress(Progress::new("download", offset, Some(offset)));
            return Ok(Fetch::Done {
                digest: hex(&hasher.finalize()),
                resumed: true,
            });
        }

        let mut request = self.cdn.get(url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let mut resp = request.send().await.map_err(network)?;
        let status = resp.status();
        let total = match status {
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE => {
                return Ok(Fetch::Expired);
            }
            StatusCode::RANGE_NOT_SATISFIABLE => return Ok(Fetch::Restart),
            StatusCode::PARTIAL_CONTENT if offset > 0 => {
                let range = header(resp.headers(), "content-range").unwrap_or_default();
                let (start, size) = parse_content_range(&range).unwrap_or((u64::MAX, None));
                if start != offset {
                    return Ok(Fetch::Restart);
                }
                size.or(known_size)
                    .or_else(|| resp.content_length().map(|len| offset + len))
            }
            s if s.is_success() => {
                offset = 0;
                resp.content_length()
            }
            s => {
                return Err(Error::Network(format!("download failed with HTTP {s}")));
            }
        };
        partial
            .write_sidecar(&Sidecar {
                url: url.to_owned(),
                size: total,
                md5: md5.map(str::to_owned),
            })
            .await?;

        let mut file = if offset > 0 {
            partial.hash_existing(&mut hasher).await?;
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&partial.data)
                .await?
        } else {
            tokio::fs::File::create(&partial.data).await?
        };
        let mut done = offset;
        progress(Progress::new("download", done, total));
        while let Some(chunk) = resp.chunk().await.map_err(network)? {
            // Flushed per chunk so reported bytes survive a cancellation.
            file.write_all(&chunk).await?;
            file.flush().await?;
            hasher.update(&chunk);
            done += chunk.len() as u64;
            progress(Progress::new("download", done, total));
        }
        file.flush().await?;
        file.sync_all().await?;
        if total.is_some_and(|t| t != done) {
            return Err(Error::Network("download ended early".into()));
        }
        Ok(Fetch::Done {
            digest: hex(&hasher.finalize()),
            resumed: offset > 0,
        })
    }

    /// Downloads an app, acquiring the free license first when the account
    /// does not own it yet. See [`StoreClient::download`].
    pub async fn download_app(
        &self,
        account: &Account,
        app_id: u64,
        version: Option<&str>,
        dest_dir: &Path,
        mut progress: impl FnMut(Progress) + Send,
    ) -> Result<PathBuf> {
        match self
            .download(account, app_id, version, dest_dir, &mut progress)
            .await
        {
            Err(Error::LicenseRequired) => {
                progress(Progress::new("purchase", 0, None));
                self.purchase(account, app_id).await?;
                self.download(account, app_id, version, dest_dir, &mut progress)
                    .await
            }
            other => other,
        }
    }
}
