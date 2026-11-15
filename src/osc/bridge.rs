//! Bidirectional OSC/UDP bridge.
//!
//! Incoming packets/bundles are decoded with `rosc`, mapped with
//! [`super::address::parse`] and forwarded to the engine. Every
//! [`ServerEvent::Changed`] is sent back as [`super::address::feedback`] to the
//! configured feedback targets plus every peer that recently sent us a valid
//! message, except the peer that caused the change (no fader echo).

use std::collections::{HashMap, VecDeque};
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
/// Deepest bundle nesting accepted (a top-level bundle is depth 1). Deeper
/// packets are dropped before `rosc` ever sees them (stack-overflow guard).
const MAX_BUNDLE_DEPTH: usize = 8;
/// Global minimum spacing between two full state dumps, across all peers.
/// Dump requests arriving sooner are queued (once per peer) and served, one per
/// window, when it reopens; queued peers that expired meanwhile are dropped.
const DUMP_INTERVAL: Duration = Duration::from_secs(1);
/// At most this many new peers are enrolled per [`ENROL_WINDOW`].
const MAX_ENROL: usize = 8;
/// Sliding window for [`MAX_ENROL`].
const ENROL_WINDOW: Duration = Duration::from_secs(10);

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

    fn is_live(&self, peer: &SocketAddr, now: Instant) -> bool {
        self.seen
            .get(peer)
            .is_some_and(|t| now.saturating_duration_since(*t) < PEER_TTL)
    }
}

/// Global full-dump rate limiter with a coalescing queue.
#[derive(Debug, Default)]
struct DumpQueue {
    last: Option<Instant>,
    pending: VecDeque<SocketAddr>,
}

impl DumpQueue {
    fn ready(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|l| now.saturating_duration_since(l) >= DUMP_INTERVAL)
    }

    /// `true`: send a dump to `peer` now. `false`: queued (at most once).
    fn request(&mut self, peer: SocketAddr, now: Instant) -> bool {
        if self.pending.is_empty() && self.ready(now) {
            self.last = Some(now);
            return true;
        }
        if !self.pending.contains(&peer) {
            self.pending.push_back(peer);
        }
        false
    }

    /// When the next queued dump may be sent.
    fn deadline(&self) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.last.map_or_else(Instant::now, |l| l + DUMP_INTERVAL))
    }

    /// Pop the next queued peer that is still alive, if the window is open.
    fn next_due(&mut self, now: Instant, peers: &Peers) -> Option<SocketAddr> {
        if !self.ready(now) {
            return None;
        }
        while let Some(p) = self.pending.pop_front() {
            if peers.is_live(&p, now) {
                self.last = Some(now);
                return Some(p);
            }
        }
        None
    }
}

/// Limits how many new peers may be enrolled per [`ENROL_WINDOW`].
#[derive(Debug, Default)]
struct EnrolLimiter {
    recent: VecDeque<Instant>,
}

impl EnrolLimiter {
    fn allow(&mut self, now: Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= ENROL_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= MAX_ENROL {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

/// Mutable per-bridge state.
#[derive(Debug, Default)]
struct Link {
    peers: Peers,
    dumps: DumpQueue,
    enrol: EnrolLimiter,
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
        let mut link = Link::default();
        let mut buf = vec![0u8; MAX_PACKET];
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tracing::info!(addr = ?self.socket.local_addr().ok(), "OSC bridge listening");

        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                r = self.socket.recv_from(&mut buf) => match r {
                    Ok((n, peer)) => {
                        let Some(data) = buf.get(..n) else { continue };
                        if !self.on_datagram(data, peer, &mut link, &engine).await {
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
                    Ok(env) => self.on_event(env, &link.peers).await,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "OSC bridge lagged, resyncing");
                        if let Some(dump) = Self::dump(&engine).await {
                            for to in self.recipients(&link.peers, None) {
                                self.send_all(&dump, to).await;
                            }
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
                () = wait_until(link.dumps.deadline()) => {
                    let now = Instant::now();
                    if let Some(to) = link.dumps.next_due(now, &link.peers) {
                        if let Some(dump) = Self::dump(&engine).await {
                            self.send_all(&dump, to).await;
                        }
                    }
                }
                _ = tick.tick() => link.peers.expire(Instant::now()),
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
        link: &mut Link,
        engine: &EngineHandle,
    ) -> bool {
        let Some(packet) = decode_guarded(data) else {
            tracing::debug!(%peer, "undecodable or too deeply nested OSC packet");
            return true;
        };
        let mut msgs = Vec::new();
        flatten(&packet, &mut msgs);

        let mut commands = Vec::new();
        let mut sync = false;
        for m in msgs {
            if address::is_sync(m) {
                sync = true;
                continue;
            }
            match address::parse(m) {
                Ok(c) => commands.push(c),
                Err(e) => tracing::debug!(%peer, addr = %m.addr, error = %e, "bad OSC message"),
            }
        }
        if commands.is_empty() && !sync {
            return true;
        }

        let now = Instant::now();
        let known = link.peers.is_live(&peer, now);
        let enrolled = if known {
            link.peers.touch(peer, now);
            true
        } else if link.enrol.allow(now) {
            link.peers.touch(peer, now);
            tracing::debug!(%peer, "OSC peer registered");
            true
        } else {
            tracing::debug!(%peer, "OSC peer enrolment rate-limited");
            false
        };
        if enrolled && (sync || !known) && link.dumps.request(peer, now) {
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
                let msgs: Vec<OscMessage> = params.iter().filter_map(address::feedback).collect();
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
    let msgs: Vec<OscMessage> = params.iter().filter_map(address::feedback).collect();
    encode_chunks(&msgs)
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// Cheap iterative pre-scan of raw bytes: `false` when bundles nest deeper than
/// [`MAX_BUNDLE_DEPTH`] or any bundle element size is malformed. Never panics,
/// never recurses. Plain messages pass (their validity is `rosc`'s job).
fn bundle_depth_ok(data: &[u8]) -> bool {
    const TAG: &[u8] = b"#bundle\0";
    if !data.starts_with(TAG) {
        return true;
    }
    if data.len() < BUNDLE_HEADER {
        return false;
    }
    // Each frame: (cursor, end) over the elements of one bundle.
    let mut stack: Vec<(usize, usize)> = vec![(BUNDLE_HEADER, data.len())];
    while let Some(&(pos, end)) = stack.last() {
        if pos == end {
            stack.pop();
            continue;
        }
        let Some(size_bytes) = pos
            .checked_add(4)
            .filter(|p| *p <= end)
            .and_then(|p| data.get(pos..p))
        else {
            return false;
        };
        let mut raw = [0u8; 4];
        raw.copy_from_slice(size_bytes);
        let Ok(size) = usize::try_from(u32::from_be_bytes(raw)) else {
            return false;
        };
        let start = pos + 4;
        let Some(elem_end) = start.checked_add(size).filter(|e| *e <= end) else {
            return false;
        };
        if let Some(top) = stack.last_mut() {
            top.0 = elem_end;
        }
        let Some(elem) = data.get(start..elem_end) else {
            return false;
        };
        if elem.starts_with(TAG) {
            if size < BUNDLE_HEADER || stack.len() >= MAX_BUNDLE_DEPTH {
                return false;
            }
            stack.push((start + BUNDLE_HEADER, elem_end));
        }
    }
    true
}

/// Decode a datagram after the nesting pre-scan; `None` when rejected.
fn decode_guarded(data: &[u8]) -> Option<OscPacket> {
    if !bundle_depth_ok(data) {
        return None;
    }
    rosc::decoder::decode_udp(data).ok().map(|(_, p)| p)
}

/// Collect every message of a packet in document order, descending into
/// bundles iteratively (timetags ignored, depth capped at [`MAX_BUNDLE_DEPTH`]).
fn flatten<'a>(packet: &'a OscPacket, out: &mut Vec<&'a OscMessage>) {
    let mut stack: Vec<(&'a OscPacket, usize)> = vec![(packet, 1)];
    while let Some((p, depth)) = stack.pop() {
        match p {
            OscPacket::Message(m) => out.push(m),
            OscPacket::Bundle(b) if depth <= MAX_BUNDLE_DEPTH => {
                stack.extend(b.content.iter().rev().map(|c| (c, depth + 1)));
            }
            OscPacket::Bundle(_) => {}
        }
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
        // DSP / reverb / clock params have no OSC address.
        let osc_addressable = params
            .iter()
            .filter(|p| address::feedback(p).is_some())
            .count();
        assert_eq!(n, osc_addressable);
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

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn dump_queue_rate_limits_and_coalesces() {
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let (a, b) = (addr(1), addr(2));
        peers.touch(a, t0);
        peers.touch(b, t0);
        let mut q = DumpQueue::default();
        assert!(q.request(a, t0));
        assert!(!q.request(b, t0 + Duration::from_millis(10)));
        assert!(!q.request(b, t0 + Duration::from_millis(20)));
        assert_eq!(q.pending.len(), 1);
        assert_eq!(q.deadline(), Some(t0 + DUMP_INTERVAL));
        assert_eq!(q.next_due(t0 + Duration::from_millis(500), &peers), None);
        assert_eq!(q.next_due(t0 + DUMP_INTERVAL, &peers), Some(b));
        assert_eq!(q.deadline(), None);
    }

    #[test]
    fn dump_queue_drops_expired_peers() {
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let (a, b) = (addr(1), addr(2));
        peers.touch(a, t0);
        let mut q = DumpQueue::default();
        assert!(q.request(a, t0));
        assert!(!q.request(b, t0)); // b never enrolled
        assert_eq!(q.next_due(t0 + DUMP_INTERVAL, &peers), None);
        assert!(q.pending.is_empty());
    }

    #[test]
    fn enrol_limiter_caps_per_window() {
        let t0 = Instant::now();
        let mut e = EnrolLimiter::default();
        for _ in 0..MAX_ENROL {
            assert!(e.allow(t0));
        }
        assert!(!e.allow(t0 + Duration::from_secs(1)));
        assert!(e.allow(t0 + ENROL_WINDOW));
    }

    fn nested(depth: usize) -> Vec<u8> {
        let mut cur = rosc::encoder::encode(&OscPacket::Message(OscMessage {
            addr: "/mix/1/ch/1/vol".into(),
            args: vec![rosc::OscType::Float(0.5)],
        }))
        .expect("encode");
        for _ in 0..depth {
            let mut b = b"#bundle\0".to_vec();
            b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
            b.extend_from_slice(&(cur.len() as u32).to_be_bytes());
            b.extend_from_slice(&cur);
            cur = b;
        }
        cur
    }

    #[test]
    fn nesting_limit() {
        assert!(decode_guarded(&nested(MAX_BUNDLE_DEPTH)).is_some());
        assert!(decode_guarded(&nested(MAX_BUNDLE_DEPTH + 1)).is_none());
        let deep = nested(3000);
        assert!(deep.len() <= MAX_PACKET);
        assert!(decode_guarded(&deep).is_none());
        let p = decode_guarded(&nested(MAX_BUNDLE_DEPTH)).expect("ok");
        let mut v = Vec::new();
        flatten(&p, &mut v);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn fuzz_never_panics() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let seed = nested(5);
        for i in 0..20_000 {
            let mut d: Vec<u8> = if i % 3 == 0 {
                seed.clone()
            } else {
                let len = (next() % 200) as usize;
                (0..len).map(|_| next() as u8).collect()
            };
            if i % 3 == 0 {
                for _ in 0..(next() % 6) {
                    let n = d.len().max(1);
                    let idx = (next() as usize) % n;
                    if let Some(b) = d.get_mut(idx) {
                        *b = next() as u8;
                    }
                }
                let cut = (next() as usize) % (d.len() + 1);
                d.truncate(cut);
            } else if i % 2 == 0 {
                let mut h = b"#bundle\0".to_vec();
                h.extend_from_slice(&d);
                d = h;
            }
            if let Some(p) = decode_guarded(&d) {
                let mut v = Vec::new();
                flatten(&p, &mut v);
                for m in v {
                    let _ = address::parse(m);
                }
            }
        }
    }
}
