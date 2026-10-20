//! Services over a Wi-Fi pairing (iOS 17 and later, the only Wi-Fi route on
//! iOS 27): pair-verify with the device's remote pairing service, open the
//! encrypted CoreDevice tunnel, and reach services through the tunnel's RSD
//! listing. Unlike lockdown over Wi-Fi this needs no USB pairing and no
//! Bonjour, so it also works over a VPN such as Tailscale, or through a
//! [`Dialer`] on a client that is on the device's network.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use idevice::remote_pairing::{
    RemotePairingClient, RpPairingFile, RpPairingSocket, connect_tls_psk_tunnel_native,
};
use idevice::rsd::RsdHandshake;
use idevice::tcp::adapter::Adapter;
use idevice::tcp::handle::{AdapterHandle, StreamHandle};
use idevice::{IdeviceError, ReadWrite, RsdService};
use orchard_core::{Error, Result};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::error::device_error;

/// Where the device's remote pairing service usually listens.
pub(crate) const USUAL_PORT: u16 = 49152;
const STEP_TIMEOUT: Duration = Duration::from_secs(8);
const SCAN_TIMEOUT: Duration = Duration::from_millis(1500);
const SCAN_PARALLEL: usize = 400;

/// A connection being opened by a [`Dialer`].
pub type DialFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Box<dyn ReadWrite>>> + Send + 'a>>;

/// Opens TCP connections to one device, by port: directly, or through a
/// client on the device's network that relays them.
pub trait Dialer: Send + Sync + fmt::Debug {
    fn dial(&self, port: u16) -> DialFuture<'_>;
}

/// Connects straight to the device's address.
#[derive(Debug)]
pub(crate) struct TcpDialer(pub(crate) IpAddr);

impl Dialer for TcpDialer {
    fn dial(&self, port: u16) -> DialFuture<'_> {
        Box::pin(async move {
            let stream = TcpStream::connect((self.0, port)).await?;
            Ok(Box::new(stream) as Box<dyn ReadWrite>)
        })
    }
}

/// An open tunnel to one device. Services are opened through it until it is
/// dropped or the device goes away.
pub(crate) struct Tunnel {
    handle: AdapterHandle,
    /// RSD service name to port inside the tunnel.
    services: HashMap<String, u16>,
    /// The device ends the tunnel when this control connection closes.
    _control: Mutex<RemotePairingClient<RpPairingSocket<Box<dyn ReadWrite>>>>,
}

impl fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tunnel")
            .field("services", &self.services.len())
            .finish_non_exhaustive()
    }
}

impl Tunnel {
    /// Opens a tunnel to the remote pairing service through `dialer`, trying
    /// `port` first, then the usual port, then, given the device's `scan`
    /// address, every dynamic port. The scan only runs when the device
    /// answered at all, so an offline device fails fast. Returns the port that
    /// worked, to try first next time.
    pub(crate) async fn open(
        dialer: &dyn Dialer,
        port: Option<u16>,
        record: &RpPairingFile,
        scan: Option<IpAddr>,
    ) -> Result<(Self, u16)> {
        let mut tried = Vec::new();
        let mut answered = false;
        let mut failure = None;
        for port in port.into_iter().chain([USUAL_PORT]) {
            if tried.contains(&port) {
                continue;
            }
            tried.push(port);
            match Self::open_on(dialer, port, record).await {
                Ok(tunnel) => return Ok((tunnel, port)),
                Err(Attempt::Unreachable) => {}
                Err(Attempt::Refused) => answered = true,
                Err(Attempt::Failed(e)) => {
                    answered = true;
                    failure = Some(e);
                }
            }
        }
        let Some(addr) = scan.filter(|_| answered) else {
            return Err(failure.unwrap_or_else(|| {
                Error::NotFound(
                    "the device did not answer. Check that it is awake and on the network".into(),
                )
            }));
        };
        for port in scan_ports(addr).await {
            if tried.contains(&port) {
                continue;
            }
            match Self::open_on(dialer, port, record).await {
                Ok(tunnel) => return Ok((tunnel, port)),
                Err(Attempt::Failed(e)) => failure = Some(e),
                Err(_) => {}
            }
        }
        // A pair-verify failure usually means the device forgot this pairing.
        Err(failure.unwrap_or_else(|| {
            Error::NotFound(format!(
                "the device at {addr} offers no Wi-Fi pairing service. Check that Developer Mode is on"
            ))
        }))
    }

    async fn open_on(
        dialer: &dyn Dialer,
        port: u16,
        record: &RpPairingFile,
    ) -> std::result::Result<Self, Attempt> {
        let socket = match timeout(STEP_TIMEOUT, dialer.dial(port)).await {
            Ok(Ok(socket)) => socket,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                return Err(Attempt::Refused);
            }
            _ => return Err(Attempt::Unreachable),
        };
        Self::start(dialer, socket, record)
            .await
            .map_err(Attempt::Failed)
    }

    async fn start(
        dialer: &dyn Dialer,
        socket: Box<dyn ReadWrite>,
        record: &RpPairingFile,
    ) -> Result<Self> {
        let step = |what: &'static str| move |_| Error::Device(format!("{what} timed out"));
        let mut control = RemotePairingClient::new(RpPairingSocket::new(socket), "orchard");
        // validate_pairing takes the record mutably; keep the caller's as is.
        let mut record = record.clone();
        timeout(STEP_TIMEOUT, control.attempt_pair_verify())
            .await
            .map_err(step("pair-verify"))?
            .map_err(|e| device_error("pair-verify", e))?;
        timeout(STEP_TIMEOUT, control.validate_pairing(&mut record))
            .await
            .map_err(step("pair-verify"))?
            .map_err(|e| device_error("pair-verify", e))?;
        let tunnel_port = timeout(STEP_TIMEOUT, control.create_tcp_listener())
            .await
            .map_err(step("tunnel"))?
            .map_err(|e| device_error("tunnel", e))?;
        let stream = timeout(STEP_TIMEOUT, dialer.dial(tunnel_port))
            .await
            .map_err(step("tunnel"))??;
        let tunnel = timeout(
            STEP_TIMEOUT,
            connect_tls_psk_tunnel_native(stream, control.encryption_key()),
        )
        .await
        .map_err(step("tunnel"))?
        .map_err(|e| device_error("tunnel", e))?;

        let bad_address = |_| Error::Protocol("tunnel address".into());
        let client_ip = tunnel.info.client_address.parse().map_err(bad_address)?;
        let server_ip = tunnel.info.server_address.parse().map_err(bad_address)?;
        let rsd_port = tunnel.info.server_rsd_port;
        let adapter = Adapter::new(Box::new(tunnel.into_inner()), client_ip, server_ip);
        let mut handle = adapter.to_async_handle();
        let rsd = timeout(STEP_TIMEOUT, handle.connect(rsd_port))
            .await
            .map_err(step("service list"))??;
        let handshake = timeout(STEP_TIMEOUT, RsdHandshake::new(rsd))
            .await
            .map_err(step("service list"))?
            .map_err(|e| device_error("service list", e))?;
        let services = handshake
            .services
            .into_iter()
            .map(|(name, service)| (name, service.port))
            .collect();
        Ok(Self {
            handle,
            services,
            _control: Mutex::new(control),
        })
    }

    /// Starts service `S` through the tunnel.
    pub(crate) async fn service<S: RsdService>(&self) -> Result<S> {
        let name = S::rsd_service_name();
        let stream: Box<dyn ReadWrite> = Box::new(self.connect(&name).await?);
        S::from_stream(stream)
            .await
            .map_err(|e| device_error(&name, e))
    }

    /// Opens a connection to the service named `name` in the RSD listing.
    pub(crate) async fn connect(&self, name: &str) -> Result<StreamHandle> {
        let port = *self
            .services
            .get(name)
            .ok_or_else(|| Error::Device(format!("the device does not offer {name}")))?;
        self.handle
            .clone()
            .connect(port)
            .await
            .map_err(|e| device_error(name, IdeviceError::Socket(e)))
    }
}

/// Why one port did not give a tunnel.
enum Attempt {
    /// Nothing answered: the device is offline or the address is wrong.
    Unreachable,
    /// The device is there, but nothing listens on the port.
    Refused,
    /// The device answered, but not with a working pairing.
    Failed(Error),
}

/// Open TCP ports in the dynamic range, where iOS puts its remote pairing
/// service when not on the usual port.
async fn scan_ports(addr: IpAddr) -> Vec<u16> {
    let mut probes = JoinSet::new();
    let mut open = Vec::new();
    for port in 49152..=u16::MAX {
        probes.spawn(async move {
            match timeout(SCAN_TIMEOUT, TcpStream::connect((addr, port))).await {
                Ok(Ok(_)) => Some(port),
                _ => None,
            }
        });
        if probes.len() >= SCAN_PARALLEL
            && let Some(Ok(Some(port))) = probes.join_next().await
        {
            open.push(port);
        }
    }
    while let Some(result) = probes.join_next().await {
        if let Ok(Some(port)) = result {
            open.push(port);
        }
    }
    open.sort_unstable();
    open
}
