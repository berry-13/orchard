//! Device discovery, info, pairing and power.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::events::Events;
use base64::Engine;
use orchard_core::{Error, Udid};
use orchard_device::{DeviceConnection, Dialer, PairOutcome, PairingFile, WifiPairingStore};

use crate::dto::{
    Connection, Device, DeviceEvent, DeviceEventKind, DeviceInfo, PairingImport, PairingKind,
};
use crate::error::{CmdResult, CommandError};
use crate::events::emit_device;
use crate::state::DeviceState;

/// How long to wait for a device to answer before listing it as unreachable.
const INFO_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the Trust dialog may stay open.
const PAIR_WAIT: Duration = Duration::from_secs(60);

/// Marketing name for a hardware identifier, falling back to the identifier.
pub fn model_name(product_type: &str) -> String {
    orchard_device::product_name(product_type)
        .unwrap_or(product_type)
        .to_owned()
}

pub fn connection_kind(c: &orchard_device::Connection) -> Connection {
    match c {
        orchard_device::Connection::Network(_) => Connection::Wifi,
        orchard_device::Connection::Other(kind) if kind == "relay" => Connection::Wifi,
        orchard_device::Connection::Usb | orchard_device::Connection::Other(_) => Connection::Usb,
    }
}

/// One entry per UDID, preferring the USB route when a device is listed twice.
pub fn dedupe(devices: Vec<orchard_device::Device>) -> Vec<(String, Connection)> {
    let mut order = Vec::new();
    let mut seen: HashMap<String, Connection> = HashMap::new();
    for d in devices {
        let kind = connection_kind(&d.connection);
        match seen.get_mut(&d.udid.0) {
            Some(existing) => {
                if kind == Connection::Usb {
                    *existing = Connection::Usb;
                }
            }
            None => {
                order.push(d.udid.0.clone());
                seen.insert(d.udid.0, kind);
            }
        }
    }
    order
        .into_iter()
        .map(|udid| {
            let kind = seen[&udid];
            (udid, kind)
        })
        .collect()
}

pub fn to_info(info: orchard_device::DeviceInfo, connection: Connection) -> DeviceInfo {
    let (total, free) = info
        .storage
        .as_ref()
        .map_or((0, 0), |s| (s.total_bytes, s.free_bytes));
    DeviceInfo {
        device: Device {
            udid: info.udid.0,
            name: info.name,
            model: model_name(&info.product_type),
            product_type: info.product_type,
            ios_version: info.os_version,
            connection,
            paired: true,
        },
        build_version: info.build_version,
        serial_number: info.serial_number.unwrap_or_default(),
        battery_level: info.battery_level,
        battery_charging: info.battery_charging.unwrap_or(false),
        storage_total: total,
        storage_used: total.saturating_sub(free),
        wifi_sync_enabled: info.wifi_sync_enabled.unwrap_or(false),
    }
}

/// Placeholder for a device that is attached but does not answer (not trusted yet).
fn unreachable(udid: &str, connection: Connection) -> Device {
    Device {
        udid: udid.to_owned(),
        name: "Unpaired device".into(),
        model: "Tap Trust on the device".into(),
        product_type: String::new(),
        ios_version: String::new(),
        connection,
        paired: false,
    }
}

async fn query_info(state: &DeviceState, udid: &str) -> CmdResult<DeviceInfo> {
    let conn = state.connection(udid).await?;
    let info = match tokio::time::timeout(INFO_TIMEOUT, conn.info()).await {
        Ok(result) => result,
        Err(_) => Err(Error::Device(format!("device {udid} did not answer"))),
    };
    let info = match info {
        Ok(info) => info,
        Err(e) => {
            state.forget(udid);
            return Err(e.into());
        }
    };
    let info = to_info(info, connection_kind(conn.connection()));
    state.remember(&info.device);
    Ok(info)
}

fn notify(app: &Events, udid: &str) {
    emit_device(
        app,
        DeviceEvent {
            kind: DeviceEventKind::Updated,
            udid: udid.to_owned(),
        },
    );
}

pub async fn list_devices(state: &Arc<DeviceState>) -> CmdResult<Vec<Device>> {
    // Without usbmuxd only devices added over Wi-Fi are reachable.
    let mut entries = orchard_device::list_devices()
        .await
        .map(dedupe)
        .unwrap_or_default();
    for udid in state.wifi_devices() {
        if !entries.iter().any(|(u, _)| *u == udid) {
            entries.push((udid, Connection::Wifi));
        }
    }
    let mut devices = Vec::with_capacity(entries.len());
    for (udid, connection) in entries {
        let device = match state.summary(&udid) {
            Some(mut known) => {
                known.connection = connection;
                known
            }
            None => match query_info(state, &udid).await {
                Ok(info) => Device {
                    connection,
                    ..info.device
                },
                Err(_) => unreachable(&udid, connection),
            },
        };
        devices.push(device);
    }
    Ok(devices)
}

pub async fn device_info(state: &Arc<DeviceState>, udid: String) -> CmdResult<DeviceInfo> {
    query_info(state, &udid).await
}

async fn resolve(address: &str) -> CmdResult<IpAddr> {
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok(ip);
    }
    tokio::net::lookup_host((address, 62078))
        .await
        .ok()
        .and_then(|mut addrs| addrs.next())
        .map(|a| a.ip())
        .ok_or_else(|| Error::InvalidInput(format!("cannot resolve {address}")).into())
}

/// Connects to a device by address, trying each stored pairing record.
pub async fn add_wifi_device(
    app: Events,
    state: &Arc<DeviceState>,
    address: String,
) -> CmdResult<Device> {
    let ip = resolve(address.trim()).await?;
    let records = state.pairing().list()?;
    let tunnels = state.wifi_pairing().devices()?;
    if records.is_empty() && tunnels.is_empty() {
        return Err(Error::InvalidInput(
            "No paired devices yet. Pair it first: over USB, or over Wi-Fi from the iPad app (Settings, Pairing).".into(),
        )
        .into());
    }
    for record in records {
        let conn = DeviceConnection::connect_wifi(&record.udid, ip, state.pairing())?;
        if let Ok(Ok(info)) = tokio::time::timeout(INFO_TIMEOUT, conn.info()).await {
            let info = to_info(info, Connection::Wifi);
            state.add_wifi(&record.udid.0, ip, conn);
            state.remember(&info.device);
            notify(&app, &record.udid.0);
            return Ok(info.device);
        }
    }
    // Wi-Fi pairings: through a tunnel, which also works over a VPN.
    let mut failure = None;
    for (device, _) in tunnels {
        let udid = Udid(device.udid.clone());
        let conn = match DeviceConnection::connect_tunnel(&udid, ip, state.wifi_pairing()).await {
            Ok(conn) => conn,
            Err(e) => {
                failure = Some(e);
                continue;
            }
        };
        match tokio::time::timeout(INFO_TIMEOUT, conn.info()).await {
            Ok(Ok(info)) => {
                let info = to_info(info, Connection::Wifi);
                state.keep(&udid.0, conn);
                state.remember(&info.device);
                notify(&app, &udid.0);
                return Ok(info.device);
            }
            Ok(Err(e)) => failure = Some(e),
            Err(_) => {}
        }
    }
    if let Some(e) = failure {
        return Err(e.into());
    }
    Err(Error::Device(format!(
        "No paired device answered at {address}. Check that it is on the same network and has Wi-Fi sync on."
    ))
    .into())
}

/// Opens a device a client found on its network and relays, trying each
/// Wi-Fi pairing, and lists it. Returns its UDID.
pub async fn attach_relayed(
    app: Events,
    state: &Arc<DeviceState>,
    dialer: Arc<dyn Dialer>,
    port: u16,
) -> CmdResult<String> {
    let conn =
        DeviceConnection::connect_relayed(&*dialer, port, state.wifi_pairing(), None).await?;
    let info = tokio::time::timeout(INFO_TIMEOUT, conn.info())
        .await
        .map_err(|_| Error::Device("the relayed device did not answer".into()))??;
    let info = to_info(info, Connection::Wifi);
    let udid = info.device.udid.clone();
    state.add_relayed(&udid, dialer, port, conn);
    state.remember(&info.device);
    emit_device(
        &app,
        DeviceEvent {
            kind: DeviceEventKind::Attached,
            udid: udid.clone(),
        },
    );
    Ok(udid)
}

/// Unlists a relayed device whose client went away.
pub fn detach_relayed(app: &Events, state: &DeviceState, udid: &str) {
    if state.remove_relayed(udid) {
        emit_device(
            app,
            DeviceEvent {
                kind: DeviceEventKind::Detached,
                udid: udid.to_owned(),
            },
        );
    }
}

pub async fn pair_device(app: Events, state: &Arc<DeviceState>, udid: String) -> CmdResult<()> {
    let mut conn = state.open(&udid).await?;
    let outcome = conn.pair(state.pairing(), "Orchard", PAIR_WAIT).await?;
    state.forget(&udid);
    notify(&app, &udid);
    match outcome {
        PairOutcome::Paired => Ok(()),
        PairOutcome::TrustPending => {
            Err(Error::Device("Tap Trust on the device, then pair again.".into()).into())
        }
        PairOutcome::Denied => {
            Err(Error::Device("The device refused to trust this computer.".into()).into())
        }
        PairOutcome::PasscodeRequired => Err(Error::Device(
            "Unlock the device with its passcode, then pair again.".into(),
        )
        .into()),
    }
}

pub async fn unpair_device(app: Events, state: &Arc<DeviceState>, udid: String) -> CmdResult<()> {
    let mut conn = state.open(&udid).await?;
    conn.unpair(state.pairing()).await?;
    state.forget(&udid);
    notify(&app, &udid);
    Ok(())
}

pub async fn set_wifi_sync(state: &Arc<DeviceState>, udid: String, enabled: bool) -> CmdResult<()> {
    state
        .connection(&udid)
        .await?
        .set_wifi_connections(enabled)
        .await?;
    Ok(())
}

/// Returns the PNG as base64 without a data URL prefix.
pub async fn take_screenshot(state: &Arc<DeviceState>, udid: String) -> CmdResult<String> {
    let png = state.connection(&udid).await?.screenshot().await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// Writes a base64 PNG (as returned by `take_screenshot`) to `path`.
pub async fn save_screenshot(path: PathBuf, png: String) -> CmdResult<()> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.trim())
        .map_err(|e| CommandError::new("invalidInput", format!("invalid image data: {e}")))?;
    tokio::fs::write(&path, bytes).await?;
    Ok(())
}

pub async fn restart_device(state: &Arc<DeviceState>, udid: String) -> CmdResult<()> {
    state.connection(&udid).await?.restart().await?;
    state.forget(&udid);
    Ok(())
}

pub async fn shutdown_device(state: &Arc<DeviceState>, udid: String) -> CmdResult<()> {
    state.connection(&udid).await?.shutdown().await?;
    state.forget(&udid);
    Ok(())
}

/// Stores a pairing record made elsewhere: a classic lockdown record (from
/// another computer) or an iOS 27 Wi-Fi pairing record (from an app such as
/// StikPair), whichever the file is.
pub async fn import_pairing_file(
    state: &Arc<DeviceState>,
    path: String,
) -> CmdResult<PairingImport> {
    let bytes = tokio::fs::read(&path).await?;
    if orchard_device::is_wifi_pairing(&bytes) {
        let stored = WifiPairingStore::new(state.data_dir()).import(&bytes)?;
        return Ok(PairingImport {
            kind: PairingKind::Wifi,
            id: stored.identifier,
        });
    }
    let record = PairingFile::from_bytes(&bytes)
        .map_err(|_| Error::InvalidInput("this is not a pairing record Orchard knows".into()))?;
    let udid = record.udid.clone().ok_or_else(|| {
        Error::InvalidInput("the pairing record does not say which device it is for".into())
    })?;
    state
        .pairing()
        .import(&orchard_core::Udid(udid.clone()), &bytes)?;
    state.forget(&udid);
    Ok(PairingImport {
        kind: PairingKind::Lockdown,
        id: udid,
    })
}

#[cfg(test)]
mod tests {
    use orchard_core::Udid;

    use super::*;

    fn dev(udid: &str, connection: orchard_device::Connection) -> orchard_device::Device {
        orchard_device::Device {
            udid: Udid(udid.into()),
            connection,
        }
    }

    #[test]
    fn dedupe_prefers_usb_and_keeps_order() {
        let net = orchard_device::Connection::Network("192.168.1.2".parse().unwrap());
        let list = dedupe(vec![
            dev("a", net.clone()),
            dev("b", net.clone()),
            dev("a", orchard_device::Connection::Usb),
            dev("c", orchard_device::Connection::Usb),
            dev("c", net),
        ]);
        assert_eq!(
            list,
            vec![
                ("a".to_owned(), Connection::Usb),
                ("b".to_owned(), Connection::Wifi),
                ("c".to_owned(), Connection::Usb),
            ]
        );
    }
}
