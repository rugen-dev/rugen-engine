//! Direct peer-to-peer path (a small ICE: candidates, simultaneous hole
//! punching, authenticated connectivity checks), the default transport of
//! online matches. TURN ([`crate::turn`]) and the server relay stay up as
//! fallbacks; [`crate::path::PathSelector`] picks the path.
//!
//! * One UDP socket per address family (IPv4, IPv6 when the machine has a
//!   global IPv6 route) carries everything of the match: STUN, checks and
//!   ggrs traffic.
//! * Candidates: host (the machine's LAN / global addresses), server
//!   reflexive (Cloudflare STUN; a second probe to another port detects an
//!   endpoint-dependent, "symmetric", NAT) and peer reflexive (learned from
//!   an authenticated check arriving from an unknown address).
//! * Signalling goes through the authenticated relay session as opaque
//!   `T_DIRECT` payloads, only between the two players (relay `DATA` is
//!   addressed to one slot; spectators never see it). Each side first
//!   sends a hello with its privacy setting and a random nonce; candidates
//!   are exchanged only when both players allow direct connections.
//! * The per-match key is HMAC-SHA256 over both nonces and the session id.
//!   Every datagram on the direct path (checks and ggrs traffic) carries a
//!   per-sender counter and a truncated HMAC-SHA256 tag; anything that does
//!   not verify, replays an old counter or comes from the wrong slot is
//!   dropped before parsing.
//! * Checks run on every candidate pair at once (both sides send, which
//!   opens both NATs); the pair with the lowest round trip wins, background
//!   checks keep measuring and keep NAT mappings alive (every 1 s, then
//!   every 10 s), and a better pair takes over with hysteresis.
//! * Both sides behind symmetric NATs: direct is skipped (TURN instead).
//!
//! Privacy: with `[net] direct = false` on either side no host or
//! reflexive candidate is ever sent, and no address is ever logged.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use ring::hmac;

use crate::batch::Batcher;
use crate::path::Srtt;
use crate::stun::{self, Builder, Class, attr, method};

/// Relay payload type of direct-path signalling (after `T_TURN` = 3,
/// `T_BATCH` = 4 and the TURN probes 5 / 6).
pub(crate) const T_DIRECT: u8 = 8;

const SIG_HELLO: u8 = 0;
const SIG_CANDIDATES: u8 = 1;
const SIG_GIVE_UP: u8 = 2;

const HELLO_ALLOWED: u8 = 1;
const HELLO_SYMMETRIC: u8 = 2;
const HELLO_HEARD: u8 = 4;

/// First byte of every direct datagram (never a STUN message: those start
/// with two zero bits).
const MAGIC: u8 = 0xD7;
const D_CHECK: u8 = 1;
const D_ACK: u8 = 2;
const D_DATA: u8 = 3;
/// magic, type, from slot, counter u32
const HEAD: usize = 7;
const TAG: usize = 8;

/// The default STUN server (Cloudflare anycast).
pub const DEFAULT_STUN: &str = "stun.cloudflare.com:3478";
/// Second STUN destination: same service, another port (a different
/// mapping there means an endpoint-dependent NAT).
const SECOND_STUN_PORT: u16 = 443;

/// Unhealthy after this long without an authenticated datagram.
const HEALTHY: Duration = Duration::from_millis(800);
/// Give up on direct when no pair works after this long.
const GIVE_UP: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_millis(50);
const STUN_RETRY: Duration = Duration::from_millis(300);
const STUN_BUDGET: Duration = Duration::from_secs(3);

/// Kind of a candidate address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandKind {
    Host = 0,
    Srflx = 1,
    Prflx = 2,
}

impl CandKind {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Host),
            1 => Some(Self::Srflx),
            2 => Some(Self::Prflx),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: CandKind,
    pub addr: SocketAddr,
}

/// Wire form of a candidate list: `gen count (kind family ip port)*`.
pub fn encode_candidates(generation: u8, cands: &[Candidate]) -> Vec<u8> {
    let mut out = vec![generation, cands.len().min(16) as u8];
    for c in cands.iter().take(16) {
        out.push(c.kind as u8);
        match c.addr.ip() {
            IpAddr::V4(ip) => {
                out.push(4);
                out.extend(ip.octets());
            }
            IpAddr::V6(ip) => {
                out.push(6);
                out.extend(ip.octets());
            }
        }
        out.extend(c.addr.port().to_be_bytes());
    }
    out
}

pub fn decode_candidates(p: &[u8]) -> Option<(u8, Vec<Candidate>)> {
    let (&generation, rest) = p.split_first()?;
    let (&n, mut rest) = rest.split_first()?;
    let mut out = Vec::new();
    for _ in 0..n.min(16) {
        let kind = CandKind::from_u8(*rest.first()?)?;
        let fam = *rest.get(1)?;
        let (ip, len) = match fam {
            4 => {
                let o: [u8; 4] = rest.get(2..6)?.try_into().ok()?;
                (IpAddr::V4(Ipv4Addr::from(o)), 4)
            }
            6 => {
                let o: [u8; 16] = rest.get(2..18)?.try_into().ok()?;
                (IpAddr::V6(Ipv6Addr::from(o)), 16)
            }
            _ => return None,
        };
        let port = u16::from_be_bytes(rest.get(2 + len..4 + len)?.try_into().ok()?);
        if port != 0 && !ip.is_unspecified() && !ip.is_multicast() {
            out.push(Candidate {
                kind,
                addr: SocketAddr::new(ip, port),
            });
        }
        rest = &rest[4 + len..];
    }
    Some((generation, out))
}

/// The per-match key from both players' hello nonces (slot 0's first) and
/// the session id.
pub fn match_key(nonce0: &[u8; 16], nonce1: &[u8; 16], session: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..16].copy_from_slice(nonce0);
    k[16..].copy_from_slice(nonce1);
    let key = hmac::Key::new(hmac::HMAC_SHA256, &k);
    let mut msg = b"rugen direct v1".to_vec();
    msg.extend(session.to_be_bytes());
    let tag = hmac::sign(&key, &msg);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// Seals and opens direct datagrams: header, body, truncated HMAC tag,
/// and a sliding replay window over the peer's counters.
pub struct Sealer {
    key: hmac::Key,
    slot: u8,
    peer: u8,
    tx: u32,
    /// Highest counter seen from the peer and a bitmap of the 64 below it.
    rx_top: u32,
    rx_seen: u64,
    rx_any: bool,
}

impl Sealer {
    pub fn new(key: &[u8; 32], slot: u8, peer: u8) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, key),
            slot,
            peer,
            tx: 0,
            rx_top: 0,
            rx_seen: 0,
            rx_any: false,
        }
    }

    pub fn seal(&mut self, kind: u8, body: &[u8]) -> Vec<u8> {
        self.tx = self.tx.wrapping_add(1);
        let mut out = Vec::with_capacity(HEAD + body.len() + TAG);
        out.extend([MAGIC, kind, self.slot]);
        out.extend(self.tx.to_be_bytes());
        out.extend_from_slice(body);
        let tag = hmac::sign(&self.key, &out);
        out.extend_from_slice(&tag.as_ref()[..TAG]);
        out
    }

    /// `(kind, body)` of an authentic, fresh datagram from the peer.
    pub fn open<'a>(&mut self, d: &'a [u8]) -> Option<(u8, &'a [u8])> {
        if d.len() < HEAD + TAG || d[0] != MAGIC || d[2] != self.peer {
            return None;
        }
        let (msg, tag) = d.split_at(d.len() - TAG);
        let want = hmac::sign(&self.key, msg);
        // constant time compare of the truncated tag
        let diff = want.as_ref()[..TAG]
            .iter()
            .zip(tag)
            .fold(0u8, |a, (x, y)| a | (x ^ y));
        if diff != 0 {
            return None;
        }
        let ctr = u32::from_be_bytes(d[3..7].try_into().ok()?);
        if !self.fresh(ctr) {
            return None;
        }
        Some((d[1], &msg[HEAD..]))
    }

    fn fresh(&mut self, ctr: u32) -> bool {
        if !self.rx_any {
            self.rx_any = true;
            self.rx_top = ctr;
            self.rx_seen = 1;
            return true;
        }
        if ctr > self.rx_top {
            let shift = ctr - self.rx_top;
            self.rx_seen = if shift >= 64 {
                0
            } else {
                self.rx_seen << shift
            };
            self.rx_seen |= 1;
            self.rx_top = ctr;
            return true;
        }
        let back = self.rx_top - ctr;
        if back >= 64 || self.rx_seen & (1 << back) != 0 {
            return false;
        }
        self.rx_seen |= 1 << back;
        true
    }
}

/// Global addresses worth offering as host candidates: the address the OS
/// routes to the internet from (route lookup, nothing is sent). IPv6 only
/// for global unicast (2000::/3).
fn host_ips() -> (Option<IpAddr>, Option<IpAddr>) {
    let v4 = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .ok()
        .map(|a| a.ip())
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback());
    let v6 = UdpSocket::bind("[::]:0")
        .and_then(|s| s.connect("[2001:4860:4860::8888]:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .ok()
        .map(|a| a.ip())
        .filter(|ip| matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xE000 == 0x2000));
    (v4, v6)
}

/// A STUN binding probe from one of our sockets.
struct StunProbe {
    fam: usize,
    server: SocketAddr,
    tid: [u8; 12],
    sent: Option<Instant>,
    tries: u32,
    mapped: Option<SocketAddr>,
    /// The second probe (symmetric NAT detection), not a candidate source
    /// unless the first failed.
    second: bool,
}

#[derive(Clone, Debug)]
struct Pair {
    fam: usize,
    remote: Candidate,
    srtt: Srtt,
    /// Outstanding checks: (id, sent at).
    inflight: VecDeque<(u64, Instant)>,
    last_check: Option<Instant>,
    last_ok: Option<Instant>,
    checks: u32,
}

impl Pair {
    fn ok_recently(&self, now: Instant) -> bool {
        self.last_ok
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(3))
    }
}

/// Settings of the direct path for one match.
#[derive(Clone, Debug)]
pub struct DirectConfig {
    /// `[net] direct`: offer our addresses to the opponent.
    pub allowed: bool,
    /// STUN servers (`host:port`); the first answers the reflexive address.
    pub stun: Vec<String>,
    /// Host candidates only (tests, LAN): no STUN.
    pub host_only: bool,
    /// Bind the sockets to loopback and offer 127.0.0.1 (tests).
    pub loopback: bool,
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            allowed: true,
            stun: vec![DEFAULT_STUN.to_string()],
            host_only: false,
            loopback: false,
        }
    }
}

/// Why the direct path is not used (for logs and the HUD).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    /// This player turned direct connections off.
    LocalPrivacy,
    /// The opponent turned direct connections off.
    PeerPrivacy,
    /// Both players are behind endpoint-dependent (symmetric) NATs.
    BothSymmetric,
    /// No candidate pair answered in time.
    NoPath,
    /// No usable socket / address.
    NoCandidates,
}

impl Off {
    pub fn reason(self) -> &'static str {
        match self {
            Off::LocalPrivacy => "direct connections are off in OPTIONS",
            Off::PeerPrivacy => "the other player keeps direct connections off",
            Off::BothSymmetric => "both players are behind symmetric NATs",
            Off::NoPath => "no direct path answered",
            Off::NoCandidates => "no network address to offer",
        }
    }
}

pub(crate) struct Direct {
    pub(crate) peer: u8,
    slot: u8,
    session: u32,
    allowed: bool,
    socks: [Option<UdpSocket>; 2],
    nonce: [u8; 16],
    peer_hello: Option<(u8, [u8; 16])>,
    sealer: Option<Sealer>,
    local: Vec<Candidate>,
    local_gen: u8,
    sent_gen: Option<(u8, Instant)>,
    remote_gen: Option<u8>,
    resolving: Option<Receiver<Vec<SocketAddr>>>,
    probes: Vec<StunProbe>,
    stun_started: Instant,
    symmetric: Option<bool>,
    pairs: Vec<Pair>,
    selected: Option<usize>,
    off: Option<Off>,
    started: Instant,
    clock: Instant,
    last_hello: Option<Instant>,
    hello_sent: Option<Vec<u8>>,
    last_rx: Instant,
    heard: bool,
    batcher: Batcher,
    pub(crate) inbox: VecDeque<Vec<u8>>,
    buf: Vec<u8>,
    next_check_id: u64,
    /// Datagrams dropped as unauthenticated / replayed.
    pub(crate) forged: u64,
    pub(crate) switches: u32,
    loopback: bool,
}

fn random16() -> [u8; 16] {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 16];
    if ring::rand::SystemRandom::new().fill(&mut b).is_err() {
        // never weaker than the hasher's random keys
        let s = std::hash::RandomState::new();
        use std::hash::BuildHasher;
        b[..8].copy_from_slice(&s.hash_one(Instant::now()).to_le_bytes());
        b[8..].copy_from_slice(&s.hash_one(std::process::id()).to_le_bytes());
    }
    b
}

fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionRefused
    )
}

fn fam_of(a: &SocketAddr) -> usize {
    usize::from(a.is_ipv6())
}

impl Direct {
    /// The direct path of a player (slot 0 / 1) in relay session `session`.
    pub(crate) fn start(cfg: &DirectConfig, session: u32, slot: u8) -> Option<Self> {
        if slot > 1 {
            return None;
        }
        let now = Instant::now();
        let mut d = Self {
            peer: 1 - slot,
            slot,
            session,
            allowed: cfg.allowed,
            socks: [None, None],
            nonce: random16(),
            peer_hello: None,
            sealer: None,
            local: Vec::new(),
            local_gen: 0,
            sent_gen: None,
            remote_gen: None,
            resolving: None,
            probes: Vec::new(),
            stun_started: now,
            symmetric: None,
            pairs: Vec::new(),
            selected: None,
            off: None,
            started: now,
            clock: now,
            last_hello: None,
            hello_sent: None,
            last_rx: now - HEALTHY,
            heard: false,
            batcher: Batcher::new(false),
            inbox: VecDeque::new(),
            buf: vec![0; 2048],
            next_check_id: 1,
            forged: 0,
            switches: 0,
            loopback: cfg.loopback,
        };
        if !cfg.allowed {
            d.off = Some(Off::LocalPrivacy);
            return Some(d);
        }
        if cfg.loopback {
            if let Ok(s) = UdpSocket::bind("127.0.0.1:0")
                && s.set_nonblocking(true).is_ok()
                && let Ok(a) = s.local_addr()
            {
                d.local.push(Candidate {
                    kind: CandKind::Host,
                    addr: a,
                });
                d.socks[0] = Some(s);
            }
        } else {
            let (ip4, ip6) = host_ips();
            for (fam, bind, ip) in [(0, "0.0.0.0:0", ip4), (1, "[::]:0", ip6)] {
                if fam == 1 && ip.is_none() {
                    continue;
                }
                let Ok(s) = UdpSocket::bind(bind) else {
                    continue;
                };
                if s.set_nonblocking(true).is_err() {
                    continue;
                }
                if let (Some(ip), Ok(a)) = (ip, s.local_addr()) {
                    d.local.push(Candidate {
                        kind: CandKind::Host,
                        addr: SocketAddr::new(ip, a.port()),
                    });
                }
                d.socks[fam] = Some(s);
            }
            if !cfg.host_only && d.socks[0].is_some() {
                let servers = cfg.stun.clone();
                let (tx, rx) = channel();
                let spawned = std::thread::Builder::new()
                    .name("stun-resolve".into())
                    .spawn(move || {
                        let mut out = Vec::new();
                        for s in servers {
                            if let Ok(a) = s.to_socket_addrs() {
                                out.extend(a.filter(SocketAddr::is_ipv4).take(1));
                            }
                            if !out.is_empty() {
                                break;
                            }
                        }
                        let _ = tx.send(out);
                    });
                if spawned.is_ok() {
                    d.resolving = Some(rx);
                }
            }
        }
        if d.socks.iter().all(Option::is_none) {
            d.off = Some(Off::NoCandidates);
        }
        Some(d)
    }

    fn now_us(&self) -> u64 {
        self.clock.elapsed().as_micros() as u64
    }

    /// Working towards a direct path (not given up, none selected yet).
    pub(crate) fn pending(&self) -> bool {
        self.off.is_none() && self.selected.is_none()
    }

    /// Why direct is not used, once that is known.
    pub(crate) fn off(&self) -> Option<Off> {
        self.off
    }

    /// The direct path carries traffic to `to`.
    pub(crate) fn carries(&self, to: u8) -> bool {
        to == self.peer && self.selected.is_some() && self.off.is_none()
    }

    pub(crate) fn healthy(&self) -> bool {
        self.selected.is_some() && self.last_rx.elapsed() < HEALTHY
    }

    /// Smoothed round trip / jitter of the selected pair (ms).
    pub(crate) fn srtt(&self) -> Option<Srtt> {
        self.selected.map(|i| self.pairs[i].srtt)
    }

    /// Kind of the selected pair's remote candidate.
    pub(crate) fn selected_kind(&self) -> Option<CandKind> {
        self.selected.map(|i| self.pairs[i].remote.kind)
    }

    pub(crate) fn symmetric(&self) -> Option<bool> {
        self.symmetric
    }

    fn give_up(&mut self, why: Off, out: &mut Vec<Vec<u8>>) {
        if self.off.is_none() {
            log::info!("direct: not used ({})", why.reason());
            self.off = Some(why);
            self.selected = None;
            if why != Off::PeerPrivacy {
                out.push(vec![T_DIRECT, SIG_GIVE_UP]);
            }
        }
    }

    fn hello(&self) -> Vec<u8> {
        let mut flags = 0;
        if self.allowed {
            flags |= HELLO_ALLOWED;
        }
        if self.symmetric == Some(true) {
            flags |= HELLO_SYMMETRIC;
        }
        if self.peer_hello.is_some() {
            flags |= HELLO_HEARD;
        }
        let mut p = vec![T_DIRECT, SIG_HELLO, 1, flags];
        p.extend(self.nonce);
        p
    }

    fn both_allowed(&self) -> bool {
        self.allowed
            && self
                .peer_hello
                .is_some_and(|(flags, _)| flags & HELLO_ALLOWED != 0)
    }

    /// Advance gathering, signalling, checks and keep-alives. Returns relay
    /// payloads for the peer (signalling through the server).
    pub(crate) fn pump(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let now = Instant::now();
        // hello until the peer has heard ours (it says so in its hello),
        // and again whenever our flags change (NAT type learnt later)
        let peer_heard = self.peer_hello.is_some_and(|(f, _)| f & HELLO_HEARD != 0);
        let hello = self.hello();
        let changed = self.hello_sent.as_ref().is_some_and(|h| h[3] != hello[3]);
        let every = Duration::from_millis(if self.peer_hello.is_some() { 400 } else { 200 });
        if (!peer_heard || changed)
            && self
                .last_hello
                .is_none_or(|t| changed || now.duration_since(t) >= every)
            && now.duration_since(self.started) < Duration::from_secs(20)
        {
            out.push(hello.clone());
            self.hello_sent = Some(hello);
            self.last_hello = Some(now);
        }
        if self.off.is_some() {
            return out;
        }
        self.gather(now);
        if !self.both_allowed() {
            return out;
        }
        // candidates: on every new generation, then again until the peer
        // answers a check (relay UDP may drop a datagram)
        if !self.local.is_empty() {
            let resend = self.sent_gen.is_none_or(|(g, t)| {
                g != self.local_gen
                    || (!self.heard && now.duration_since(t) >= Duration::from_millis(400))
            });
            if resend {
                let mut p = vec![T_DIRECT, SIG_CANDIDATES];
                p.extend(encode_candidates(self.local_gen, &self.local));
                out.push(p);
                self.sent_gen = Some((self.local_gen, now));
            }
        }
        let peer_symmetric = self
            .peer_hello
            .is_some_and(|(f, _)| f & HELLO_SYMMETRIC != 0);
        // both behind endpoint-dependent NATs: reflexive pairs cannot work;
        // only host pairs (same network) get a short chance
        if self.symmetric == Some(true) && peer_symmetric && self.selected.is_none() {
            self.pairs.retain(|p| p.remote.kind == CandKind::Host);
            if now.duration_since(self.started) > Duration::from_millis(2500) {
                self.give_up(Off::BothSymmetric, &mut out);
                return out;
            }
        }
        self.check(now);
        if self.selected.is_none() && now.duration_since(self.started) > GIVE_UP {
            self.give_up(Off::NoPath, &mut out);
        }
        if self.batcher.due() {
            self.flush();
        }
        out
    }

    /// STUN: resolve the server, send binding requests (retrying), collect
    /// the mapped addresses.
    fn gather(&mut self, now: Instant) {
        if let Some(rx) = &self.resolving {
            match rx.try_recv() {
                Ok(servers) => {
                    self.resolving = None;
                    self.stun_started = now;
                    if let Some(&s) = servers.first() {
                        for (second, port) in [(false, s.port()), (true, SECOND_STUN_PORT)] {
                            if second && port == s.port() {
                                continue;
                            }
                            self.probes.push(StunProbe {
                                fam: 0,
                                server: SocketAddr::new(s.ip(), port),
                                tid: crate::turn::tid(),
                                sent: None,
                                tries: 0,
                                mapped: None,
                                second,
                            });
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.resolving = None,
            }
        }
        if now.duration_since(self.stun_started) > STUN_BUDGET {
            return;
        }
        for i in 0..self.probes.len() {
            let p = &self.probes[i];
            if p.mapped.is_some() || p.sent.is_some_and(|t| now.duration_since(t) < STUN_RETRY) {
                continue;
            }
            let req = Builder::new(method::BINDING, Class::Request, p.tid).finish();
            if let Some(s) = &self.socks[p.fam] {
                let _ = s.send_to(&req, p.server);
            }
            let p = &mut self.probes[i];
            p.sent = Some(now);
            p.tries += 1;
        }
    }

    fn on_stun(&mut self, raw: &[u8]) {
        let Some(m) = stun::Message::decode(raw) else {
            return;
        };
        if m.method != method::BINDING || m.class != Class::Success {
            return;
        }
        let Some(i) = self.probes.iter().position(|p| p.tid == m.tid) else {
            return;
        };
        let Some(mapped) = m.xor_addr(attr::XOR_MAPPED_ADDRESS) else {
            return;
        };
        if self.probes[i].mapped.is_some() {
            return;
        }
        self.probes[i].mapped = Some(mapped);
        let firsts: Vec<SocketAddr> = self
            .probes
            .iter()
            .filter_map(|p| p.mapped)
            .collect::<Vec<_>>();
        // endpoint-dependent mapping: the two destinations saw different ports
        if self.probes.iter().all(|p| p.mapped.is_some()) && self.probes.len() > 1 {
            let sym = firsts.windows(2).any(|w| w[0] != w[1]);
            if self.symmetric.is_none() {
                log::info!(
                    "direct: NAT mapping is {}",
                    if sym {
                        "endpoint dependent (symmetric)"
                    } else {
                        "endpoint independent"
                    }
                );
            }
            self.symmetric = Some(sym);
        }
        let primary = !self.probes[i].second;
        if primary && !self.local.iter().any(|c| c.addr == mapped) {
            self.local.push(Candidate {
                kind: CandKind::Srflx,
                addr: mapped,
            });
            self.local_gen = self.local_gen.wrapping_add(1);
        }
    }

    /// A signalling payload from the peer (relay `DATA`, after `T_DIRECT`).
    pub(crate) fn on_signal(&mut self, p: &[u8]) {
        match p.first() {
            Some(&SIG_HELLO) if p.len() >= 19 && p[1] == 1 => {
                let flags = p[2];
                let mut nonce = [0u8; 16];
                nonce.copy_from_slice(&p[3..19]);
                if self.peer_hello.is_none() {
                    if flags & HELLO_ALLOWED == 0 {
                        let mut sink = Vec::new();
                        if self.off.is_none() {
                            self.give_up(Off::PeerPrivacy, &mut sink);
                        }
                    }
                    let (n0, n1) = if self.slot == 0 {
                        (self.nonce, nonce)
                    } else {
                        (nonce, self.nonce)
                    };
                    let key = match_key(&n0, &n1, self.session);
                    self.sealer = Some(Sealer::new(&key, self.slot, self.peer));
                }
                // keep the first nonce (the key depends on it), refresh flags
                let nonce = self.peer_hello.map_or(nonce, |(_, n)| n);
                self.peer_hello = Some((flags, nonce));
            }
            Some(&SIG_CANDIDATES) => {
                if !self.both_allowed() || self.off.is_some() {
                    return;
                }
                let Some((generation, cands)) = decode_candidates(&p[1..]) else {
                    return;
                };
                if self.remote_gen == Some(generation) {
                    return;
                }
                self.remote_gen = Some(generation);
                for c in cands {
                    self.add_pair(c);
                }
            }
            Some(&SIG_GIVE_UP) if self.selected.is_none() => {
                let mut sink = Vec::new();
                self.give_up(Off::NoPath, &mut sink);
            }
            _ => {}
        }
    }

    fn add_pair(&mut self, c: Candidate) {
        let fam = fam_of(&c.addr);
        if self.socks[fam].is_none() || self.pairs.iter().any(|p| p.remote.addr == c.addr) {
            return;
        }
        // loopback tests only talk to loopback; real matches never do
        if c.addr.ip().is_loopback() != self.loopback {
            return;
        }
        self.pairs.push(Pair {
            fam,
            remote: c,
            srtt: Srtt::default(),
            inflight: VecDeque::new(),
            last_check: None,
            last_ok: None,
            checks: 0,
        });
    }

    fn send_to(&mut self, fam: usize, to: SocketAddr, kind: u8, body: &[u8]) {
        let (Some(sealer), Some(s)) = (self.sealer.as_mut(), self.socks[fam].as_ref()) else {
            return;
        };
        let d = sealer.seal(kind, body);
        let _ = s.send_to(&d, to);
    }

    /// Connectivity checks: every pair every 50 ms until one works, then
    /// background checks (every 1 s for 10 s, then every 10 s) for RTT and
    /// NAT keep-alive; the selected pair every 200 ms (consent: it stays healthy while idle,
    /// and a dead path shows within the 800 ms health window).
    fn check(&mut self, now: Instant) {
        let since = now.duration_since(self.started);
        for i in 0..self.pairs.len() {
            let p = &self.pairs[i];
            let every = if self.selected == Some(i) {
                Duration::from_millis(200)
            } else if self.selected.is_none() {
                CHECK_EVERY
            } else if since < Duration::from_secs(10) {
                Duration::from_millis(1000)
            } else {
                Duration::from_secs(10)
            };
            if p.last_check.is_some_and(|t| now.duration_since(t) < every) {
                continue;
            }
            // pairs that never answered get quiet after a while
            if p.last_ok.is_none() && p.checks > 200 {
                continue;
            }
            let id = self.next_check_id;
            self.next_check_id += 1;
            let mut body = id.to_be_bytes().to_vec();
            body.extend(self.now_us().to_be_bytes());
            let (fam, to) = (p.fam, p.remote.addr);
            self.send_to(fam, to, D_CHECK, &body);
            let p = &mut self.pairs[i];
            p.last_check = Some(now);
            p.checks += 1;
            p.inflight.push_back((id, now));
            if p.inflight.len() > 32 {
                p.inflight.pop_front();
            }
        }
        self.select(now);
    }

    /// Best pair by round trip, with hysteresis between pairs.
    fn select(&mut self, now: Instant) {
        let best = self
            .pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.ok_recently(now))
            .filter_map(|(i, p)| p.srtt.rtt.map(|r| (r + 2.0 * p.srtt.jitter, i)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((score, i)) = best else {
            return;
        };
        match self.selected {
            None => {
                log::info!(
                    "direct: path to the other player works ({:.0} ms, {} candidate)",
                    self.pairs[i].srtt.rtt.unwrap_or(0.0),
                    match self.pairs[i].remote.kind {
                        CandKind::Host => "host",
                        CandKind::Srflx => "server reflexive",
                        CandKind::Prflx => "peer reflexive",
                    }
                );
                self.selected = Some(i);
            }
            Some(cur) if cur != i => {
                let cp = &self.pairs[cur];
                let cur_score = if cp.ok_recently(now) {
                    cp.srtt.rtt.map_or(f32::MAX, |r| r + 2.0 * cp.srtt.jitter)
                } else {
                    f32::MAX
                };
                if score < cur_score - 10.0f32.max(cur_score * 0.15) {
                    self.selected = Some(i);
                    self.switches += 1;
                }
            }
            _ => {}
        }
    }

    /// Receive datagrams (STUN answers, checks, ggrs traffic) into
    /// [`Self::inbox`] (relay payload format).
    pub(crate) fn poll(&mut self) {
        for fam in 0..2 {
            while let Some(s) = self.socks[fam].as_ref() {
                let (n, from) = match s.recv_from(&mut self.buf) {
                    Ok(r) => r,
                    Err(e) if would_block(&e) => break,
                    Err(e) => {
                        log::debug!("direct recv: {e}");
                        break;
                    }
                };
                let d = self.buf[..n].to_vec();
                if d.first() == Some(&MAGIC) {
                    self.on_datagram(fam, from, &d);
                } else if d.first().is_some_and(|b| b & 0xC0 == 0) {
                    self.on_stun(&d);
                } else {
                    self.forged += 1;
                }
            }
        }
    }

    fn on_datagram(&mut self, fam: usize, from: SocketAddr, d: &[u8]) {
        let Some(sealer) = self.sealer.as_mut() else {
            self.forged += 1;
            return;
        };
        let Some((kind, body)) = sealer.open(d) else {
            self.forged += 1;
            return;
        };
        let body = body.to_vec();
        let now = Instant::now();
        let idx = match self.pairs.iter().position(|p| p.remote.addr == from) {
            Some(i) => Some(i),
            None if self.off.is_none() && self.both_allowed() => {
                // an authentic check from an address we did not know: the
                // peer's NAT mapping towards us (peer reflexive)
                self.add_pair(Candidate {
                    kind: CandKind::Prflx,
                    addr: from,
                });
                self.pairs.iter().position(|p| p.remote.addr == from)
            }
            None => None,
        };
        self.heard = true;
        if idx.is_some() && idx == self.selected {
            self.last_rx = now;
        }
        match kind {
            D_CHECK if body.len() == 16 => {
                self.send_to(fam, from, D_ACK, &body);
            }
            D_ACK if body.len() == 16 => {
                let id = u64::from_be_bytes(body[..8].try_into().unwrap_or_default());
                if let Some(i) = idx {
                    let p = &mut self.pairs[i];
                    if let Some(k) = p.inflight.iter().position(|(c, _)| *c == id) {
                        let (_, sent) = p.inflight.remove(k).unwrap_or((0, now));
                        let ms = now.duration_since(sent).as_secs_f32() * 1000.0;
                        p.srtt.sample(ms);
                        p.last_ok = Some(now);
                        if self.selected.is_none() || self.selected == Some(i) {
                            self.last_rx = now;
                        }
                    }
                }
                self.select(now);
            }
            D_DATA => {
                if self.selected.is_some() {
                    self.last_rx = now;
                }
                crate::batch::unpack(&body, &mut self.inbox);
            }
            _ => {}
        }
    }

    /// Send a relay-format payload to the peer on the selected pair (ggrs
    /// messages coalesced per tick).
    pub(crate) fn send(&mut self, p: &[u8]) {
        if let Some(d) = self.batcher.push(p) {
            self.send_raw(&d);
        }
    }

    pub(crate) fn flush_due(&mut self) {
        if self.batcher.due() {
            self.flush();
        }
    }

    pub(crate) fn flush(&mut self) {
        if let Some(d) = self.batcher.flush() {
            self.send_raw(&d);
        }
    }

    fn send_raw(&mut self, d: &[u8]) {
        let Some(i) = self.selected else {
            return;
        };
        let (fam, to) = (self.pairs[i].fam, self.pairs[i].remote.addr);
        self.send_to(fam, to, D_DATA, d);
    }

    /// Diagnostic for tests: send arbitrary bytes from our socket to the
    /// selected pair (forged / replayed datagrams).
    #[doc(hidden)]
    pub(crate) fn raw_to_peer(&self, d: &[u8]) {
        if let Some(i) = self.selected
            && let Some(s) = self.socks[self.pairs[i].fam].as_ref()
        {
            let _ = s.send_to(d, self.pairs[i].remote.addr);
        }
    }

    /// Diagnostic for tests: stop answering and sending (simulates the
    /// direct path breaking, e.g. a NAT mapping expiring).
    #[doc(hidden)]
    pub(crate) fn sever(&mut self) {
        self.socks = [None, None];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_round_trip_and_reject_junk() {
        let c = vec![
            Candidate {
                kind: CandKind::Host,
                addr: "192.168.1.10:5000".parse().unwrap(),
            },
            Candidate {
                kind: CandKind::Srflx,
                addr: "[2a01:db8::1]:6000".parse().unwrap(),
            },
        ];
        let wire = encode_candidates(3, &c);
        assert_eq!(decode_candidates(&wire), Some((3, c.clone())));
        // truncated / unknown family
        assert_eq!(decode_candidates(&wire[..wire.len() - 1]), None);
        let mut bad = wire.clone();
        bad[3] = 5;
        assert_eq!(decode_candidates(&bad), None);
        // unspecified / port 0 entries are skipped
        let z = encode_candidates(
            1,
            &[Candidate {
                kind: CandKind::Host,
                addr: "0.0.0.0:9".parse().unwrap(),
            }],
        );
        assert_eq!(decode_candidates(&z), Some((1, vec![])));
        assert_eq!(decode_candidates(&[]), None);
    }

    #[test]
    fn both_sides_derive_the_same_key() {
        let (a, b) = ([1u8; 16], [2u8; 16]);
        assert_eq!(match_key(&a, &b, 7), match_key(&a, &b, 7));
        assert_ne!(match_key(&a, &b, 7), match_key(&b, &a, 7));
        assert_ne!(match_key(&a, &b, 7), match_key(&a, &b, 8));
    }

    #[test]
    fn sealed_datagrams_authenticate_and_resist_replay() {
        let key = match_key(&[9; 16], &[8; 16], 1);
        let mut a = Sealer::new(&key, 0, 1);
        let mut b = Sealer::new(&key, 1, 0);
        let d1 = a.seal(D_DATA, b"hello");
        let d2 = a.seal(D_DATA, b"world");
        assert_eq!(b.open(&d2), Some((D_DATA, &b"world"[..])));
        // late but not replayed: accepted once
        assert_eq!(b.open(&d1), Some((D_DATA, &b"hello"[..])));
        assert_eq!(b.open(&d1), None, "replay");
        assert_eq!(b.open(&d2), None, "replay");
        // a flipped bit anywhere fails
        for i in 0..d1.len() {
            let mut f = a.seal(D_DATA, b"hello");
            f[i] ^= 0x10;
            assert_eq!(b.open(&f), None, "byte {i}");
        }
        // another key, the wrong direction, junk
        let mut other = Sealer::new(&match_key(&[1; 16], &[8; 16], 1), 0, 1);
        assert_eq!(b.open(&other.seal(D_DATA, b"x")), None);
        assert_eq!(a.open(&a.clone_for_test().seal(D_DATA, b"x")), None);
        assert_eq!(b.open(&[MAGIC; 4]), None);
        assert_eq!(b.open(&[]), None);
        // far older than the window
        let old = a.seal(D_DATA, b"old");
        for _ in 0..100 {
            let d = a.seal(D_DATA, b"n");
            assert!(b.open(&d).is_some());
        }
        assert_eq!(b.open(&old), None);
    }

    impl Sealer {
        fn clone_for_test(&self) -> Sealer {
            // same key and slot as `self`: what a reflected datagram looks like
            Sealer {
                key: self.key.clone(),
                slot: self.slot,
                peer: self.peer,
                tx: 1000,
                rx_top: 0,
                rx_seen: 0,
                rx_any: false,
            }
        }
    }

    fn pump_pair(a: &mut Direct, b: &mut Direct, until: impl Fn(&Direct, &Direct) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) && !until(a, b) {
            for s in a.pump() {
                b.on_signal(&s[1..]);
            }
            for s in b.pump() {
                a.on_signal(&s[1..]);
            }
            a.poll();
            b.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn loopback(allowed: bool) -> DirectConfig {
        DirectConfig {
            allowed,
            host_only: true,
            loopback: true,
            ..Default::default()
        }
    }

    #[test]
    fn two_peers_punch_through_on_loopback() {
        let mut a = Direct::start(&loopback(true), 42, 0).unwrap();
        let mut b = Direct::start(&loopback(true), 42, 1).unwrap();
        pump_pair(&mut a, &mut b, |a, b| a.healthy() && b.healthy());
        assert!(a.carries(1) && b.carries(0));
        assert!(a.srtt().unwrap().rtt.unwrap() < 50.0);
        // ggrs-format payloads get through, sealed
        a.send(&[crate::relay::T_GGRS + 1, 1, 2, 3]);
        pump_pair(&mut a, &mut b, |_, b| !b.inbox.is_empty());
        assert_eq!(b.inbox.pop_front(), Some(vec![1, 1, 2, 3]));
        // forged datagrams are counted and dropped
        a.raw_to_peer(&[MAGIC, D_DATA, 0, 0, 0, 0, 9, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        a.raw_to_peer(b"\xff garbage");
        pump_pair(&mut a, &mut b, |_, b| b.forged >= 2);
        assert_eq!(b.forged, 2);
        assert!(b.inbox.is_empty());
    }

    #[test]
    fn privacy_opt_out_sends_no_candidates() {
        let mut a = Direct::start(&loopback(true), 42, 0).unwrap();
        let mut b = Direct::start(&loopback(false), 42, 1).unwrap();
        let mut a_sent = Vec::new();
        let mut b_sent = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(600) {
            for s in a.pump() {
                b.on_signal(&s[1..]);
                a_sent.push(s);
            }
            for s in b.pump() {
                a.on_signal(&s[1..]);
                b_sent.push(s);
            }
            a.poll();
            b.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        let cands = |v: &[Vec<u8>]| v.iter().filter(|s| s[1] == SIG_CANDIDATES).count();
        assert_eq!(cands(&a_sent), 0, "the opted-in side waits for consent");
        assert_eq!(cands(&b_sent), 0);
        assert_eq!(a.off(), Some(Off::PeerPrivacy));
        assert_eq!(b.off(), Some(Off::LocalPrivacy));
        assert!(!a.carries(1) && !b.carries(0));
        assert!(b.socks.iter().all(Option::is_none), "no socket when off");
    }
}
