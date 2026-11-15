//! Zeroconf announcement of the web UI (`http://<mdns_hostname>.local:<port>`).
//!
//! Best effort: any failure is logged and the server keeps running (on
//! Proxmox/RPi deployments `avahi-daemon` may also announce the host).

use mdns_sd::{ServiceDaemon, ServiceInfo};

const SERVICE_TYPE: &str = "_http._tcp.local.";

/// Keeps the announcement alive; unregisters on [`Announcement::stop`].
pub struct Announcement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Announcement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Announcement")
            .field("fullname", &self.fullname)
            .finish_non_exhaustive()
    }
}

/// Announce `hostname` (without `.local`) on `port`. `None` on failure.
pub fn announce(hostname: &str, port: u16) -> Option<Announcement> {
    let host = hostname
        .trim()
        .trim_end_matches(".local")
        .trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mDNS unavailable, not announcing");
            return None;
        }
    };
    let props = [("path", "/"), ("version", env!("CARGO_PKG_VERSION"))];
    let info = match ServiceInfo::new(
        SERVICE_TYPE,
        host,
        &format!("{host}.local."),
        "",
        port,
        &props[..],
    ) {
        Ok(i) => i.enable_addr_auto(),
        Err(e) => {
            tracing::warn!(error = %e, "invalid mDNS service info");
            return None;
        }
    };
    let fullname = info.get_fullname().to_owned();
    if let Err(e) = daemon.register(info) {
        tracing::warn!(error = %e, "mDNS registration failed");
        return None;
    }
    tracing::info!(url = %format!("http://{host}.local:{port}"), "announced via mDNS");
    Some(Announcement { daemon, fullname })
}

impl Announcement {
    /// Unregister (sends the mDNS goodbye) and stop the daemon, waiting
    /// briefly for each acknowledgement so the daemon never writes to a
    /// dropped channel.
    pub fn stop(self) {
        const ACK: std::time::Duration = std::time::Duration::from_millis(500);
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(ACK);
        }
        if let Ok(rx) = self.daemon.shutdown() {
            let _ = rx.recv_timeout(ACK);
        }
    }
}
