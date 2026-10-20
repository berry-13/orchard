use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use orchard_server::{AppState, Auth, Files, event_channel, is_local, router};
use orchard_service::Service;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Server {
    app: Router,
    _dir: tempfile::TempDir,
    code: String,
}

fn server(peer: &str) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let server_dir = dir.path().join("server");
    let (events, sink) = event_channel();
    let state = Arc::new(AppState {
        service: Arc::new(Service::new(dir.path().to_path_buf(), sink).unwrap()),
        auth: Auth::open(&server_dir).unwrap(),
        files: Files::new(&server_dir).unwrap(),
        events,
        allow_public: false,
    });
    let code = Auth::new_code(&server_dir).unwrap();
    let peer: SocketAddr = peer.parse().unwrap();
    let app = router(state, None).layer(MockConnectInfo(peer));
    Server {
        app,
        _dir: dir,
        code,
    }
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn post(uri: &str, token: Option<&str>, body: impl Into<Body>) -> Request<Body> {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    request.body(body.into()).unwrap()
}

async fn connect(s: &Server) -> String {
    let body = json!({ "code": s.code, "client": "test" }).to_string();
    let (status, reply) = send(&s.app, post("/api/auth", None, body)).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn commands_need_a_token_from_the_connect_code() {
    let s = server("192.168.1.20:5000");
    let (status, reply) = send(
        &s.app,
        Request::get("/api/hello").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["name"], "orchard");

    let (status, reply) = send(&s.app, post("/api/call/get_settings", None, "{}")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply["kind"], "unauthorized");

    let bad = json!({ "code": "AAAA-AAAA", "client": "test" }).to_string();
    assert_eq!(
        send(&s.app, post("/api/auth", None, bad)).await.0,
        StatusCode::UNAUTHORIZED
    );

    let token = connect(&s).await;
    let (status, settings) = send(&s.app, post("/api/call/get_settings", Some(&token), "{}")).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert!(settings["dataDir"].is_string());

    let (status, info) = send(&s.app, post("/api/call/app_info", Some(&token), "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["material"], Value::Null);
}

#[tokio::test]
async fn command_errors_keep_kind_and_message() {
    let s = server("127.0.0.1:5000");
    let token = connect(&s).await;
    let (status, reply) = send(
        &s.app,
        post("/api/call/no_such_command", Some(&token), "{}"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply["kind"], "invalidInput");

    let (status, reply) = send(&s.app, post("/api/call/device_info", Some(&token), "{}")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        reply["message"].as_str().unwrap().contains("udid"),
        "{reply}"
    );
}

#[tokio::test]
async fn uploads_become_server_paths_and_exports_download() {
    let s = server("10.0.0.5:5000");
    let token = connect(&s).await;
    let (status, reply) = send(&s.app, post("/api/upload?name=App.ipa", Some(&token), "PK")).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert!(reply["path"].as_str().unwrap().ends_with("App.ipa"));

    let (_, dir) = send(&s.app, post("/api/call/new_export_dir", Some(&token), "")).await;
    let file = std::path::Path::new(dir.as_str().unwrap()).join("shot.png");
    std::fs::write(&file, b"png").unwrap();
    let args = json!({ "path": file }).to_string();
    let (status, url) = send(
        &s.app,
        post("/api/call/download_ticket", Some(&token), args),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{url}");
    let url = url.as_str().unwrap();

    let response = s
        .app
        .clone()
        .oneshot(Request::get(url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"shot.png\""
    );
    let again = s
        .app
        .clone()
        .oneshot(Request::get(url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND, "tickets work once");
}

#[tokio::test]
async fn public_peers_are_refused() {
    let s = server("8.8.8.8:5000");
    let response = s
        .app
        .clone()
        .oneshot(Request::get("/api/hello").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
fn local_ranges() {
    for ip in [
        "127.0.0.1",
        "192.168.0.2",
        "10.1.2.3",
        "172.16.0.1",
        "100.101.102.103",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:192.168.1.1",
    ] {
        assert!(is_local(ip.parse().unwrap()), "{ip}");
    }
    for ip in ["8.8.8.8", "100.128.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
        assert!(!is_local(ip.parse().unwrap()), "{ip}");
    }
}
