//! Device transport layer on top of the `idevice` crate.
//!
//! [`list_devices`] and [`watch_devices`] talk to usbmuxd. [`DeviceConnection`]
//! opens services on one device, over USB, the usbmuxd network entry, or
//! direct Wi-Fi with a pairing record kept in a [`PairingStore`]. Everything
//! else is a method on [`DeviceConnection`]: info, pairing, apps, files,
//! screenshots, syslog and power actions.

mod apps;
mod connect;
mod diagnostics;
mod discovery;
mod error;
mod files;
mod info;
mod models;
mod mux;
mod pairing;
mod syslog;
mod wifi_pairing;

use std::fmt;
use std::net::IpAddr;

use orchard_core::Udid;
use serde::{Deserialize, Serialize};

pub use apps::{AppInfo, AppKind};
pub use connect::DeviceConnection;
pub use discovery::{WifiDevice, discover_wifi_devices};
pub use files::{
    DeviceFiles, FileEntry, FileKind, FileLocation, StorageInfo, join_path, normalize_path,
};
pub use idevice::pairing_file::PairingFile;
pub use info::{DeviceInfo, Storage};
pub use models::product_name;
pub use mux::{DeviceEvent, DeviceWatcher, list_devices, watch_devices};
pub use pairing::{PairOutcome, PairingStore, StoredPairing};
pub use syslog::SyslogStream;
pub use wifi_pairing::{WifiPairing, WifiPairingStore, is_wifi_pairing};

/// How a device is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Connection {
    /// USB through usbmuxd.
    Usb,
    /// Network, through usbmuxd or a direct TCP connection.
    Network(IpAddr),
    Other(String),
}

impl fmt::Display for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usb => f.write_str("usb"),
            Self::Network(ip) => write!(f, "network {ip}"),
            Self::Other(kind) => f.write_str(kind),
        }
    }
}

/// A device as listed by usbmuxd. A device on both USB and Wi-Fi appears twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub udid: Udid,
    pub connection: Connection,
}
