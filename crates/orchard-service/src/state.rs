//! Long lived backend state shared by the commands.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::events::Events;
use orchard_core::{Error, Udid};
use orchard_device::{DeviceConnection, Dialer, PairingStore, WifiPairingStore};
use orchard_ipa::AppStore;

use crate::dto::{self, DeviceEvent, DeviceEventKind};
use crate::error::{CmdResult, CommandError};
use crate::events::emit_device;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How a relayed device is reached: the dialer and its remote pairing port.
type Relay = (Arc<dyn Dialer>, u16);

/// Device connections and what Orchard knows about each device.
pub struct DeviceState {
    data_dir: PathBuf,
    pairing: PairingStore,
    wifi_pairing: WifiPairingStore,
    /// Open connections, dropped when the device detaches.
    connections: Mutex<HashMap<String, Arc<DeviceConnection>>>,
    /// Devices added by address with "Add Wi-Fi device".
    wifi: Mutex<HashMap<String, IpAddr>>,
    /// Devices a client on their network relays, with the dialer and the
    /// port of their remote pairing service.
    relayed: Mutex<HashMap<String, Relay>>,
    /// Last known summary per device, so listing does not query every device.
    summaries: Mutex<HashMap<String, dto::Device>>,
}

impl DeviceState {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            pairing: PairingStore::new(data_dir),
            wifi_pairing: WifiPairingStore::new(data_dir),
            connections: Mutex::default(),
            wifi: Mutex::default(),
            relayed: Mutex::default(),
            summaries: Mutex::default(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn pairing(&self) -> &PairingStore {
        &self.pairing
    }

    pub fn wifi_pairing(&self) -> &WifiPairingStore {
        &self.wifi_pairing
    }

    /// A cached connection to `udid`, opening one when needed.
    pub async fn connection(&self, udid: &str) -> CmdResult<Arc<DeviceConnection>> {
        if let Some(conn) = lock(&self.connections).get(udid) {
            return Ok(conn.clone());
        }
        let conn = Arc::new(self.open(udid).await?);
        lock(&self.connections).insert(udid.to_owned(), conn.clone());
        Ok(conn)
    }

    /// A fresh, uncached connection (pairing needs exclusive access).
    pub async fn open(&self, udid: &str) -> CmdResult<DeviceConnection> {
        let id = Udid(udid.to_owned());
        let relayed = lock(&self.relayed).get(udid).cloned();
        if let Some((dialer, port)) = relayed {
            let conn =
                DeviceConnection::connect_relayed(&*dialer, port, &self.wifi_pairing, Some(&id))
                    .await?;
            return Ok(conn);
        }
        let manual = lock(&self.wifi).get(udid).copied();
        let tunnel = self.wifi_pairing.find(udid).ok().flatten();
        match DeviceConnection::connect(&id, &self.pairing).await {
            Ok(conn) => Ok(conn),
            Err(e) => match (tunnel.and_then(|(d, _)| d.address), manual) {
                (Some(addr), _) => {
                    Ok(DeviceConnection::connect_tunnel(&id, addr, &self.wifi_pairing).await?)
                }
                (None, Some(addr)) => Ok(DeviceConnection::connect_wifi(&id, addr, &self.pairing)?),
                (None, None) => Err(e.into()),
            },
        }
    }

    /// Forgets the cached connection and summary of `udid`.
    pub fn forget(&self, udid: &str) {
        lock(&self.connections).remove(udid);
        lock(&self.summaries).remove(udid);
    }

    pub fn add_wifi(&self, udid: &str, addr: IpAddr, conn: DeviceConnection) {
        lock(&self.wifi).insert(udid.to_owned(), addr);
        self.keep(udid, conn);
    }

    /// Caches `conn` as the connection to `udid`.
    pub fn keep(&self, udid: &str, conn: DeviceConnection) {
        lock(&self.connections).insert(udid.to_owned(), Arc::new(conn));
    }

    /// Starts relaying `udid` through `dialer`, with `conn` already open.
    pub fn add_relayed(
        &self,
        udid: &str,
        dialer: Arc<dyn Dialer>,
        port: u16,
        conn: DeviceConnection,
    ) {
        lock(&self.relayed).insert(udid.to_owned(), (dialer, port));
        self.keep(udid, conn);
    }

    /// Stops relaying `udid`; false when it was not relayed.
    pub fn remove_relayed(&self, udid: &str) -> bool {
        let removed = lock(&self.relayed).remove(udid).is_some();
        if removed {
            self.forget(udid);
        }
        removed
    }

    /// Devices added by address or relayed, including Wi-Fi pairings with a
    /// known address.
    pub fn wifi_devices(&self) -> Vec<String> {
        let mut udids: Vec<String> = lock(&self.wifi).keys().cloned().collect();
        for udid in lock(&self.relayed).keys() {
            if !udids.contains(udid) {
                udids.push(udid.clone());
            }
        }
        for (device, _) in self.wifi_pairing.devices().unwrap_or_default() {
            if device.address.is_some() && !udids.contains(&device.udid) {
                udids.push(device.udid);
            }
        }
        udids
    }

    pub fn summary(&self, udid: &str) -> Option<dto::Device> {
        lock(&self.summaries).get(udid).cloned()
    }

    pub fn remember(&self, device: &dto::Device) {
        lock(&self.summaries).insert(device.udid.clone(), device.clone());
    }
}

/// Follows usbmuxd attach and detach events forever, reconnecting when
/// usbmuxd restarts. Run it on its own task.
pub async fn watch_devices(app: Events, state: Arc<DeviceState>) {
    loop {
        if let Ok(mut watcher) = orchard_device::watch_devices().await {
            while let Some(Ok(event)) = watcher.next().await {
                let (kind, device) = match event {
                    orchard_device::DeviceEvent::Attached(d) => (DeviceEventKind::Attached, d),
                    orchard_device::DeviceEvent::Detached(d) => (DeviceEventKind::Detached, d),
                };
                let udid = device.udid.0;
                // The other entry (USB or network) may still be there; reconnect lazily.
                state.forget(&udid);
                emit_device(&app, DeviceEvent { kind, udid });
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// The App Store facade (ipatool when installed, native otherwise).
pub struct StoreState {
    store: Arc<AppStore>,
}

impl StoreState {
    pub fn new(data_dir: &Path) -> CmdResult<Self> {
        Ok(Self {
            store: Arc::new(AppStore::new(data_dir)?),
        })
    }

    pub fn store(&self) -> Arc<AppStore> {
        self.store.clone()
    }

    pub fn backend(&self) -> dto::StoreBackend {
        match self.store.backend() {
            orchard_ipa::StoreBackend::Ipatool => dto::StoreBackend::Ipatool,
            orchard_ipa::StoreBackend::Native => dto::StoreBackend::Native,
        }
    }
}

/// Backups opened with `open_backup`, keyed by backup id (its folder path).
#[derive(Default)]
pub struct BackupState {
    open: Mutex<HashMap<String, Arc<Mutex<orchard_backup::Backup>>>>,
}

impl BackupState {
    pub fn insert(&self, id: &str, backup: orchard_backup::Backup) {
        lock(&self.open).insert(id.to_owned(), Arc::new(Mutex::new(backup)));
    }

    pub fn get(&self, id: &str) -> CmdResult<Arc<Mutex<orchard_backup::Backup>>> {
        lock(&self.open)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput(format!("backup {id} is not open")).into())
    }
}

/// Runs blocking work (SQLite, zip, file IO) off the async runtime.
pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    F: FnOnce() -> CmdResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::new("unknown", format!("background work failed: {e}")))?
}

/// Locks a shared value from blocking code.
pub fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock(m)
}

/// Signing identities and provisioning profiles loaded for re-signing, keyed
/// by ids handed to the UI. They never cross to the frontend themselves.
#[derive(Default)]
pub struct SigningState {
    next: std::sync::atomic::AtomicU64,
    identities: Mutex<HashMap<String, orchard_ipa::SigningIdentity>>,
    profiles: Mutex<HashMap<String, orchard_ipa::ProvisioningProfile>>,
}

impl SigningState {
    fn id(&self, prefix: &str) -> String {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{prefix}-{n}")
    }

    pub fn add_identity(&self, identity: orchard_ipa::SigningIdentity) -> String {
        let id = self.id("identity");
        lock(&self.identities).insert(id.clone(), identity);
        id
    }

    pub fn add_profile(&self, profile: orchard_ipa::ProvisioningProfile) -> String {
        let id = self.id("profile");
        lock(&self.profiles).insert(id.clone(), profile);
        id
    }

    pub fn identity(&self, id: &str) -> CmdResult<orchard_ipa::SigningIdentity> {
        lock(&self.identities)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput("load the signing certificate again".into()).into())
    }

    pub fn profile(&self, id: &str) -> CmdResult<orchard_ipa::ProvisioningProfile> {
        lock(&self.profiles)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput("load the provisioning profile again".into()).into())
    }
}
