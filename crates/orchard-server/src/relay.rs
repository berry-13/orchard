//! Wi-Fi pairing through a client's network. The server is usually not on
//! the device's Wi-Fi, so the iPad app announces the pairing host there and
//! relays the connection the device opens to `/api/relay/pair/{session}`,
//! where the server runs the pairing itself.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use orchard_device::{PairingAnnouncement, PairingSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

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
}
