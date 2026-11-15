//! MOTU IP discovery.
//!
//! Order (HANDOFF §2.2): explicit `[motu].ip` from config → link-local
//! (`169.254.0.0/16`) interface detection via `if-addrs` (Phase 1, here) →
//! mDNS/Bonjour browse via `mdns-sd` → ARP neighbour table + TCP probe on
//! port [`super::MOTU_WS_PORT`] (both Phase 2).

use std::net::{IpAddr, Ipv4Addr};

/// A local interface holding an IPv4 link-local address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkLocalIface {
    pub name: String,
    pub addr: Ipv4Addr,
}

/// `true` when `ip` is in the IPv4 link-local range `169.254.0.0/16`.
pub fn is_link_local(ip: Ipv4Addr) -> bool {
    ip.is_link_local()
}

/// Local interfaces with a `169.254.0.0/16` address (candidate MOTU links),
/// optionally restricted to `preferred` (config `network.motu_interface`).
pub fn link_local_interfaces(preferred: Option<&str>) -> std::io::Result<Vec<LinkLocalIface>> {
    let found = if_addrs::get_if_addrs()?
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(v4) if is_link_local(v4) => Some(LinkLocalIface {
                name: iface.name,
                addr: v4,
            }),
            _ => None,
        })
        .filter(|i| preferred.map_or(true, |p| p.is_empty() || i.name == p))
        .collect();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_local_range() {
        assert!(is_link_local(Ipv4Addr::new(169, 254, 254, 1)));
        assert!(!is_link_local(Ipv4Addr::new(192, 0, 2, 10)));
        assert!(!is_link_local(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn enumeration_does_not_fail() {
        // Host-dependent result; only the call itself must succeed.
        assert!(link_local_interfaces(None).is_ok());
        assert!(link_local_interfaces(Some("definitely-not-an-iface")).is_ok_and(|v| v.is_empty()));
    }
}
