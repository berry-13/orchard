//! Device transport layer.
//!
//! Planned services: usbmuxd discovery, lockdownd pairing and sessions,
//! AFC file access, installation_proxy, notification_proxy, syslog relay.

use orchard_core::{Error, Result, Udid};

/// Lists devices visible through usbmuxd (USB and network).
pub fn list_devices() -> Result<Vec<Udid>> {
    Err(Error::NotImplemented("usbmuxd device listing"))
}
