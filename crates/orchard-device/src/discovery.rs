use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use orchard_core::{Error, Result, Udid};
use serde::{Deserialize, Serialize};

use crate::pairing::{PairingStore, StoredPairing};

const SERVICE: &str = "_apple-mobdev2._tcp.local.";

/// A paired device announcing itself over Bonjour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiDevice {
    pub udid: Udid,
    pub addresses: Vec<IpAddr>,
    /// Bonjour instance name.
    pub service_name: String,
}

impl WifiDevice {
    /// IPv4 first, then a routable IPv6 (link-local needs a scope id).
    pub fn preferred_address(&self) -> Option<IpAddr> {
        self.addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| {
                self.addresses.iter().find(|a| match a {
                    IpAddr::V6(v6) => !v6.is_unicast_link_local(),
                    IpAddr::V4(_) => false,
                })
            })
            .copied()
    }
}

/// Browses `_apple-mobdev2._tcp` for `timeout` and returns the devices that
/// match a record in `store`.
///
/// Devices advertise only when Wi-Fi connections are enabled and they are on
/// the same network.
pub async fn discover_wifi_devices(
    store: &PairingStore,
    timeout: Duration,
) -> Result<Vec<WifiDevice>> {
    browse(store.list()?, timeout, None).await
}

/// Like [`discover_wifi_devices`] but returns as soon as `udid` is seen.
pub(crate) async fn find_wifi_device(
    store: &PairingStore,
    udid: &Udid,
    timeout: Duration,
) -> Result<Option<WifiDevice>> {
    let pairings: Vec<_> = store
        .list()?
        .into_iter()
        .filter(|p| &p.udid == udid)
        .collect();
    Ok(browse(pairings, timeout, Some(udid)).await?.pop())
}

async fn browse(
    pairings: Vec<StoredPairing>,
    timeout: Duration,
    target: Option<&Udid>,
) -> Result<Vec<WifiDevice>> {
    if pairings.is_empty() {
        return Ok(Vec::new());
    }
    let mdns_error = |e: mdns_sd::Error| Error::Network(format!("mDNS: {e}"));
    let daemon = ServiceDaemon::new().map_err(mdns_error)?;
    let events = daemon.browse(SERVICE).map_err(mdns_error)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut found: Vec<WifiDevice> = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv_async()).await {
        let ServiceEvent::ServiceResolved(service) = event else {
            continue;
        };
        let txt: Vec<(&str, &[u8])> = service
            .txt_properties
            .iter()
            .map(|p| (p.key(), p.val().unwrap_or_default()))
            .collect();
        let Some(udid) = match_service(&service.fullname, &txt, &pairings) else {
            continue;
        };
        let addresses = service.addresses.iter().map(|a| a.to_ip_addr());
        let done = target == Some(&udid);
        match found.iter_mut().find(|d| d.udid == udid) {
            Some(existing) => {
                for addr in addresses {
                    if !existing.addresses.contains(&addr) {
                        existing.addresses.push(addr);
                    }
                }
            }
            None => found.push(WifiDevice {
                udid,
                addresses: addresses.collect(),
                service_name: instance_name(&service.fullname).to_string(),
            }),
        }
        if done {
            break;
        }
    }
    let _ = daemon.shutdown();
    Ok(found)
}

fn instance_name(fullname: &str) -> &str {
    fullname
        .strip_suffix(SERVICE)
        .map_or(fullname, |s| s.trim_end_matches('.'))
}

/// Matches a Bonjour announcement to a stored pairing.
///
/// iOS 26.4 and later publish `identifier` and `authTag` TXT values derived
/// from the pairing HostID. Earlier versions name the instance
/// `<wifi mac>@<address>`.
fn match_service(
    fullname: &str,
    txt: &[(&str, &[u8])],
    pairings: &[StoredPairing],
) -> Option<Udid> {
    let identifier = txt
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("identifier"))
        .map(|(_, v)| *v);
    let tags: Vec<&[u8]> = txt
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("authTag"))
        .map(|(_, v)| *v)
        .collect();
    if let Some(identifier) = identifier
        && !tags.is_empty()
        && let Some(p) = pairings
            .iter()
            .find(|p| idevice::mdns::txt_record_matches(p.host_id.as_bytes(), identifier, &tags))
    {
        return Some(p.udid.clone());
    }

    let mac = instance_name(fullname).split('@').next()?;
    pairings
        .iter()
        .find(|p| !p.wifi_mac_address.is_empty() && p.wifi_mac_address.eq_ignore_ascii_case(mac))
        .map(|p| p.udid.clone())
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;

    fn pairing(udid: &str, host_id: &str, mac: &str) -> StoredPairing {
        StoredPairing {
            udid: Udid(udid.into()),
            host_id: host_id.into(),
            wifi_mac_address: mac.into(),
        }
    }

    #[test]
    fn matches_auth_tag_from_host_id() {
        let pairings = [
            pairing(
                "A",
                "11111111-2222-3333-4444-555555555555",
                "aa:aa:aa:aa:aa:aa",
            ),
            pairing(
                "B",
                "66666666-7777-8888-9999-000000000000",
                "bb:bb:bb:bb:bb:bb",
            ),
        ];
        let identifier = b"7F3C2A10-1D2E-4F50-8A6B-9C0D1E2F3A4B";
        let tag = idevice::mdns::derive_auth_tag(pairings[1].host_id.as_bytes(), identifier);
        let tag = base64::engine::general_purpose::STANDARD.encode(tag);
        let txt: Vec<(&str, &[u8])> = vec![
            ("identifier", identifier),
            ("authTag", b"AAAAAAAAAAA="),
            ("authTag", tag.as_bytes()),
        ];
        let name = "7F3C2A10-1D2E-4F50-8A6B-9C0D1E2F3A4B._apple-mobdev2._tcp.local.";
        assert_eq!(match_service(name, &txt, &pairings), Some(Udid("B".into())));
        assert_eq!(match_service(name, &txt[..2], &pairings), None);
    }

    #[test]
    fn matches_legacy_mac_instance_name() {
        let pairings = [pairing("A", "h", "a4:83:e7:12:34:56")];
        let name = "A4:83:E7:12:34:56@fe80::a683:e7ff:fe12:3456._apple-mobdev2._tcp.local.";
        assert_eq!(match_service(name, &[], &pairings), Some(Udid("A".into())));
        let other = "00:11:22:33:44:55@fe80::1._apple-mobdev2._tcp.local.";
        assert_eq!(match_service(other, &[], &pairings), None);
    }

    #[test]
    fn prefers_ipv4_then_routable_ipv6() {
        let mut dev = WifiDevice {
            udid: Udid("A".into()),
            addresses: vec!["fe80::1".parse().unwrap(), "2001:db8::5".parse().unwrap()],
            service_name: String::new(),
        };
        assert_eq!(
            dev.preferred_address(),
            Some("2001:db8::5".parse().unwrap())
        );
        dev.addresses.push("192.168.1.9".parse().unwrap());
        assert_eq!(
            dev.preferred_address(),
            Some("192.168.1.9".parse().unwrap())
        );
        dev.addresses = vec!["fe80::1".parse().unwrap()];
        assert_eq!(dev.preferred_address(), None);
    }

    #[tokio::test]
    async fn empty_store_does_not_browse() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        let found = discover_wifi_devices(&store, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(found.is_empty());
    }
}
