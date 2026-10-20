use std::collections::HashMap;

use futures_util::StreamExt;
use idevice::IdeviceError;
use idevice::pairing_file::PairingFile;
use idevice::usbmuxd::{self, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice, UsbmuxdListenEvent};
use orchard_core::{Error, Result, Udid};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::error::device_error;
use crate::{Connection, Device};

/// A usbmuxd attach or detach notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "device", rename_all = "camelCase")]
pub enum DeviceEvent {
    Attached(Device),
    Detached(Device),
}

/// Live stream of [`DeviceEvent`]s. Dropping it stops the watch.
#[derive(Debug)]
pub struct DeviceWatcher {
    rx: mpsc::Receiver<Result<DeviceEvent>>,
    _stop: oneshot::Sender<()>,
}

impl DeviceWatcher {
    /// Waits for the next event. `None` once usbmuxd closed the connection or
    /// after an error was delivered.
    pub async fn next(&mut self) -> Option<Result<DeviceEvent>> {
        self.rx.recv().await
    }
}

/// Lists devices visible through usbmuxd (USB and network).
///
/// The daemon address can be overridden with `USBMUXD_SOCKET_ADDRESS`.
pub async fn list_devices() -> Result<Vec<Device>> {
    let addr = mux_addr()?;
    let mut mux = mux_connect(&addr).await?;
    Ok(list_on(&mut mux).await?.iter().map(to_device).collect())
}

/// Subscribes to usbmuxd attach and detach events.
///
/// Devices already connected are reported as `Attached` first.
pub async fn watch_devices() -> Result<DeviceWatcher> {
    let addr = mux_addr()?;
    watch_on(mux_connect(&addr).await?).await
}

pub(crate) fn mux_addr() -> Result<UsbmuxdAddr> {
    UsbmuxdAddr::from_env_var()
        .map_err(|e| Error::InvalidInput(format!("USBMUXD_SOCKET_ADDRESS: {e}")))
}

pub(crate) async fn mux_connect(addr: &UsbmuxdAddr) -> Result<UsbmuxdConnection> {
    addr.connect(0)
        .await
        .map_err(|e| device_error(&format!("usbmuxd at {addr:?}"), e))
}

pub(crate) async fn list_on(mux: &mut UsbmuxdConnection) -> Result<Vec<UsbmuxdDevice>> {
    mux.get_devices()
        .await
        .map_err(|e| device_error("usbmuxd list", e))
}

/// Reads the pairing record usbmuxd keeps for `udid`, `None` when it has none.
pub(crate) async fn read_pair_record(
    addr: &UsbmuxdAddr,
    udid: &Udid,
) -> Result<Option<PairingFile>> {
    let mut mux = mux_connect(addr).await?;
    match mux.get_pair_record(&udid.0).await {
        Ok(record) => Ok(Some(record)),
        Err(IdeviceError::UnexpectedResponse(_)) => Ok(None),
        Err(e) => Err(device_error("usbmuxd pair record", e)),
    }
}

pub(crate) fn to_device(d: &UsbmuxdDevice) -> Device {
    Device {
        udid: Udid(d.udid.clone()),
        connection: match &d.connection_type {
            usbmuxd::Connection::Usb => Connection::Usb,
            usbmuxd::Connection::Network(ip) => Connection::Network(*ip),
            usbmuxd::Connection::Unknown(kind) => Connection::Other(kind.clone()),
        },
    }
}

/// Runs a usbmuxd `Listen` on its own thread, since idevice's listen stream is
/// not `Send`, and forwards events through a channel.
pub(crate) async fn watch_on(conn: UsbmuxdConnection) -> Result<DeviceWatcher> {
    let (tx, rx) = mpsc::channel(32);
    let (ready_tx, ready_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("orchard-usbmuxd-watch".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(Error::Io(e)));
                    return;
                }
            };
            rt.block_on(listen_loop(conn, tx, ready_tx, stop_rx));
        })?;
    ready_rx
        .await
        .map_err(|_| Error::Device("usbmuxd watcher exited".into()))??;
    Ok(DeviceWatcher { rx, _stop: stop_tx })
}

async fn listen_loop(
    mut conn: UsbmuxdConnection,
    tx: mpsc::Sender<Result<DeviceEvent>>,
    ready: oneshot::Sender<Result<()>>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut stream = match conn.listen().await {
        Ok(stream) => {
            let _ = ready.send(Ok(()));
            stream
        }
        Err(e) => {
            let _ = ready.send(Err(device_error("usbmuxd listen", e)));
            return;
        }
    };
    let mut known: HashMap<u32, Device> = HashMap::new();
    loop {
        let item = tokio::select! {
            _ = &mut stop => return,
            _ = tx.closed() => return,
            item = stream.next() => item,
        };
        let event = match item {
            None => return,
            Some(Err(e)) => {
                let _ = tx.send(Err(device_error("usbmuxd listen", e))).await;
                return;
            }
            Some(Ok(UsbmuxdListenEvent::Connected(dev))) => {
                let device = to_device(&dev);
                known.insert(dev.device_id, device.clone());
                DeviceEvent::Attached(device)
            }
            Some(Ok(UsbmuxdListenEvent::Disconnected(id))) => match known.remove(&id) {
                Some(device) => DeviceEvent::Detached(device),
                None => continue,
            },
        };
        if tx.send(Ok(event)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use idevice::usbmuxd::RawPacket;
    use plist::{Dictionary, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    use super::*;

    async fn read_request(peer: &mut DuplexStream) -> Dictionary {
        let mut header = [0u8; 16];
        peer.read_exact(&mut header).await.unwrap();
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize - 16;
        let mut body = vec![0; len];
        peer.read_exact(&mut body).await.unwrap();
        plist::from_bytes(&body).unwrap()
    }

    async fn reply(peer: &mut DuplexStream, dict: Dictionary) {
        let bytes: Vec<u8> = RawPacket::new(dict, 1, 8, 0).into();
        peer.write_all(&bytes).await.unwrap();
    }

    fn dict(pairs: Vec<(&str, Value)>) -> Dictionary {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn attached(id: u64, udid: &str, network: bool) -> Dictionary {
        let mut props = dict(vec![
            ("SerialNumber", udid.into()),
            (
                "ConnectionType",
                if network { "Network" } else { "USB" }.into(),
            ),
        ]);
        if network {
            // Linux sockaddr_in: family 2, port, 192.168.1.20, zero padding.
            let mut addr = vec![2, 0, 0, 0, 192, 168, 1, 20];
            addr.resize(16, 0);
            props.insert("NetworkAddress".into(), Value::Data(addr));
        }
        dict(vec![
            ("MessageType", "Attached".into()),
            ("DeviceID", Value::Integer(id.into())),
            ("Properties", Value::Dictionary(props)),
        ])
    }

    #[tokio::test]
    async fn lists_usb_and_network_devices() {
        let (client, mut peer) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let req = read_request(&mut peer).await;
            assert_eq!(req["MessageType"].as_string(), Some("ListDevices"));
            let list = vec![
                Value::Dictionary(attached(1, "00008110-000A1B2C3D4E5F60", false)),
                Value::Dictionary(attached(2, "00008110-000A1B2C3D4E5F60", true)),
            ];
            reply(&mut peer, dict(vec![("DeviceList", Value::Array(list))])).await;
        });
        let mut mux = UsbmuxdConnection::new(Box::new(client), 0);
        let devices: Vec<Device> = list_on(&mut mux)
            .await
            .unwrap()
            .iter()
            .map(to_device)
            .collect();
        server.await.unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].connection, Connection::Usb);
        assert_eq!(
            devices[1].connection,
            Connection::Network("192.168.1.20".parse().unwrap())
        );
        assert_eq!(devices[1].udid.0, "00008110-000A1B2C3D4E5F60");
    }

    #[tokio::test]
    async fn watch_reports_attach_and_detach() {
        let (client, mut peer) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let req = read_request(&mut peer).await;
            assert_eq!(req["MessageType"].as_string(), Some("Listen"));
            reply(&mut peer, dict(vec![("Number", Value::Integer(0.into()))])).await;
            reply(&mut peer, attached(7, "UDID-A", false)).await;
            reply(&mut peer, dict(vec![("MessageType", "Paired".into())])).await;
            reply(
                &mut peer,
                dict(vec![
                    ("MessageType", "Detached".into()),
                    ("DeviceID", Value::Integer(7.into())),
                ]),
            )
            .await;
            peer
        });
        let mut watcher = watch_on(UsbmuxdConnection::new(Box::new(client), 0))
            .await
            .unwrap();
        let usb = Device {
            udid: Udid("UDID-A".into()),
            connection: Connection::Usb,
        };
        assert_eq!(
            watcher.next().await.unwrap().unwrap(),
            DeviceEvent::Attached(usb.clone())
        );
        assert_eq!(
            watcher.next().await.unwrap().unwrap(),
            DeviceEvent::Detached(usb)
        );
        let peer = server.await.unwrap();
        drop(peer);
        assert!(matches!(watcher.next().await, Some(Err(_)) | None));
    }

    #[tokio::test]
    async fn watch_fails_when_listen_is_refused() {
        let (client, mut peer) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            read_request(&mut peer).await;
            reply(&mut peer, dict(vec![("Number", Value::Integer(1.into()))])).await;
            peer
        });
        let err = watch_on(UsbmuxdConnection::new(Box::new(client), 0))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "protocol");
    }

    #[test]
    fn events_serialize_for_the_app() {
        let event = DeviceEvent::Attached(Device {
            udid: Udid("X".into()),
            connection: Connection::Usb,
        });
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"attached","device":{"udid":"X","connection":"usb"}}"#
        );
    }
}
