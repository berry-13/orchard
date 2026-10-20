//! Orchard as a network service: the same commands as the desktop app over
//! HTTP, events over a WebSocket, and the web UI, for the iPad and browser
//! clients.
//!
//! - `GET  /api/hello`              server name and version, no auth
//! - `POST /api/auth`               `{code, client}` -> `{token}`
//! - `POST /api/call/{command}`     JSON arguments -> JSON result, or `{kind, message}`
//! - `GET  /api/events`             WebSocket; send `{token}` first, then `{event, payload}` frames
//! - `POST /api/upload?name=`       raw file body -> `{path}` on the server
//! - `GET  /api/download/{ticket}`  a file or zipped folder, ticket from `download_ticket`
//! - `GET  /api/relay/pair/{id}`    WebSocket; send `{token}` first, then the bytes of the
//!   connection a device opened to the pairing host a client announced (`start_wifi_pairing`)
//! - `GET  /api/relay/devices`      WebSocket; send `{token}` first, then
//!   `{type: "devices", devices: [{name, host, port, identifier, authTag}]}` with the
//!   `_remotepairing._tcp` services on the client's network whenever they change; the
//!   server opens the ones paired with it and sends `{type: "open", id, host, port}`
//! - `GET  /api/relay/stream/{id}`  WebSocket; send `{token}` first, then the bytes of the
//!   device connection asked for with `open`
//!
//! Anyone holding a token can do everything the desktop user can, with the
//! server's file access, so tokens are only handed out for a connect code
//! shown on the server, and by default only local network peers are served.

pub mod auth;
pub mod files;
pub mod relay;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use orchard_service::Service;
use orchard_service::dto::AppInfo;
use orchard_service::error::CommandError;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

pub use auth::Auth;
pub use files::Files;

/// Largest command argument body; uploads go through `/api/upload`.
const MAX_CALL_BODY: usize = 16 * 1024 * 1024;

pub struct AppState {
    pub service: Arc<Service>,
    pub auth: Auth,
    pub files: Files,
    /// Event frames, already serialized, for every connected WebSocket.
    pub events: broadcast::Sender<String>,
    /// Serve peers outside private, loopback and VPN ranges too.
    pub allow_public: bool,
    /// Wi-Fi pairings waiting for their device, see [`relay`].
    pub pairings: relay::Pairings,
    /// Device connections asked of clients, see [`relay`].
    pub streams: Arc<relay::Streams>,
}

/// Wi-Fi pairing progress: `{sessionId, stage, pin?, device?, message?}`.
pub const WIFI_PAIRING_EVENT: &str = "orchard://wifi-pairing";

fn emit(state: &AppState, event: &str, payload: Value) {
    let _ = state
        .events
        .send(json!({ "event": event, "payload": payload }).to_string());
}

fn random_id() -> Result<String, ApiError> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(|e| CommandError::new("unknown", e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// An event sender for [`orchard_service::Events`]: frames go to every client.
pub fn event_channel() -> (broadcast::Sender<String>, orchard_service::Events) {
    let (tx, _) = broadcast::channel(1024);
    let sender = tx.clone();
    let events = orchard_service::Events::new(move |event, payload| {
        // No receivers just means no client is connected.
        let _ = sender.send(json!({ "event": event, "payload": payload }).to_string());
    });
    (tx, events)
}

/// Errors are `{kind, message}`, like the desktop app's.
struct ApiError(StatusCode, CommandError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<CommandError> for ApiError {
    fn from(e: CommandError) -> Self {
        Self(StatusCode::UNPROCESSABLE_ENTITY, e)
    }
}

impl From<orchard_core::Error> for ApiError {
    fn from(e: orchard_core::Error) -> Self {
        CommandError::from(e).into()
    }
}

fn unauthorized() -> ApiError {
    ApiError(
        StatusCode::UNAUTHORIZED,
        CommandError::new("unauthorized", "This client is not connected to the server"),
    )
}

/// Loopback, private (RFC 1918, IPv6 ULA), link local and the 100.64/10 range
/// Tailscale uses.
pub fn is_local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (a == 100 && b & 0xc0 == 64)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

async fn local_only(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if state.allow_public || is_local(peer.ip()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(token) if state.auth.check(token) => Ok(()),
        _ => Err(unauthorized()),
    }
}

fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        material: None,
    }
}

async fn hello() -> Json<Value> {
    Json(json!({ "name": "orchard", "info": app_info() }))
}

#[derive(Deserialize)]
struct AuthRequest {
    code: String,
    client: String,
}

async fn authenticate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AuthRequest>,
) -> Result<Json<Value>, ApiError> {
    let token = state
        .auth
        .redeem(&request.code, &request.client)
        .map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e.into()))?;
    Ok(Json(json!({ "token": token })))
}

fn invalid(message: impl Into<String>) -> ApiError {
    CommandError::new("invalidInput", message).into()
}

/// The download folder set in Settings, when there is one.
async fn download_dir(service: &Service) -> Option<PathBuf> {
    let settings = service.dispatch("get_settings", Value::Null).await.ok()?;
    settings["downloadDir"].as_str().map(PathBuf::from)
}

async fn call(
    State(state): State<Arc<AppState>>,
    UrlPath(command): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &headers)?;
    let args: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).map_err(|e| invalid(format!("bad arguments: {e}")))?
    };
    let reply = match command.as_str() {
        "app_info" => json!(app_info()),
        "open_data_dir" => {
            return Err(CommandError::new(
                "notImplemented",
                "The data folder is on the server; open it there",
            )
            .into());
        }
        // Remote only: where a command should write results the client will download.
        "new_export_dir" => json!(state.files.new_export_dir()?),
        "download_ticket" => {
            let path = args["path"]
                .as_str()
                .ok_or_else(|| invalid("path is required"))?;
            let ticket = state.files.ticket(
                Path::new(path),
                download_dir(&state.service).await.as_deref(),
            )?;
            json!(format!("/api/download/{ticket}"))
        }
        // Remote only: a pairing host for the client to announce on the device's Wi-Fi.
        "start_wifi_pairing" => {
            let name = args["name"].as_str().unwrap_or("Orchard");
            let id = random_id()?;
            let a = state.pairings.start(id.clone(), name);
            let txt: serde_json::Map<String, Value> = a
                .txt
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            json!({
                "sessionId": id,
                "relayPath": format!("/api/relay/pair/{id}"),
                "serviceType": a.service_type,
                "serviceName": a.service_name,
                "txt": txt,
            })
        }
        _ => state.service.dispatch(&command, args).await?,
    };
    Ok(Json(reply))
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

async fn upload(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &headers)?;
    let path = state
        .files
        .upload(&query.name, body.into_data_stream())
        .await?;
    Ok(Json(json!({ "path": path })))
}

async fn download(
    State(state): State<Arc<AppState>>,
    UrlPath(ticket): UrlPath<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let path = state.files.take(&ticket).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            CommandError::new("notFound", "This download link has expired"),
        )
    })?;
    let name = path
        .file_name()
        .map_or_else(|| "download".into(), |n| n.to_string_lossy().into_owned());
    let (file, name) = if path.is_dir() {
        let state = state.clone();
        let zip = tokio::task::spawn_blocking(move || state.files.zip_dir(&path))
            .await
            .map_err(|e| CommandError::new("unknown", e.to_string()))??;
        (zip, format!("{name}.zip"))
    } else {
        (path, name)
    };
    let mut response = ServeFile::new(file)
        .try_call(request)
        .await
        .map_err(|e| CommandError::new("io", e.to_string()))?
        .map(Body::new);
    // Quote safely: drop characters that would end or break the header value.
    let safe: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

#[derive(Deserialize)]
struct Hello {
    token: String,
}

async fn events(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state))
}

async fn close_unauthorized(mut socket: WebSocket) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 4401,
            reason: "unauthorized".into(),
        })))
        .await;
}

/// A browser WebSocket cannot send headers, so the token is the first
/// message; `None` (and the socket closed) when it is missing or wrong.
async fn authorize_socket(mut socket: WebSocket, state: &AppState) -> Option<(WebSocket, String)> {
    let first = tokio::time::timeout(Duration::from_secs(10), socket.recv()).await;
    let token = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Hello>(&text)
            .map(|hello| hello.token)
            .unwrap_or_default(),
        _ => String::new(),
    };
    if state.auth.check(&token) {
        Some((socket, token))
    } else {
        close_unauthorized(socket).await;
        None
    }
}

async fn relay_pair(
    State(state): State<Arc<AppState>>,
    UrlPath(session): UrlPath<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| run_pairing(socket, state, session))
}

/// Runs a Wi-Fi pairing over the device connection a client relays.
async fn run_pairing(socket: WebSocket, state: Arc<AppState>, session_id: String) {
    let Some((socket, _)) = authorize_socket(socket, &state).await else {
        return;
    };
    let progress = |stage: &str, extra: Value| {
        let mut payload = json!({ "sessionId": session_id, "stage": stage });
        if let (Value::Object(p), Value::Object(e)) = (&mut payload, extra) {
            p.extend(e);
        }
        emit(&state, WIFI_PAIRING_EVENT, payload);
    };
    let Some(session) = state.pairings.take(&session_id) else {
        progress(
            "failed",
            json!({ "message": "This pairing expired; start it again" }),
        );
        return;
    };
    progress("connected", json!({}));
    let store = orchard_device::WifiPairingStore::new(state.service.data_dir());
    let result = session
        .accept(relay::websocket_stream(socket), &store, |pin| {
            progress("pin", json!({ "pin": pin }));
        })
        .await;
    match result {
        Ok(device) => progress("paired", json!({ "device": device })),
        Err(e) => progress("failed", json!({ "message": e.to_string() })),
    }
}

async fn relay_devices(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| run_device_relay(socket, state))
}

async fn relay_stream(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        if let Some((socket, _)) = authorize_socket(socket, &state).await {
            state.streams.deliver(&id, relay::websocket_stream(socket));
        }
    })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum RelayReport {
    Devices { devices: Vec<RelayedService> },
}

/// A device's remote pairing service, as a client found it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelayedService {
    host: String,
    port: u16,
    /// The advertisement's TXT values, which tell whether it is a device
    /// paired with this server.
    #[serde(default)]
    identifier: String,
    #[serde(default)]
    auth_tag: String,
}

/// UDIDs of the devices a client relays; false while still opening.
type Relayed = Arc<Mutex<HashMap<String, bool>>>;

/// Lists the devices a client relays, for as long as its socket stays open.
async fn run_device_relay(socket: WebSocket, state: Arc<AppState>) {
    let Some((mut socket, token)) = authorize_socket(socket, &state).await else {
        return;
    };
    let (control, mut opens) = mpsc::unbounded_channel::<String>();
    let relayed = Relayed::default();
    let mut recheck = tokio::time::interval(Duration::from_secs(15));
    let mut revoked = false;
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !state.auth.check(&token) {
                    revoked = true;
                    break;
                }
            },
            Some(open) = opens.recv() => {
                if socket.send(Message::Text(open.into())).await.is_err() {
                    break;
                }
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(RelayReport::Devices { devices }) = serde_json::from_str(&text) {
                        sync_relayed(&state, &control, &relayed, devices);
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let udids: Vec<String> = relayed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .filter_map(|(udid, open)| open.then_some(udid))
        .collect();
    for udid in udids {
        state.service.detach_relayed_device(&udid);
    }
    if revoked {
        close_unauthorized(socket).await;
    }
}

/// Opens the devices paired with this server among those the client found,
/// and unlists the ones it no longer sees. Other devices are left alone. A
/// device that fails to open is tried again on the client's next report.
fn sync_relayed(
    state: &Arc<AppState>,
    control: &mpsc::UnboundedSender<String>,
    relayed: &Relayed,
    devices: Vec<RelayedService>,
) {
    let mut paired: HashMap<String, RelayedService> = HashMap::new();
    for device in devices {
        if let Some(udid) = state
            .service
            .advertised_device(&device.identifier, &device.auth_tag)
        {
            paired.entry(udid).or_insert(device);
        }
    }
    let mut known = relayed.lock().unwrap_or_else(|e| e.into_inner());
    let gone: Vec<String> = known
        .keys()
        .filter(|udid| !paired.contains_key(*udid))
        .cloned()
        .collect();
    for udid in gone {
        if known.remove(&udid) == Some(true) {
            state.service.detach_relayed_device(&udid);
        }
    }
    for (udid, device) in paired {
        if known.contains_key(&udid) {
            continue;
        }
        known.insert(udid.clone(), false);
        let dialer = Arc::new(relay::RelayDialer::new(
            device.host,
            control.clone(),
            state.streams.clone(),
        ));
        let (state, relayed) = (state.clone(), relayed.clone());
        tokio::spawn(async move {
            let result = state
                .service
                .attach_relayed_device(&udid, dialer, device.port)
                .await;
            let mut known = relayed.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                // The client may have gone while the device opened.
                Ok(()) => match known.get_mut(&udid) {
                    Some(open) => *open = true,
                    None => state.service.detach_relayed_device(&udid),
                },
                Err(e) => {
                    eprintln!("relayed device {udid}: {}", e.message);
                    known.remove(&udid);
                }
            }
        });
    }
}

async fn stream_events(socket: WebSocket, state: Arc<AppState>) {
    let Some((mut socket, token)) = authorize_socket(socket, &state).await else {
        return;
    };
    let mut rx = state.events.subscribe();
    // Revocation happens outside this connection; look again now and then.
    let mut recheck = tokio::time::interval(Duration::from_secs(15));
    let _ = socket
        .send(Message::Text(r#"{"event":"ready"}"#.into()))
        .await;
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !state.auth.check(&token) {
                    return close_unauthorized(socket).await;
                }
            },
            event = rx.recv() => match event {
                Ok(frame) => {
                    if socket.send(Message::Text(frame.into())).await.is_err() {
                        break;
                    }
                }
                // A slow client misses some progress frames; the next ones catch it up.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// The whole HTTP surface; `ui` is the built web app (`app/dist`).
pub fn router(state: Arc<AppState>, ui: Option<&Path>) -> Router {
    let api = Router::new()
        .route("/api/hello", get(hello))
        .route("/api/auth", post(authenticate))
        .route(
            "/api/call/{command}",
            post(call).layer(DefaultBodyLimit::max(MAX_CALL_BODY)),
        )
        .route(
            "/api/upload",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/download/{ticket}", get(download))
        .route("/api/events", get(events))
        .route("/api/relay/pair/{session}", get(relay_pair))
        .route("/api/relay/devices", get(relay_devices))
        .route("/api/relay/stream/{id}", get(relay_stream));
    let app = match ui {
        Some(ui) => {
            api.fallback_service(ServeDir::new(ui).fallback(ServeFile::new(ui.join("index.html"))))
        }
        None => api,
    };
    app
        // Bearer tokens, never cookies, so any origin (the iPad app's
        // tauri://localhost included) may call in.
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(middleware::from_fn_with_state(state.clone(), local_only))
        .with_state(state)
}
