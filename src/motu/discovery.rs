//! MOTU IP discovery.
//!
//! [`discover`] returns an **ordered list of candidates** the client tries in
//! turn: explicit `[motu].ip` from config (when set it is the only candidate:
//! a pinned address is never second-guessed) -> UDP announcement on port 1280
//! -> mDNS/Bonjour browse via `mdns-sd` -> ARP neighbour table
//! (`/proc/net/arp`, Linux) + TCP probe. Every address of this host is excluded
//! from the discovered candidates (we publish our own `_http._tcp` service on
//! every interface, including the MOTU link). [`link_local_interfaces`] lists
//! the local `169.254.0.0/16` interfaces.
//!
//! PROVISIONAL (announcement): `docs/MOTU-PROTOCOL.md` documents a JSON
//! announcement (`{"uid","name","ip","model","version","interval"}`) on UDP
//! port 1280 but not the multicast group. We bind `0.0.0.0:1280` for at most
//! [`ANNOUNCE_LISTEN`] and accept datagrams from `169.254.0.0/16` sources whose
//! JSON mentions "UltraLite"/"MOTU"; the *source* IP is used. No group is
//! joined and `SO_REUSEADDR` is not set (no `socket2` dependency): if the port
//! is taken the step is skipped.
//!
//! PROVISIONAL (mDNS): the MOTU's service type is unknown. We browse
//! `_http._tcp.local.` and keep resolved services whose instance name contains
//! "MOTU"/"UltraLite" (case-insensitive) and that are not local addresses.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::{TcpStream, UdpSocket};
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
        .filter(|i| preferred.is_none_or(|p| p.is_empty() || i.name == p))
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
            let iface_ok = iface.is_none_or(|i| i.is_empty() || i == dev);
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

/// UDP port of the MOTU announcement (PROVISIONAL, see module docs).
pub const ANNOUNCE_PORT: u16 = 1280;
/// How long the announcement listener waits.
pub const ANNOUNCE_LISTEN: Duration = Duration::from_millis(1500);

/// Every address of this host (loopback included).
pub fn local_addrs() -> HashSet<IpAddr> {
    if_addrs::get_if_addrs()
        .map(|v| v.into_iter().map(|i| i.ip()).collect())
        .unwrap_or_default()
}

/// Does a resolved mDNS service look like a MOTU? The *name* must match
/// "motu"/"ultralite" (case-insensitive); addresses belonging to this host
/// (`local`) never qualify, whatever the name (our own announcement).
fn looks_like_motu(fullname: &str, addrs: &[IpAddr], local: &HashSet<IpAddr>) -> Option<Ipv4Addr> {
    let lower = fullname.to_ascii_lowercase();
    if !(lower.contains("motu") || lower.contains("ultralite")) {
        return None;
    }
    addrs.iter().find_map(|a| match a {
        IpAddr::V4(v4) if !local.contains(a) => Some(*v4),
        _ => None,
    })
}

/// Parse one announcement datagram (PROVISIONAL): a JSON object mentioning
/// "UltraLite"/"MOTU" sent from a `169.254.0.0/16` source. Returns the source IP.
pub fn parse_announcement(datagram: &[u8], src: Ipv4Addr) -> Option<Ipv4Addr> {
    if !is_link_local(src) {
        return None;
    }
    let text = std::str::from_utf8(datagram).ok()?;
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    if !json.is_object() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    (lower.contains("ultralite") || lower.contains("motu")).then_some(src)
}

/// Listen for the MOTU announcement for at most `limit` (PROVISIONAL).
/// Bind failures are logged at debug and skipped.
pub async fn announcement_discover(
    port: u16,
    limit: Duration,
    local: &HashSet<IpAddr>,
) -> Option<Ipv4Addr> {
    let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(port, error = %e, "announcement listener unavailable, skipped");
            return None;
        }
    };
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, from) = match tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "announcement receive failed");
                return None;
            }
            Err(_) => return None,
        };
        let IpAddr::V4(src) = from.ip() else { continue };
        if local.contains(&from.ip()) {
            continue;
        }
        if let Some(data) = buf.get(..n) {
            if let Some(ip) = parse_announcement(data, src) {
                return Some(ip);
            }
        }
    }
}

/// Blocking mDNS browse, bounded by `limit`.
fn mdns_browse_blocking(limit: Duration, local: &HashSet<IpAddr>) -> Option<Ipv4Addr> {
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
                    let addrs: Vec<IpAddr> = info
                        .get_addresses()
                        .iter()
                        .map(|a| a.to_ip_addr())
                        .collect();
                    if let Some(ip) = looks_like_motu(info.get_fullname(), &addrs, local) {
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
pub async fn mdns_discover(limit: Duration, local: HashSet<IpAddr>) -> Option<Ipv4Addr> {
    tokio::task::spawn_blocking(move || mdns_browse_blocking(limit, &local))
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

/// Ordered, de-duplicated MOTU candidates (see module docs). Empty = nothing found.
///
/// An explicit IP is returned alone, unprobed (the connect attempt validates
/// it). Otherwise: announcement and mDNS run concurrently (their order in the
/// result is fixed), then ARP neighbours that answer a TCP probe. Local
/// addresses never appear.
pub async fn discover(explicit: Option<IpAddr>, port: u16, iface: Option<&str>) -> Vec<IpAddr> {
    if let Some(ip) = explicit {
        return vec![ip];
    }
    let local = local_addrs();
    let (announced, mdns) = tokio::join!(
        announcement_discover(ANNOUNCE_PORT, ANNOUNCE_LISTEN, &local),
        mdns_discover(Duration::from_millis(1500), local.clone()),
    );
    let mut out: Vec<IpAddr> = Vec::new();
    let mut add = |ip: IpAddr, how: &str| {
        if !local.contains(&ip) && !out.contains(&ip) {
            tracing::info!(%ip, how, "MOTU candidate");
            out.push(ip);
        }
    };
    if let Some(ip) = announced {
        add(IpAddr::V4(ip), "announcement");
    }
    if let Some(ip) = mdns {
        add(IpAddr::V4(ip), "mDNS");
    }
    for ip in arp_candidates(iface).await {
        let ip = IpAddr::V4(ip);
        if !local.contains(&ip) && probe(ip, port, Duration::from_millis(500)).await {
            add(ip, "ARP + probe");
        }
    }
    if out.is_empty() {
        tracing::debug!("MOTU discovery found nothing");
    }
    out
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
        assert!(
            local_addrs().contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)) || local_addrs().is_empty()
        );
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
        let none = HashSet::new();
        // Link-local alone is not enough (our own cuemix-web announcement).
        assert!(looks_like_motu("cuemix-web._http._tcp.local.", &ll, &none).is_none());
        assert!(looks_like_motu("x._http._tcp.local.", &ll, &none).is_none());
        assert!(looks_like_motu("UltraLite-mk5._http._tcp.local.", &other, &none).is_some());
        assert!(looks_like_motu("my-MOTU._http._tcp.local.", &ll, &none).is_some());
        assert!(looks_like_motu("printer._http._tcp.local.", &other, &none).is_none());
    }

    #[test]
    fn mdns_excludes_local_addresses() {
        let own = Ipv4Addr::new(169, 254, 254, 1);
        let local: HashSet<IpAddr> = [IpAddr::V4(own)].into_iter().collect();
        let addrs = [IpAddr::V4(own), IpAddr::V4(Ipv4Addr::new(169, 254, 7, 7))];
        assert_eq!(
            looks_like_motu("UltraLite._http._tcp.local.", &addrs, &local),
            Some(Ipv4Addr::new(169, 254, 7, 7))
        );
        assert!(looks_like_motu("UltraLite._http._tcp.local.", &addrs[..1], &local).is_none());
    }

    const ANN: &[u8] = br#"{"uid":"0001f2","name":"UltraLite-mk5","ip":"169.254.5.9","model":"UltraLite-mk5","version":"2.0.8","interval":2000}"#;

    #[test]
    fn announcement_parser() {
        let src = Ipv4Addr::new(169, 254, 5, 9);
        assert_eq!(parse_announcement(ANN, src), Some(src));
        assert_eq!(parse_announcement(br#"{"name":"MOTU 8A"}"#, src), Some(src));
        // Non link-local source.
        assert_eq!(parse_announcement(ANN, Ipv4Addr::new(192, 0, 2, 9)), None);
        // Not JSON / not an object / unrelated device / not UTF-8 / empty.
        assert_eq!(parse_announcement(b"UltraLite", src), None);
        assert_eq!(parse_announcement(br#"["UltraLite"]"#, src), None);
        assert_eq!(parse_announcement(br#"{"name":"printer"}"#, src), None);
        assert_eq!(parse_announcement(&[0xff, 0xfe, 0x7b], src), None);
        assert_eq!(parse_announcement(b"", src), None);
    }

    #[tokio::test]
    async fn announcement_listener_never_fails_hard() {
        let Ok(held) = UdpSocket::bind("0.0.0.0:0").await else {
            return;
        };
        let port = held.local_addr().map(|a| a.port()).unwrap_or(0);
        // Port already bound (bind failure path) -> None, no panic.
        let r = announcement_discover(port, Duration::from_millis(100), &HashSet::new()).await;
        assert!(r.is_none());
        // Free port, nothing sent -> bounded wait, None.
        drop(held);
        let r = announcement_discover(port, Duration::from_millis(100), &HashSet::new()).await;
        assert!(r.is_none());
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
        assert_eq!(discover(Some(lo), port, None).await, vec![lo]);
    }
}
