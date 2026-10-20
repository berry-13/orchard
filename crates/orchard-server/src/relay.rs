//! Devices on a client's network. The server is usually not on the device's
//! Wi-Fi, so the iPad app works there for it:
//!
//! - Pairing: the app announces the pairing host and relays the connection
//!   the device opens to `/api/relay/pair/{session}`, where the server runs
//!   the pairing itself.
//! - Use: the app reports the paired devices it finds on its network over
//!   `/api/relay/devices`. To reach one, the server sends the app
//!   `{type: "open", id, host, port}` there, and the app connects to the
//!   device and relays that connection over `/api/relay/stream/{id}`.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use orchard_device::{DialFuture, Dialer, PairingAnnouncement, PairingSession, ReadWrite};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};

/// A session the device has not connected to within this time is dropped.
const SESSION_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct Pairings {
    sessions: Mutex<HashMap<String, (PairingSession, Instant)>>,
}

impl Pairings {
    /// Starts a session named `name` and returns its id and announcement.
    pub fn start(&self, id: String, name: &str) -> PairingAnnouncement {
        let session = PairingSession::new(name);
        let announcement = session.announcement();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, (_, at)| at.elapsed() < SESSION_LIFETIME);
        sessions.insert(id, (session, Instant::now()));
        announcement
    }

    /// Takes a session for the device connection that arrived, once.
    pub fn take(&self, id: &str) -> Option<PairingSession> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match sessions.remove(id) {
            Some((session, at)) if at.elapsed() < SESSION_LIFETIME => Some(session),
            _ => None,
        }
    }
}

/// How long the client has to open a requested device connection.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Device connections the server asked a client for, by id, until the client
/// opens them.
#[derive(Debug, Default)]
pub struct Streams {
    pending: Mutex<HashMap<String, oneshot::Sender<DuplexStream>>>,
}

impl Streams {
    fn expect(&self, id: &str) -> oneshot::Receiver<DuplexStream> {
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id.to_owned(), tx);
        rx
    }

    fn cancel(&self, id: &str) {
        lock(&self.pending).remove(id);
    }

    /// Hands the connection the client opened to the dial waiting for it.
    pub fn deliver(&self, id: &str, stream: DuplexStream) {
        if let Some(tx) = lock(&self.pending).remove(id) {
            let _ = tx.send(stream);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Dials one device through the client that found it at `host`.
pub struct RelayDialer {
    host: String,
    /// The client's control socket.
    control: mpsc::UnboundedSender<String>,
    streams: Arc<Streams>,
}

impl RelayDialer {
    pub fn new(
        host: String,
        control: mpsc::UnboundedSender<String>,
        streams: Arc<Streams>,
    ) -> Self {
        Self {
            host,
            control,
            streams,
        }
    }
}

impl fmt::Debug for RelayDialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayDialer")
            .field("host", &self.host)
            .finish()
    }
}

impl Dialer for RelayDialer {
    fn dial(&self, port: u16) -> DialFuture<'_> {
        Box::pin(async move {
            let mut bytes = [0u8; 12];
            getrandom::fill(&mut bytes).map_err(io::Error::other)?;
            let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let opened = self.streams.expect(&id);
            let open = json!({ "type": "open", "id": id, "host": self.host, "port": port });
            if self.control.send(open.to_string()).is_err() {
                self.streams.cancel(&id);
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "the relaying client went away",
                ));
            }
            match tokio::time::timeout(DIAL_TIMEOUT, opened).await {
                Ok(Ok(stream)) => Ok(Box::new(stream) as Box<dyn ReadWrite>),
                _ => {
                    self.streams.cancel(&id);
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the relaying client did not connect",
                    ))
                }
            }
        })
    }
}

/// A byte stream over a WebSocket's binary frames, for code that wants a
/// socket. Ends when either side closes.
pub fn websocket_stream(socket: WebSocket) -> DuplexStream {
    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    tokio::spawn(async move {
        let (mut ws_tx, mut ws_rx) = socket.split();
        let (mut rd, mut wr) = tokio::io::split(theirs);
        let inbound = async {
            while let Some(Ok(message)) = ws_rx.next().await {
                match message {
                    Message::Binary(bytes) => {
                        if wr.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            let _ = wr.shutdown().await;
        };
        let outbound = async {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let frame = Message::Binary(buf[..n].to_vec().into());
                        if ws_tx.send(frame).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = ws_tx.send(Message::Close(None)).await;
        };
        // Whichever direction ends first ends the relay.
        tokio::select! {
            () = inbound => {},
            () = outbound => {},
        }
    });
    ours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_taken_once() {
        let pairings = Pairings::default();
        let a = pairings.start("s1".into(), "Orchard");
        assert_eq!(a.service_type, "_remotepairing-pairable-host._tcp");
        assert!(pairings.take("s1").is_some());
        assert!(pairings.take("s1").is_none());
        assert!(pairings.take("unknown").is_none());
    }

    #[tokio::test]
    async fn dials_through_the_client() {
        let streams = Arc::new(Streams::default());
        let (control, mut opens) = mpsc::unbounded_channel();
        let dialer = RelayDialer::new("172.20.10.2".into(), control, streams.clone());
        let client = tokio::spawn(async move {
            let open: serde_json::Value =
                serde_json::from_str(&opens.recv().await.unwrap()).unwrap();
            assert_eq!(open["type"], "open");
            assert_eq!(open["host"], "172.20.10.2");
            assert_eq!(open["port"], 49152);
            let (ours, mut theirs) = tokio::io::duplex(64);
            streams.deliver(open["id"].as_str().unwrap(), ours);
            theirs.write_all(b"hi").await.unwrap();
        });
        let mut stream = dialer.dial(49152).await.unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hi");
        client.await.unwrap();
    }

    #[tokio::test]
    async fn dial_fails_when_the_client_is_gone() {
        let (control, opens) = mpsc::unbounded_channel();
        drop(opens);
        let dialer = RelayDialer::new("h".into(), control, Arc::default());
        let err = dialer.dial(1).await.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotConnected);
    }
}
