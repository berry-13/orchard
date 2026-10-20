mod common;

use common::*;
use md5::{Digest, Md5};
use orchard_core::{AuthError, Error, Progress};
use orchard_ipa::{Account, LookupKey, StoreClient, StoreConfig, inspect};
use plist::{Dictionary, Value};
use wiremock::matchers::{body_string_contains, header, headers, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOWNLOAD_PATH: &str = "/WebObjects/MZFinance.woa/wa/volumeStoreDownloadProduct";
const PURCHASE_PATH: &str = "/WebObjects/MZFinance.woa/wa/buyProduct";
const APP_ID: u64 = 1_234_567;

fn client(server: &MockServer) -> StoreClient {
    StoreClient::with_config(StoreConfig {
        auth_url: format!("{}/auth", server.uri()),
        buy_url: server.uri(),
        search_url: server.uri(),
        ..StoreConfig::default()
    })
    .unwrap()
}

fn plist_response(status: u16, dict: Dictionary) -> ResponseTemplate {
    let mut body = Vec::new();
    Value::Dictionary(dict).to_writer_xml(&mut body).unwrap();
    ResponseTemplate::new(status).set_body_raw(body, "text/xml")
}

fn d<const N: usize>(entries: [(&str, Value); N]) -> Dictionary {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

fn account() -> Account {
    Account {
        email: "user@example.com".into(),
        name: "Jane Appleseed".into(),
        ds_person_id: "8888".into(),
        password_token: "token".into(),
        store_front: "143441-1,29".into(),
        pod: None,
        guid: "0123456789AB".into(),
    }
}

#[tokio::test]
async fn sign_in_follows_redirect_and_asks_for_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .and(query_param("guid", "AABBCCDDEEFF"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "/pod25/auth?guid=AABBCCDDEEFF"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/pod25/auth"))
        .and(body_string_contains("<string>hunter2123456</string>"))
        .respond_with(
            plist_response(
                200,
                d([
                    ("dsPersonId", Value::Integer(8888.into())),
                    ("passwordToken", "tok-1".into()),
                    (
                        "accountInfo",
                        Value::Dictionary(d([
                            ("appleId", "user@example.com".into()),
                            (
                                "address",
                                Value::Dictionary(d([
                                    ("firstName", "Jane".into()),
                                    ("lastName", "Appleseed".into()),
                                ])),
                            ),
                        ])),
                    ),
                ]),
            )
            .insert_header("X-Set-Apple-Store-Front", "143441-1,29")
            .insert_header("pod", "25"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/pod25/auth"))
        .and(body_string_contains("<string>hunter2</string>"))
        .respond_with(plist_response(
            200,
            d([(
                "customerMessage",
                "MZFinance.BadLogin.Configurator_message".into(),
            )]),
        ))
        .mount(&server)
        .await;

    let store = client(&server);
    let err = store
        .sign_in("user@example.com", "hunter2", None, Some("AABBCCDDEEFF"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Auth(AuthError::CodeRequired)),
        "{err:?}"
    );

    let account = store
        .sign_in(
            "user@example.com",
            "hunter2",
            Some("123 456"),
            Some("AABBCCDDEEFF"),
        )
        .await
        .unwrap();
    assert_eq!(
        account,
        Account {
            email: "user@example.com".into(),
            name: "Jane Appleseed".into(),
            ds_person_id: "8888".into(),
            password_token: "tok-1".into(),
            store_front: "143441-1,29".into(),
            pod: Some("25".into()),
            guid: "AABBCCDDEEFF".into(),
        }
    );
}

#[tokio::test]
async fn sign_in_retries_once_then_reports_bad_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth"))
        .respond_with(plist_response(
            200,
            d([
                ("failureType", "-5000".into()),
                (
                    "customerMessage",
                    "Your Apple ID or password was entered incorrectly.".into(),
                ),
            ]),
        ))
        .expect(2)
        .mount(&server)
        .await;

    let err = client(&server)
        .sign_in("user@example.com", "wrong", None, None)
        .await
        .unwrap_err();
    match err {
        Error::Auth(AuthError::BadCredentials(msg)) => assert!(msg.contains("incorrectly")),
        other => panic!("unexpected {other:?}"),
    }
    let attempt2 = server.received_requests().await.unwrap()[1].body.clone();
    assert!(
        String::from_utf8(attempt2)
            .unwrap()
            .contains("<string>2</string>")
    );
}

#[tokio::test]
async fn search_and_lookup() {
    let server = MockServer::start().await;
    let app = serde_json::json!({
        "trackId": APP_ID,
        "bundleId": "com.example.demo",
        "trackName": "Demo",
        "version": "1.2.3",
        "price": 0.0,
        "formattedPrice": "Free",
        "artworkUrl512": "https://example.com/512.png",
        "artworkUrl100": "https://example.com/100.png",
        "sellerName": "Example Inc.",
        "fileSizeBytes": "123456",
        "averageUserRating": 4.5,
        "userRatingCount": 10,
        "primaryGenreName": "Utilities",
        "minimumOsVersion": "15.0",
        "releaseNotes": "Fixes"
    });
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("term", "demo app"))
        .and(query_param("country", "IT"))
        .and(query_param("entity", "software"))
        .and(query_param("limit", "5"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"resultCount": 1, "results": [app]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lookup"))
        .and(query_param("bundleId", "com.example.demo"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"resultCount": 1, "results": [app]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lookup"))
        .and(query_param("id", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"resultCount": 0, "results": []})),
        )
        .mount(&server)
        .await;

    let store = client(&server);
    let results = store.search("demo app", "IT", 5).await.unwrap();
    assert_eq!(results.len(), 1);
    let found = &results[0];
    assert_eq!(found.id, APP_ID);
    assert_eq!(found.bundle_id, "com.example.demo");
    assert_eq!(found.formatted_price.as_deref(), Some("Free"));
    assert_eq!(
        found.artwork_url.as_deref(),
        Some("https://example.com/512.png")
    );
    assert_eq!(found.file_size, Some(123_456));
    assert_eq!(found.rating, Some(4.5));

    let looked_up = store
        .lookup(&LookupKey::BundleId("com.example.demo".into()), "US")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(looked_up.name, "Demo");
    assert!(
        store
            .lookup(&LookupKey::Id(1), "US")
            .await
            .unwrap()
            .is_none()
    );
}

fn download_item(server: &MockServer, ipa: &[u8], sinfs: usize) -> Dictionary {
    let md5: String = Md5::digest(ipa)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sinfs = (0..sinfs)
        .map(|i| {
            Value::Dictionary(d([
                ("id", Value::Integer((i as u64).into())),
                ("sinf", Value::Data(format!("sinf-{i}").into_bytes())),
            ]))
        })
        .collect();
    d([(
        "songList",
        Value::Array(vec![Value::Dictionary(d([
            ("URL", format!("{}/cdn/demo.ipa", server.uri()).into()),
            ("md5", md5.into()),
            ("sinfs", Value::Array(sinfs)),
            (
                "metadata",
                Value::Dictionary(d([
                    ("softwareVersionBundleId", "com.example.demo".into()),
                    ("bundleShortVersionString", "1.2.3".into()),
                    ("itemId", Value::Integer(APP_ID.into())),
                    (
                        "softwareVersionExternalIdentifiers",
                        Value::Array(vec![Value::Integer(100.into()), Value::Integer(200.into())]),
                    ),
                    (
                        "softwareVersionExternalIdentifier",
                        Value::Integer(200.into()),
                    ),
                ])),
            ),
        ]))]),
    )])
}

#[tokio::test]
async fn download_app_purchases_then_downloads_and_patches() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());

    // First ticket request: not owned yet.
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .and(query_param("guid", "0123456789AB"))
        .and(header("X-Dsid", "8888"))
        .respond_with(plist_response(200, d([("failureType", "9610".into())])))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .and(body_string_contains("<key>externalVersionId</key>"))
        .respond_with(plist_response(200, download_item(&server, &ipa, 1)))
        .mount(&server)
        .await;
    // Regular App Store pricing is unavailable, Arcade pricing succeeds.
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .and(body_string_contains("STDQ"))
        .and(header("X-Token", "token"))
        .and(headers("X-Apple-Store-Front", vec!["143441-1", "29"]))
        .respond_with(plist_response(200, d([("failureType", "2059".into())])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .and(body_string_contains("GAME"))
        .respond_with(plist_response(
            200,
            d([
                ("jingleDocType", "purchaseSuccess".into()),
                ("status", Value::Integer(0.into())),
            ]),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ipa.clone()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut events: Vec<Progress> = Vec::new();
    let dest = client(&server)
        .download_app(&account(), APP_ID, Some("200"), dir.path(), |p| {
            events.push(p)
        })
        .await
        .unwrap();

    assert_eq!(dest, dir.path().join("com.example.demo_1234567_1.2.3.ipa"));
    assert_eq!(
        files(dir.path()),
        ["com.example.demo_1234567_1.2.3.ipa"],
        "no leftovers"
    );

    let last_download = events.iter().rev().find(|p| p.stage == "download").unwrap();
    assert_eq!(last_download.done, ipa.len() as u64);
    assert_eq!(last_download.total, Some(ipa.len() as u64));
    assert!(events.iter().any(|p| p.stage == "purchase"));
    assert!(events.iter().any(|p| p.stage == "patch" && p.done == 1));

    let patched = read_entries(&std::fs::read(&dest).unwrap());
    let original = read_entries(&ipa);
    for entry in &original {
        assert!(patched.contains(entry), "{} preserved", entry.0);
    }
    let get = |name: &str| {
        patched
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
    };
    assert_eq!(
        get("Payload/Demo.app/SC_Info/Demo.sinf").unwrap(),
        b"sinf-0"
    );
    let metadata: Dictionary = plist::from_bytes(&get("iTunesMetadata.plist").unwrap()).unwrap();
    assert_eq!(
        metadata.get("apple-id").and_then(Value::as_string),
        Some("user@example.com")
    );
    assert_eq!(
        metadata.get("userName").and_then(Value::as_string),
        Some("user@example.com")
    );
    assert_eq!(
        metadata
            .get("softwareVersionBundleId")
            .and_then(Value::as_string),
        Some("com.example.demo")
    );

    let info = inspect(&dest).unwrap();
    assert!(info.has_sinf && info.has_itunes_metadata);
    assert_eq!(info.bundle_id, "com.example.demo");
}

#[tokio::test]
async fn download_uses_manifest_sinf_paths_and_lists_versions() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec {
        manifest_sinf_paths: Some(vec![
            "SC_Info/Demo.sinf".into(),
            "PlugIns/Ext.appex/SC_Info/Ext.sinf".into(),
        ]),
        ..IpaSpec::default()
    });
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, download_item(&server, &ipa, 2)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ipa.clone()))
        .mount(&server)
        .await;

    let store = client(&server);
    let versions = store.list_versions(&account(), APP_ID).await.unwrap();
    assert_eq!(versions.external_version_ids, ["100", "200"]);
    assert_eq!(versions.latest, "200");

    let dir = tempfile::tempdir().unwrap();
    let dest = store
        .download(&account(), APP_ID, None, dir.path(), |_| {})
        .await
        .unwrap();
    let patched = read_entries(&std::fs::read(&dest).unwrap());
    let get = |name: &str| {
        patched
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
    };
    assert_eq!(
        get("Payload/Demo.app/SC_Info/Demo.sinf").unwrap(),
        b"sinf-0"
    );
    assert_eq!(
        get("Payload/Demo.app/PlugIns/Ext.appex/SC_Info/Ext.sinf").unwrap(),
        b"sinf-1"
    );
}

#[tokio::test]
async fn download_rejects_md5_mismatch() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());
    let mut item = download_item(&server, &ipa, 1);
    let song = item.get_mut("songList").unwrap().as_array_mut().unwrap()[0]
        .as_dictionary_mut()
        .unwrap();
    song.insert("md5".into(), "00000000000000000000000000000000".into());
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, item))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ipa))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let err = client(&server)
        .download(&account(), APP_ID, None, dir.path(), |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Protocol(ref m) if m.contains("md5")),
        "{err:?}"
    );
    assert!(files(dir.path()).is_empty());
}

#[tokio::test]
async fn store_errors_map_to_core_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .respond_with(plist_response(200, d([("failureType", "2034".into())])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .and(body_string_contains("<integer>1</integer>"))
        .respond_with(plist_response(200, d([("failureType", "9610".into())])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .and(body_string_contains("<integer>2</integer>"))
        .respond_with(plist_response(
            200,
            d([(
                "customerMessage",
                "This item is no longer available.".into(),
            )]),
        ))
        .mount(&server)
        .await;

    let store = client(&server);
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        store.purchase(&account(), 1).await,
        Err(Error::Auth(AuthError::SessionExpired))
    ));
    assert!(matches!(
        store
            .download(&account(), 1, None, dir.path(), |_| {})
            .await,
        Err(Error::LicenseRequired)
    ));
    assert!(matches!(
        store.download(&account(), 2, None, dir.path(), |_| {}).await,
        Err(Error::Store(ref m)) if m.contains("no longer available")
    ));
}

#[tokio::test]
async fn purchase_treats_existing_license_as_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .respond_with(plist_response(200, d([("failureType", "5002".into())])))
        .expect(1)
        .mount(&server)
        .await;
    client(&server).purchase(&account(), APP_ID).await.unwrap();
}

/// Files under `dir`, relative, sorted.
fn files(dir: &std::path::Path) -> Vec<String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn item_with_url(server: &MockServer, ipa: &[u8], url: &str) -> Dictionary {
    let mut item = download_item(server, ipa, 1);
    let song = item.get_mut("songList").unwrap().as_array_mut().unwrap()[0]
        .as_dictionary_mut()
        .unwrap();
    song.insert("URL".into(), url.into());
    item
}

/// Serves `prefix` of a `total` byte body and then stalls; returns its URL.
async fn stalling_cdn(prefix: Vec<u8>, total: usize) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/stall.ipa", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n");
        sock.write_all(head.as_bytes()).await.unwrap();
        sock.write_all(&prefix).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    url
}

/// Starts a download that stalls after `prefix`, then drops it, leaving a partial.
/// The stall item is served once, ahead of mocks the caller mounts later.
async fn leave_partial(server: &MockServer, dir: &std::path::Path, ipa: &[u8], prefix: Vec<u8>) {
    let url = stalling_cdn(prefix, ipa.len()).await;
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, item_with_url(server, ipa, &url)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let store = client(server);
    let acc = account();
    let download = store.download(&acc, APP_ID, None, dir, |p| {
        if p.done > 0
            && let Some(tx) = tx.take()
        {
            let _ = tx.send(());
        }
    });
    tokio::select! {
        _ = download => panic!("download should stall"),
        _ = rx => {}
    }
}

fn range_response(ipa: &[u8], from: usize) -> ResponseTemplate {
    ResponseTemplate::new(206)
        .insert_header(
            "Content-Range",
            format!("bytes {from}-{}/{}", ipa.len() - 1, ipa.len()),
        )
        .set_body_bytes(ipa[from..].to_vec())
}

async fn assert_patched(dest: &std::path::Path) {
    let info = inspect(dest).unwrap();
    assert!(info.has_sinf && info.has_itunes_metadata);
}

const PREFIX: usize = 1000;

#[tokio::test]
async fn cancelled_download_resumes_with_range() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());
    let dir = tempfile::tempdir().unwrap();
    leave_partial(&server, dir.path(), &ipa, ipa[..PREFIX].to_vec()).await;

    // The partial stays in the cache dir, never next to the IPA.
    let left = files(dir.path());
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.iter().all(|f| f.starts_with(".orchard-partial/")));
    let part = left.iter().find(|f| f.ends_with(".part")).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(part)).unwrap(),
        &ipa[..PREFIX]
    );

    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, download_item(&server, &ipa, 1)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .and(header("Range", format!("bytes={PREFIX}-").as_str()))
        .respond_with(range_response(&ipa, PREFIX))
        .expect(1)
        .mount(&server)
        .await;

    let mut events: Vec<Progress> = Vec::new();
    let dest = client(&server)
        .download(&account(), APP_ID, None, dir.path(), |p| events.push(p))
        .await
        .unwrap();
    assert_eq!(
        events[0],
        Progress::new("download", PREFIX as u64, Some(ipa.len() as u64))
    );
    assert_patched(&dest).await;
    assert_eq!(files(dir.path()), ["com.example.demo_1234567_1.2.3.ipa"]);
}

#[tokio::test]
async fn server_ignoring_range_restarts_from_zero() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());
    let dir = tempfile::tempdir().unwrap();
    leave_partial(&server, dir.path(), &ipa, ipa[..PREFIX].to_vec()).await;

    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, download_item(&server, &ipa, 1)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ipa.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let mut events: Vec<Progress> = Vec::new();
    let dest = client(&server)
        .download(&account(), APP_ID, None, dir.path(), |p| events.push(p))
        .await
        .unwrap();
    assert_eq!(
        events[0],
        Progress::new("download", 0, Some(ipa.len() as u64))
    );
    assert_patched(&dest).await;
    assert_eq!(files(dir.path()), ["com.example.demo_1234567_1.2.3.ipa"]);
}

#[tokio::test]
async fn corrupt_partial_fails_md5_and_restarts() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());
    let dir = tempfile::tempdir().unwrap();
    leave_partial(&server, dir.path(), &ipa, vec![0xEE; PREFIX]).await;

    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, download_item(&server, &ipa, 1)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .and(header("Range", format!("bytes={PREFIX}-").as_str()))
        .respond_with(range_response(&ipa, PREFIX))
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/demo.ipa"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ipa.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let dest = client(&server)
        .download(&account(), APP_ID, None, dir.path(), |_| {})
        .await
        .unwrap();
    assert_patched(&dest).await;
    assert_eq!(files(dir.path()), ["com.example.demo_1234567_1.2.3.ipa"]);
}

#[tokio::test]
async fn expired_url_is_refreshed_and_still_resumes() {
    let server = MockServer::start().await;
    let ipa = build_ipa(&IpaSpec::default());
    let dir = tempfile::tempdir().unwrap();
    leave_partial(&server, dir.path(), &ipa, ipa[..PREFIX].to_vec()).await;

    let old = format!("{}/cdn/old.ipa", server.uri());
    let new = format!("{}/cdn/new.ipa", server.uri());
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, item_with_url(&server, &ipa, &old)))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DOWNLOAD_PATH))
        .respond_with(plist_response(200, item_with_url(&server, &ipa, &new)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/old.ipa"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/new.ipa"))
        .and(header("Range", format!("bytes={PREFIX}-").as_str()))
        .respond_with(range_response(&ipa, PREFIX))
        .expect(1)
        .mount(&server)
        .await;

    let dest = client(&server)
        .download(&account(), APP_ID, None, dir.path(), |_| {})
        .await
        .unwrap();
    assert_patched(&dest).await;
    assert_eq!(files(dir.path()), ["com.example.demo_1234567_1.2.3.ipa"]);
}

#[test]
fn prune_removes_only_stale_partials() {
    use orchard_ipa::{PARTIAL_MAX_AGE, prune_partial_downloads};
    use std::time::{Duration, SystemTime};

    let dir = tempfile::tempdir().unwrap();
    let old = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
    for name in ["old.part", "old.json", "other.txt"] {
        let file = std::fs::File::create(dir.path().join(name)).unwrap();
        file.set_modified(old).unwrap();
    }
    std::fs::write(dir.path().join("fresh.part"), b"x").unwrap();

    assert_eq!(
        prune_partial_downloads(dir.path(), PARTIAL_MAX_AGE).unwrap(),
        2
    );
    assert_eq!(files(dir.path()), ["fresh.part", "other.txt"]);
    assert_eq!(
        prune_partial_downloads(&dir.path().join("missing"), PARTIAL_MAX_AGE).unwrap(),
        0
    );
}

#[tokio::test]
async fn paid_purchase_asks_for_confirmation_then_buys() {
    use orchard_ipa::PaidPurchase;

    let server = MockServer::start().await;
    let dialog = d([
        ("message", "Buy Demo for $4.99?".into()),
        ("okButtonString", "Buy".into()),
        (
            "okButtonAction",
            Value::Dictionary(d([
                ("kind", "Buy".into()),
                (
                    "buyParams",
                    "productType=C&price=4990&salableAdamId=1234567&pricingParameters=STDQ&appExtVrsId=99"
                        .into(),
                ),
            ])),
        ),
    ]);
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .and(body_string_contains("<string>4990</string>"))
        .and(body_string_contains("buyWithoutAuthorization"))
        .respond_with(plist_response(
            200,
            d([("dialog", Value::Dictionary(dialog))]),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .and(body_string_contains("<key>appExtVrsId</key>"))
        .and(body_string_contains("<string>99</string>"))
        .and(body_string_contains("0123456789AB"))
        .respond_with(plist_response(
            200,
            d([
                ("jingleDocType", "purchaseSuccess".into()),
                ("status", Value::Integer(0.into())),
            ]),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let store = client(&server);
    let confirmation = match store
        .purchase_paid(&account(), APP_ID, 4.99, None)
        .await
        .unwrap()
    {
        PaidPurchase::NeedsConfirmation(c) => c,
        other => panic!("expected a confirmation, got {other:?}"),
    };
    assert_eq!(confirmation.message, "Buy Demo for $4.99?");
    assert_eq!(confirmation.ok_label, "Buy");
    let done = store
        .purchase_paid(&account(), APP_ID, 4.99, Some(&confirmation))
        .await
        .unwrap();
    assert_eq!(done, PaidPurchase::Purchased);
}

#[tokio::test]
async fn paid_purchase_reports_payment_problems() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PURCHASE_PATH))
        .respond_with(plist_response(
            200,
            d([
                ("failureType", "1010".into()),
                (
                    "customerMessage",
                    "Your payment method was declined.".into(),
                ),
            ]),
        ))
        .mount(&server)
        .await;
    let err = client(&server)
        .purchase_paid(&account(), APP_ID, 0.99, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Store(m) if m == "Your payment method was declined."));
    let err = client(&server)
        .purchase_paid(&account(), APP_ID, 0.0, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}
