use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use idevice::lockdown::LockdownClient;
use idevice::pairing_file::PairingFile;
use idevice::provider::IdeviceProvider;
use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdDevice};
use idevice::{Idevice, IdeviceError, IdeviceService, RsdService};
use orchard_core::{Error, Result, Udid};
use tokio::net::TcpStream;

use crate::Connection;
use crate::discovery::find_wifi_device;
use crate::error::device_error;
use crate::mux::{self, mux_addr, mux_connect};
use crate::pairing::PairingStore;
use crate::tunnel::{Dialer, TcpDialer, Tunnel};
use crate::wifi_pairing::WifiPairingStore;

const LABEL: &str = "orchard";
/// How long [`DeviceConnection::connect`] browses mDNS before giving up.
const WIFI_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

/// An open route to one device, used to start services on it.
///
/// Opening it is cheap: no service is started until a method needs one.
#[derive(Debug)]
pub struct DeviceConnection {
    udid: Udid,
    connection: Connection,
    pub(crate) provider: OrchardProvider,
}

#[derive(Debug, Clone)]
pub(crate) enum Route {
    Mux {
        addr: UsbmuxdAddr,
        device_id: u32,
    },
    Tcp {
        addr: IpAddr,
    },
    /// Through a tunnel opened with a Wi-Fi pairing; services come from its
    /// RSD listing instead of lockdown.
    Tunnel(Arc<Tunnel>),
}

/// Provider with a pre-resolved pairing record, for both usbmuxd and direct TCP.
#[derive(Debug)]
pub(crate) struct OrchardProvider {
    pub(crate) route: Route,
    pub(crate) pairing: Option<PairingFile>,
}

impl IdeviceProvider for OrchardProvider {
    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Idevice, IdeviceError>> + Send>> {
        let route = self.route.clone();
        Box::pin(async move {
            match route {
                Route::Mux { addr, device_id } => {
                    let mux = addr.connect(device_id).await?;
                    mux.connect_to_device(device_id, port, LABEL).await
                }
                Route::Tcp { addr } => {
                    let stream = TcpStream::connect((addr, port)).await?;
                    Ok(Idevice::new(Box::new(stream), LABEL))
                }
                // Tunnel services are opened by name, see DeviceConnection::service.
                Route::Tunnel(_) => Err(IdeviceError::ServiceNotFound),
            }
        })
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<PairingFile, IdeviceError>> + Send>> {
        let pairing = self.pairing.clone();
        Box::pin(async move { pairing.ok_or(IdeviceError::InvalidHostID) })
    }
}

impl DeviceConnection {
    /// Finds `udid` and prepares a connection to it.
    ///
    /// Order: usbmuxd USB, usbmuxd network, then direct Wi-Fi to an address
    /// found over mDNS using the record in `store`. The pairing record comes
    /// from usbmuxd when it has one (and is copied into `store` so Wi-Fi works
    /// later), otherwise from `store`. An unpaired USB device still connects so
    /// that [`DeviceConnection::pair`] can be called.
    pub async fn connect(udid: &Udid, store: &PairingStore) -> Result<Self> {
        let stored = load_stored(store, udid);
        if let Ok(addr) = mux_addr()
            && let Ok(mut mux) = mux_connect(&addr).await
            && let Ok(devices) = mux::list_on(&mut mux).await
            && let Some(dev) = pick_mux_entry(&devices, udid)
        {
            let record = mux::read_pair_record(&addr, udid).await.ok().flatten();
            if let Some(record) = &record
                && needs_refresh(stored.as_ref(), record)
            {
                let _ = store.save(udid, record);
            }
            let route = Route::Mux {
                addr,
                device_id: dev.device_id,
            };
            return Ok(Self::new(
                udid,
                mux::to_device(dev).connection,
                route,
                record.or(stored),
            ));
        }

        let Some(pairing) = stored else {
            return Err(Error::NotFound(format!(
                "device {udid} is not connected and has no Wi-Fi pairing record"
            )));
        };
        let found = find_wifi_device(store, udid, WIFI_DISCOVERY_TIMEOUT).await?;
        match found.and_then(|d| d.preferred_address()) {
            Some(addr) => Ok(Self::with_pairing(udid, addr, pairing)),
            None => Err(Error::NotFound(format!(
                "device {udid} is not connected and was not found on the network"
            ))),
        }
    }

    /// Prepares a direct Wi-Fi connection to `addr` with the record stored for `udid`.
    ///
    /// Works without usbmuxd network support (Linux). The device must have Wi-Fi
    /// connections enabled, see [`DeviceConnection::set_wifi_connections`].
    pub fn connect_wifi(udid: &Udid, addr: IpAddr, store: &PairingStore) -> Result<Self> {
        let pairing = store
            .load(udid)?
            .ok_or_else(|| Error::NotFound(format!("pairing record for {udid}")))?;
        Ok(Self::with_pairing(udid, addr, pairing))
    }

    /// Opens a tunnel to `udid` at `addr` with its Wi-Fi pairing record in
    /// `store`, and remembers the address so the device is listed there.
    pub async fn connect_tunnel(
        udid: &Udid,
        addr: IpAddr,
        store: &WifiPairingStore,
    ) -> Result<Self> {
        let (device, record) = store
            .find(&udid.0)?
            .ok_or_else(|| Error::NotFound(format!("Wi-Fi pairing for {udid}")))?;
        let known = (device.address == Some(addr))
            .then_some(device.port)
            .flatten();
        // Boxed: the handshake futures are large enough to overflow a worker
        // thread's stack in debug builds.
        let (tunnel, port) =
            Box::pin(Tunnel::open(&TcpDialer(addr), known, &record, Some(addr))).await?;
        if known != Some(port) {
            store.set_route(&device, addr, port)?;
        }
        Ok(Self::new(
            udid,
            Connection::Network(addr),
            Route::Tunnel(Arc::new(tunnel)),
            None,
        ))
    }

    /// Opens a tunnel through `dialer`, a client on the device's network, to
    /// the remote pairing service on `port`. Tries the Wi-Fi pairing of
    /// `udid`, or when it is not known yet each Wi-Fi pairing in `store`.
    pub async fn connect_relayed(
        dialer: &dyn Dialer,
        port: u16,
        store: &WifiPairingStore,
        udid: Option<&Udid>,
    ) -> Result<Self> {
        let mut failure = None;
        for (device, record) in store.devices()? {
            if udid.is_some_and(|u| u.0 != device.udid) {
                continue;
            }
            match Box::pin(Tunnel::open(dialer, Some(port), &record, None)).await {
                Ok((tunnel, _)) => {
                    return Ok(Self::new(
                        &Udid(device.udid),
                        Connection::Other("relay".into()),
                        Route::Tunnel(Arc::new(tunnel)),
                        None,
                    ));
                }
                Err(e) => failure = Some(e),
            }
        }
        Err(failure.unwrap_or_else(|| Error::NotFound("no Wi-Fi pairing for this device".into())))
    }

    /// Prepares a direct Wi-Fi connection with an explicit pairing record.
    pub fn with_pairing(udid: &Udid, addr: IpAddr, pairing: PairingFile) -> Self {
        Self::new(
            udid,
            Connection::Network(addr),
            Route::Tcp { addr },
            Some(pairing),
        )
    }

    fn new(
        udid: &Udid,
        connection: Connection,
        route: Route,
        pairing: Option<PairingFile>,
    ) -> Self {
        Self {
            udid: udid.clone(),
            connection,
            provider: OrchardProvider { route, pairing },
        }
    }

    pub fn udid(&self) -> &Udid {
        &self.udid
    }

    /// How this connection reaches the device.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The pairing record in use, if any.
    pub fn pairing_record(&self) -> Option<&PairingFile> {
        self.provider.pairing.as_ref()
    }

    /// Whether this connection runs through a Wi-Fi pairing tunnel.
    pub fn is_tunnel(&self) -> bool {
        matches!(self.provider.route, Route::Tunnel(_))
    }

    pub(crate) fn require_pairing(&self) -> Result<&PairingFile> {
        self.provider
            .pairing
            .as_ref()
            .ok_or_else(|| Error::Device(format!("device {} is not paired", self.udid)))
    }

    /// Lockdown without a session, enough for pairing requests. Through a
    /// tunnel this is the trusted lockdown, which needs no session.
    pub(crate) async fn lockdown_plain(&self) -> Result<LockdownClient> {
        if let Route::Tunnel(tunnel) = &self.provider.route {
            return tunnel.service().await;
        }
        LockdownClient::connect(&self.provider)
            .await
            .map_err(|e| device_error("lockdown", e))
    }

    /// Lockdown with an authenticated session.
    pub async fn lockdown(&self) -> Result<LockdownClient> {
        if self.is_tunnel() {
            return self.lockdown_plain().await;
        }
        let pairing = self.require_pairing()?;
        let mut lockdown = self.lockdown_plain().await?;
        lockdown
            .start_session(pairing)
            .await
            .map_err(|e| device_error("lockdown session", e))?;
        Ok(lockdown)
    }

    /// Starts a service, through lockdown or the tunnel's service listing.
    pub async fn service<S: IdeviceService + RsdService>(&self) -> Result<S> {
        if let Route::Tunnel(tunnel) = &self.provider.route {
            return tunnel.service().await;
        }
        self.require_pairing()?;
        S::connect(&self.provider)
            .await
            .map_err(|e| device_error(&S::service_name(), e))
    }
}

/// The stored record, treating an unreadable one as missing so that a valid
/// usbmuxd record can still be used and saved over it.
fn load_stored(store: &PairingStore, udid: &Udid) -> Option<PairingFile> {
    store.load(udid).ok().flatten()
}

/// Whether the usbmuxd record differs from the stored one, for example after
/// re-pairing outside Orchard.
fn needs_refresh(stored: Option<&PairingFile>, mux: &PairingFile) -> bool {
    stored.is_none_or(|s| {
        s.host_id != mux.host_id
            || s.host_certificate != mux.host_certificate
            || s.device_certificate != mux.device_certificate
    })
}

/// Picks the usbmuxd entry for `udid`, USB first.
fn pick_mux_entry<'a>(devices: &'a [UsbmuxdDevice], udid: &Udid) -> Option<&'a UsbmuxdDevice> {
    devices
        .iter()
        .filter(|d| d.udid == udid.0)
        .min_by_key(|d| !matches!(d.connection_type, idevice::usbmuxd::Connection::Usb))
}

#[cfg(test)]
mod tests {
    use idevice::usbmuxd::Connection as MuxConnection;

    use super::*;

    fn dev(id: u32, udid: &str, connection_type: MuxConnection) -> UsbmuxdDevice {
        UsbmuxdDevice {
            connection_type,
            udid: udid.into(),
            device_id: id,
        }
    }

    #[test]
    fn prefers_usb_over_network() {
        let devices = vec![
            dev(1, "A", MuxConnection::Network("10.0.0.2".parse().unwrap())),
            dev(2, "A", MuxConnection::Usb),
            dev(3, "B", MuxConnection::Usb),
        ];
        let udid = Udid("A".into());
        assert_eq!(pick_mux_entry(&devices, &udid).unwrap().device_id, 2);
        assert_eq!(pick_mux_entry(&devices[..1], &udid).unwrap().device_id, 1);
        assert!(pick_mux_entry(&devices, &Udid("C".into())).is_none());
    }

    #[test]
    fn corrupt_stored_record_counts_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let udid = Udid("ABC".into());
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.dir().join("ABC.plist"), b"garbage").unwrap();
        assert!(store.load(&udid).is_err());
        assert!(load_stored(&store, &udid).is_none());
    }

    #[test]
    fn refreshes_only_changed_records() {
        let record = |host: &str| {
            PairingFile::from_bytes(&crate::pairing::tests::fixture(host, "m")).unwrap()
        };
        assert!(needs_refresh(None, &record("H1")));
        assert!(!needs_refresh(Some(&record("H1")), &record("H1")));
        assert!(needs_refresh(Some(&record("H1")), &record("H2")));
    }

    fn assert_send<T: Send>(_: T) {}

    #[allow(dead_code)]
    fn public_futures_are_send(conn: &DeviceConnection, store: &PairingStore, udid: &Udid) {
        assert_send(DeviceConnection::connect(udid, store));
        assert_send(conn.info());
        assert_send(conn.is_paired());
        assert_send(conn.list_apps(false));
        assert_send(conn.app_icon("com.example"));
        assert_send(conn.install_ipa(std::path::Path::new("a.ipa"), |_| {}));
        assert_send(conn.uninstall_app("com.example"));
        assert_send(conn.files(crate::FileLocation::Media));
        assert_send(conn.screenshot());
        assert_send(conn.syslog());
        assert_send(conn.restart());
        assert_send(crate::watch_devices());
        assert_send(crate::discover_wifi_devices(store, Duration::from_secs(1)));
    }

    #[allow(dead_code)]
    fn mutating_futures_are_send(conn: &mut DeviceConnection, store: &PairingStore) {
        assert_send(conn.pair(store, "host", Duration::from_secs(1)));
        assert_send(conn.unpair(store));
    }

    #[allow(dead_code)]
    async fn file_futures_are_send(files: &mut crate::DeviceFiles) {
        assert_send(files.list_dir("/"));
        assert_send(files.download("/a", std::path::Path::new("a"), |_| {}));
    }
}
