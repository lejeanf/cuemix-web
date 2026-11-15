//! MOTU IP discovery.
//!
//! Order (HANDOFF §2.2): explicit `[motu].ip` from config -> mDNS/Bonjour
//! browse via `mdns-sd` -> ARP neighbour table (`/proc/net/arp`, Linux) + TCP
//! probe on the MOTU WebSocket port. [`link_local_interfaces`] lists the local
//! `169.254.0.0/16` interfaces (candidate MOTU links).
//!
//! PROVISIONAL: the MOTU's mDNS service type is unknown. We browse
//! `_http._tcp.local.` and keep resolved services whose instance name contains
//! "MOTU"/"UltraLite" (case-insensitive) or that resolve to a `169.254.0.0/16`
//! address. Revisit once the real advertisement is captured.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

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

/// Parse the text of `/proc/net/arp` into neighbour IPv4 addresses.
///
/// Incomplete entries (flags `0x0`) and malformed lines are skipped. When
/// `iface` is `Some` (and non-empty) only entries of that device are kept.
pub fn parse_proc_net_arp(text: &str, iface: Option<&str>) -> Vec<Ipv4Addr> {
    text.lines()
        .skip(1) // header
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let ip: Ipv4Addr = cols.next()?.parse().ok()?;
            let _hw_type = cols.next()?;
            let flags = cols.next()?;
            let _mac = cols.next()?;
            let _mask = cols.next()?;
            let dev = cols.next()?;
            let complete = u32::from_str_radix(flags.trim_start_matches("0x"), 16)
                .map(|f| f != 0)
                .unwrap_or(false);
            let iface_ok = iface.map_or(true, |i| i.is_empty() || i == dev);
            (complete && iface_ok).then_some(ip)
        })
        .collect()
}

/// `true` when a TCP connection to `ip:port` succeeds within `limit`.
pub async fn probe(ip: IpAddr, port: u16, limit: Duration) -> bool {
    matches!(
        timeout(limit, TcpStream::connect(SocketAddr::new(ip, port))).await,
        Ok(Ok(_))
    )
}

/// Does a resolved mDNS service look like a MOTU? (PROVISIONAL heuristic.)
fn looks_like_motu(fullname: &str, addrs: &[IpAddr]) -> Option<Ipv4Addr> {
    let lower = fullname.to_ascii_lowercase();
    let name_match = lower.contains("motu") || lower.contains("ultralite");
    addrs.iter().find_map(|a| match a {
        IpAddr::V4(v4) if name_match || is_link_local(*v4) => Some(*v4),
        _ => None,
    })
}

/// Blocking mDNS browse, bounded by `limit`.
fn mdns_browse_blocking(limit: Duration) -> Option<Ipv4Addr> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(error = %e, "mdns daemon unavailable");
            return None;
        }
    };
    let found = (|| {
        let rx = daemon.browse("_http._tcp.local.").ok()?;
        let deadline = std::time::Instant::now() + limit;
        loop {
            let left = deadline.checked_duration_since(std::time::Instant::now())?;
            match rx.recv_timeout(left) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    if let Some(ip) = looks_like_motu(info.get_fullname(), &addrs) {
                        return Some(ip);
                    }
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    })();
    let _ = daemon.shutdown();
    found
}

/// mDNS browse (up to `limit`), run off the async runtime.
pub async fn mdns_discover(limit: Duration) -> Option<Ipv4Addr> {
    tokio::task::spawn_blocking(move || mdns_browse_blocking(limit))
        .await
        .ok()
        .flatten()
}

/// Link-local ARP neighbours (Linux only; empty elsewhere).
async fn arp_candidates(iface: Option<&str>) -> Vec<Ipv4Addr> {
    #[cfg(target_os = "linux")]
    {
        match tokio::fs::read_to_string("/proc/net/arp").await {
            Ok(text) => parse_proc_net_arp(&text, iface)
                .into_iter()
                .filter(|ip| is_link_local(*ip))
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read /proc/net/arp");
                Vec::new()
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
        Vec::new()
    }
}

/// Find the MOTU: explicit IP, then mDNS, then ARP neighbours + TCP probe.
///
/// An explicit IP is returned as-is (the connect attempt validates it).
pub async fn discover(explicit: Option<IpAddr>, port: u16, iface: Option<&str>) -> Option<IpAddr> {
    if let Some(ip) = explicit {
        return Some(ip);
    }
    if let Some(ip) = mdns_discover(Duration::from_millis(1500)).await {
        tracing::info!(%ip, "MOTU found via mDNS");
        return Some(IpAddr::V4(ip));
    }
    for ip in arp_candidates(iface).await {
        if probe(IpAddr::V4(ip), port, Duration::from_millis(500)).await {
            tracing::info!(%ip, "MOTU found via ARP + probe");
            return Some(IpAddr::V4(ip));
        }
    }
    tracing::debug!("MOTU discovery found nothing");
    None
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

    const ARP: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
169.254.10.2     0x1         0x2         02:00:00:00:00:01     *        usb0
192.0.2.7        0x1         0x2         02:00:00:00:00:02     *        eth0
169.254.99.9     0x1         0x0         00:00:00:00:00:00     *        usb0
garbage line
169.254.20.3     0x1         0x6         02:00:00:00:00:03     *        eth0
";

    #[test]
    fn arp_all_complete_entries() {
        assert_eq!(
            parse_proc_net_arp(ARP, None),
            vec![
                Ipv4Addr::new(169, 254, 10, 2),
                Ipv4Addr::new(192, 0, 2, 7),
                Ipv4Addr::new(169, 254, 20, 3)
            ]
        );
    }

    #[test]
    fn arp_interface_filter() {
        assert_eq!(
            parse_proc_net_arp(ARP, Some("usb0")),
            vec![Ipv4Addr::new(169, 254, 10, 2)]
        );
        assert_eq!(parse_proc_net_arp(ARP, Some("")).len(), 3);
        assert!(parse_proc_net_arp(ARP, Some("nope")).is_empty());
        assert!(parse_proc_net_arp("", None).is_empty());
    }

    #[test]
    fn mdns_heuristic() {
        let ll = [IpAddr::V4(Ipv4Addr::new(169, 254, 1, 2))];
        let other = [IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))];
        assert!(looks_like_motu("x._http._tcp.local.", &ll).is_some());
        assert!(looks_like_motu("UltraLite-mk5._http._tcp.local.", &other).is_some());
        assert!(looks_like_motu("printer._http._tcp.local.", &other).is_none());
    }

    #[tokio::test]
    async fn probe_and_explicit_discovery() {
        let Ok(l) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
        let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(probe(lo, port, Duration::from_secs(1)).await);
        drop(l);
        assert!(!probe(lo, port, Duration::from_millis(300)).await);
        assert_eq!(discover(Some(lo), port, None).await, Some(lo));
    }
}
