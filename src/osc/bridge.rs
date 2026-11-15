//! Bidirectional OSC/UDP bridge.
//!
//! Incoming packets/bundles are decoded with `rosc`, mapped with
//! [`super::address::parse`] and forwarded to the engine. Every
//! [`ServerEvent::Changed`] is sent back as [`super::address::feedback`] to the
//! configured feedback targets plus every peer that recently sent us a valid
//! message, except the peer that caused the change (no fader echo).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime};
use tokio::net::UdpSocket;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::engine::{EngineHandle, Envelope, Origin, Shutdown};
use crate::osc::address;
use crate::state::{Param, ServerEvent};

/// Largest UDP datagram we accept.
const MAX_PACKET: usize = 64 * 1024;
/// Largest datagram we emit.
const MAX_DATAGRAM: usize = 8 * 1024;
/// Peers silent for longer than this stop receiving feedback.
const PEER_TTL: Duration = Duration::from_secs(10 * 60);
/// Maximum number of registered peers (oldest evicted).
const MAX_PEERS: usize = 64;
/// Bundle header: `#bundle\0` + timetag.
const BUNDLE_HEADER: usize = 16;

/// Registered feedback peers with last-seen times.
#[derive(Debug, Default)]
struct Peers {
    seen: HashMap<SocketAddr, Instant>,
}

impl Peers {
    /// Register or refresh a peer; returns `true` when it was not known.
    fn touch(&mut self, peer: SocketAddr, now: Instant) -> bool {
        let is_new = !self.seen.contains_key(&peer);
        if is_new && self.seen.len() >= MAX_PEERS {
            let oldest = self.seen.iter().min_by_key(|(_, t)| **t).map(|(a, _)| *a);
            if let Some(a) = oldest {
                self.seen.remove(&a);
            }
        }
        self.seen.insert(peer, now);
        is_new
    }

    fn expire(&mut self, now: Instant) {
        self.seen
            .retain(|_, t| now.saturating_duration_since(*t) < PEER_TTL);
    }
}

#[derive(Debug)]
pub struct OscBridge {
    socket: UdpSocket,
    targets: Vec<SocketAddr>,
}

impl OscBridge {
    pub async fn bind(
        listen: SocketAddr,
        feedback_targets: Vec<SocketAddr>,
    ) -> anyhow::Result<Self> {
        let socket = UdpSocket::bind(listen)
            .await
            .with_context(|| format!("cannot bind OSC socket on {listen}"))?;
        Ok(Self {
            socket,
            targets: feedback_targets,
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub async fn run(self, engine: EngineHandle, mut shutdown: Shutdown) -> anyhow::Result<()> {
        // Subscribe before anything else so no event is missed.
        let mut events = engine.subscribe();
        let mut peers = Peers::default();
        let mut buf = vec![0u8; MAX_PACKET];
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tracing::info!(addr = ?self.socket.local_addr().ok(), "OSC bridge listening");

        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                r = self.socket.recv_from(&mut buf) => match r {
                    Ok((n, peer)) => {
                        let Some(data) = buf.get(..n) else { continue };
                        if !self.on_datagram(data, peer, &mut peers, &engine).await {
                            break;
                        }
                    }
                    Err(e) => {
                        // e.g. ICMP "connection reset" surfaced on some platforms.
                        tracing::debug!(error = %e, "OSC recv error");
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                },
                ev = events.recv() => match ev {
                    Ok(env) => self.on_event(env, &peers).await,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "OSC bridge lagged, resyncing");
                        if let Some(dump) = Self::dump(&engine).await {
                            for to in self.recipients(&peers, None) {
                                self.send_all(&dump, to).await;
                            }
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = tick.tick() => peers.expire(Instant::now()),
            }
        }
        tracing::debug!("OSC bridge stopped");
        Ok(())
    }

    /// Handle one datagram; returns `false` when the engine is gone.
    async fn on_datagram(
        &self,
        data: &[u8],
        peer: SocketAddr,
        peers: &mut Peers,
        engine: &EngineHandle,
    ) -> bool {
        let packet = match rosc::decoder::decode_udp(data) {
            Ok((_, p)) => p,
            Err(e) => {
                tracing::debug!(%peer, error = ?e, "undecodable OSC packet");
                return true;
            }
        };
        let mut msgs = Vec::new();
        flatten(&packet, &mut msgs);

        let mut commands = Vec::new();
        for m in msgs {
            match address::parse(m) {
                Ok(c) => commands.push(c),
                Err(e) => tracing::debug!(%peer, addr = %m.addr, error = %e, "bad OSC message"),
            }
        }
        if commands.is_empty() {
            return true;
        }

        if peers.touch(peer, Instant::now()) {
            tracing::debug!(%peer, "OSC peer registered");
            if let Some(dump) = Self::dump(engine).await {
                self.send_all(&dump, peer).await;
            }
        }
        for c in commands {
            if engine.command(Origin::Osc(peer), c).await.is_err() {
                return false;
            }
        }
        true
    }

    async fn on_event(&self, env: Envelope, peers: &Peers) {
        let origin_peer = match env.origin {
            Origin::Osc(a) => Some(a),
            _ => None,
        };
        match env.event {
            ServerEvent::Changed { params } => {
                let msgs: Vec<OscMessage> = params.iter().map(address::feedback).collect();
                let datagrams = encode_chunks(&msgs);
                for to in self.recipients(peers, origin_peer) {
                    self.send_all(&datagrams, to).await;
                }
            }
            ServerEvent::Snapshot { state, .. } => {
                let datagrams = dump_datagrams(&state.all_params());
                for to in self.recipients(peers, None) {
                    self.send_all(&datagrams, to).await;
                }
            }
            ServerEvent::Error { message } if origin_peer.is_some() => {
                tracing::debug!(origin = ?env.origin, %message, "request from OSC peer rejected");
            }
            _ => {}
        }
    }

    /// Configured targets plus live peers, deduplicated, minus `exclude`.
    fn recipients(&self, peers: &Peers, exclude: Option<SocketAddr>) -> Vec<SocketAddr> {
        let mut out: Vec<SocketAddr> = Vec::new();
        for a in self.targets.iter().chain(peers.seen.keys()) {
            if Some(*a) != exclude && !out.contains(a) {
                out.push(*a);
            }
        }
        out
    }

    async fn send_all(&self, datagrams: &[Vec<u8>], to: SocketAddr) {
        for d in datagrams {
            if let Err(e) = self.socket.send_to(d, to).await {
                tracing::debug!(%to, error = %e, "OSC send failed");
                return;
            }
        }
    }

    /// Full state dump as ready-to-send datagrams.
    async fn dump(engine: &EngineHandle) -> Option<Vec<Vec<u8>>> {
        let events = engine.initial_events().await.ok()?;
        events.into_iter().find_map(|e| match e {
            ServerEvent::Snapshot { state, .. } => Some(dump_datagrams(&state.all_params())),
            _ => None,
        })
    }
}

fn dump_datagrams(params: &[Param]) -> Vec<Vec<u8>> {
    let msgs: Vec<OscMessage> = params.iter().map(address::feedback).collect();
    encode_chunks(&msgs)
}

/// Collect every message of a packet, recursing into bundles (timetags ignored).
fn flatten<'a>(packet: &'a OscPacket, out: &mut Vec<&'a OscMessage>) {
    match packet {
        OscPacket::Message(m) => out.push(m),
        OscPacket::Bundle(b) => b.content.iter().for_each(|p| flatten(p, out)),
    }
}

fn encode_packet(p: &OscPacket) -> Option<Vec<u8>> {
    match rosc::encoder::encode(p) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!(error = ?e, "OSC encode failed");
            None
        }
    }
}

/// Encode messages into datagrams of at most [`MAX_DATAGRAM`] bytes: a lone
/// message is sent bare, several are grouped in immediate-timetag bundles.
fn encode_chunks(msgs: &[OscMessage]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut group: Vec<OscPacket> = Vec::new();
    let mut size = BUNDLE_HEADER;

    let mut flush = |group: &mut Vec<OscPacket>, size: &mut usize| {
        let packet = match group.len() {
            0 => None,
            1 => group.pop(),
            _ => Some(OscPacket::Bundle(OscBundle {
                timetag: OscTime {
                    seconds: 0,
                    fractional: 1,
                },
                content: std::mem::take(group),
            })),
        };
        group.clear();
        *size = BUNDLE_HEADER;
        if let Some(d) = packet.as_ref().and_then(encode_packet) {
            out.push(d);
        }
    };

    for m in msgs {
        let packet = OscPacket::Message(m.clone());
        let Some(len) = encode_packet(&packet).map(|d| d.len()) else {
            continue;
        };
        if !group.is_empty() && size + 4 + len > MAX_DATAGRAM {
            flush(&mut group, &mut size);
        }
        size += 4 + len;
        group.push(packet);
    }
    flush(&mut group, &mut size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_respect_datagram_limit() {
        let params = crate::state::MotuState::default().all_params();
        let dgrams = dump_datagrams(&params);
        assert!(dgrams.len() > 1);
        assert!(dgrams.iter().all(|d| d.len() <= MAX_DATAGRAM));
        let mut n = 0;
        for d in &dgrams {
            let (_, p) = rosc::decoder::decode_udp(d).expect("decodes");
            let mut v = Vec::new();
            flatten(&p, &mut v);
            n += v.len();
        }
        assert_eq!(n, params.len());
    }

    #[test]
    fn peers_cap_and_expiry() {
        let mut p = Peers::default();
        let t0 = Instant::now();
        for i in 0..70u16 {
            p.touch(SocketAddr::from(([127, 0, 0, 1], 1000 + i)), t0);
        }
        assert_eq!(p.seen.len(), MAX_PEERS);
        p.expire(t0 + PEER_TTL + Duration::from_secs(1));
        assert!(p.seen.is_empty());
    }
}
