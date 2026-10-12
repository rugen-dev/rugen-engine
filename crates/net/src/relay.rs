//! Match traffic through `rugen-server`'s relay session. ggrs sees the
//! other players as virtual addresses ([`slot_addr`]); every datagram goes
//! to the server tagged with session, slots and our secret token, and the
//! server forwards it.
//!
//! The relay session also carries the signalling of the faster player ↔
//! player paths, as opaque payloads: the direct peer-to-peer path
//! ([`crate::direct`], the default; it reveals the players' addresses to
//! each other unless either opts out) and TURN ([`crate::turn`]). A
//! [`PathSelector`] moves the match to the best working path (direct >
//! TURN > relay by round trip and jitter, with hysteresis) and back to the
//! relay whenever the faster paths go quiet.
//!
//! Transport: UDP to the server's relay port when it answers within a short
//! probe, else the same frames over the `/relay` WebSocket (works through an
//! HTTP-only tunnel such as Cloudflare's).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use ggrs::NonBlockingSocket;
use rugen_proto::SessionTicket;
use rugen_proto::relay::{self as wire, Header, TOKEN_LEN};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::direct::{Direct, DirectConfig};
use crate::path::{LinkInfo, PathKind, PathSample, PathSelector};
use crate::socket::median;
use crate::{Error, Result};

/// Inner payload types of relayed `DATA` frames.
pub(crate) const T_GGRS: u8 = 0;
const T_PING: u8 = 1;
const T_PONG: u8 = 2;
/// Ping answered through the relay whatever path is selected (the relay's
/// round trip measured like the other paths', during the match).
const T_RPING: u8 = 9;
const T_RPONG: u8 = 10;

/// The address ggrs uses for relay slot `slot` (class E, never routed).
pub fn slot_addr(slot: u8) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)), 7000 + slot as u16)
}

fn addr_slot(a: &SocketAddr) -> Option<u8> {
    (a.ip() == IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)) && (7000..7016).contains(&a.port()))
        .then(|| (a.port() - 7000) as u8)
}

enum Transport {
    Udp {
        socket: UdpSocket,
        server: SocketAddr,
    },
    Ws(Box<WebSocket<MaybeTlsStream<TcpStream>>>),
}

pub struct RelaySocket {
    transport: Transport,
    session: u32,
    slot: u8,
    token: [u8; TOKEN_LEN],
    /// Bitmask of slots connected to the session (from ACK / PEERS).
    pub connected: u8,
    clock: Instant,
    last_hello: Instant,
    buf: Vec<u8>,
    /// ggrs messages that arrived before the session existed (connect /
    /// RTT loops), handed to ggrs first: a lost first sync request can stall
    /// a spectator for good.
    early: Vec<(SocketAddr, ggrs::Message)>,
    /// Player ↔ player path through TURN (see [`crate::turn`]), when the
    /// ticket carries a `turn` transport.
    turn: Option<crate::turn::Overlay>,
    /// Direct peer-to-peer path ([`RelaySocket::enable_direct`]).
    direct: Option<Direct>,
    selector: PathSelector,
    /// Player ↔ player round trip through the server relay (ms).
    relay_rtt: Option<f32>,
    relay_srtt: crate::path::Srtt,
    rpongs: u32,
    last_rping: Instant,
    last_select: Instant,
    /// What the HUD shows; stays readable after the socket moved into ggrs.
    link: std::sync::Arc<std::sync::Mutex<LinkInfo>>,
    hooks: std::sync::Arc<Hooks>,
}

/// Test hooks that still reach the socket after it moved into a ggrs
/// session: break the direct path, inject forged datagrams.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Hooks {
    pub sever_direct: std::sync::atomic::AtomicBool,
    pub forge: std::sync::atomic::AtomicU32,
}

fn rz(msg: impl Into<String>) -> Error {
    Error::Rendezvous(msg.into())
}

fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
    )
}

/// The UDP relay address to use: a server bound to 0.0.0.0 without a
/// configured public address advertises `0.0.0.0:port`, meaning "my HTTP
/// host, this port".
pub fn udp_target(advertised: &str, server_base: &str) -> String {
    match advertised.parse::<SocketAddr>() {
        Ok(a) if a.ip().is_unspecified() => {
            format!("{}:{}", crate::online::url_host(server_base), a.port())
        }
        _ => advertised.to_string(),
    }
}

/// `https://host` → `wss://host/relay`.
pub fn relay_ws_url(server_base: &str) -> String {
    let base = server_base.trim_end_matches('/');
    let ws = if let Some(r) = base.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = base.strip_prefix("http://") {
        format!("ws://{r}")
    } else {
        format!("ws://{base}")
    };
    format!("{ws}/relay")
}

impl RelaySocket {
    fn new(transport: Transport, t: &SessionTicket) -> Result<Self> {
        let token: [u8; TOKEN_LEN] = rugen_proto::from_hex(&t.token)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| rz("bad session token"))?;
        Ok(Self {
            transport,
            session: t.session,
            slot: t.slot,
            token,
            connected: 0,
            clock: Instant::now(),
            last_hello: Instant::now() - Duration::from_secs(10),
            buf: vec![0; 2048],
            early: Vec::new(),
            turn: crate::turn::Overlay::from_ticket(t),
            direct: None,
            selector: PathSelector::default(),
            relay_rtt: None,
            relay_srtt: Default::default(),
            rpongs: 0,
            last_rping: Instant::now(),
            last_select: Instant::now(),
            link: Default::default(),
            hooks: Default::default(),
        })
    }

    /// Join the relay session of `ticket` with the server's transports in
    /// its order of preference (`ticket.transports`; older servers: UDP
    /// from `ticket.udp`, then the WebSocket). UDP is tried only when the
    /// server offers it and `udp_probe` is not zero: a server without UDP
    /// (tunnel-only) goes straight to the WebSocket, no probe delay.
    pub fn connect(
        ticket: &SessionTicket,
        server_base: &str,
        udp_probe: Duration,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Self> {
        let plan = if ticket.transports.is_empty() {
            rugen_proto::relay_transports(ticket.udp.as_deref())
        } else {
            ticket.transports.clone()
        };
        let mut last = rz("the server offers no relay transport this game knows");
        for t in plan {
            match t.kind.as_str() {
                "udp" if !udp_probe.is_zero() => {
                    let udp = udp_target(&t.addr, server_base);
                    match Self::connect_udp(ticket, &udp, udp_probe) {
                        Ok(s) => {
                            progress("relay: UDP");
                            return Ok(s);
                        }
                        Err(e) => {
                            progress(&format!(
                                "relay: UDP unavailable ({e}), trying the next transport"
                            ));
                            last = e;
                        }
                    }
                }
                "ws" => {
                    let url = if t.addr.starts_with("ws://") || t.addr.starts_with("wss://") {
                        t.addr.clone()
                    } else {
                        relay_ws_url(server_base)
                    };
                    match Self::connect_ws(ticket, &url) {
                        Ok(s) => {
                            progress("relay: WebSocket");
                            return Ok(s);
                        }
                        Err(e) => last = e,
                    }
                }
                _ => {}
            }
        }
        Err(last)
    }

    pub fn connect_udp(ticket: &SessionTicket, server: &str, probe: Duration) -> Result<Self> {
        use std::net::ToSocketAddrs;
        let server = server
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.find(SocketAddr::is_ipv4))
            .ok_or_else(|| rz(format!("cannot resolve {server}")))?;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let mut s = Self::new(Transport::Udp { socket, server }, ticket)?;
        s.hello_until_ack(probe)?;
        Ok(s)
    }

    pub fn connect_ws(ticket: &SessionTicket, url: &str) -> Result<Self> {
        let (ws, _) = tungstenite::connect(url).map_err(|e| rz(format!("relay websocket: {e}")))?;
        let tcp: Option<&TcpStream> = match ws.get_ref() {
            MaybeTlsStream::Plain(t) => Some(t),
            MaybeTlsStream::Rustls(t) => Some(t.get_ref()),
            _ => None,
        };
        if let Some(t) = tcp {
            t.set_nonblocking(true)?;
            let _ = t.set_nodelay(true);
        }
        let mut s = Self::new(Transport::Ws(Box::new(ws)), ticket)?;
        s.hello_until_ack(Duration::from_secs(5))?;
        Ok(s)
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// Start the direct peer-to-peer path (players only): gathers
    /// candidates at once and exchanges them through this session while
    /// the other player connects. Call right after connecting.
    pub fn enable_direct(&mut self, cfg: &DirectConfig) {
        if self.direct.is_none() {
            self.direct = Direct::start(cfg, self.session, self.slot);
        }
    }

    /// The path match traffic to the other player takes now.
    pub fn path(&self) -> PathKind {
        self.selector.current()
    }

    /// Transport name for logs: `direct`, the TURN transport, or the
    /// relay's (`UDP` / `WebSocket`).
    pub fn transport_name(&self) -> &'static str {
        match self.selector.current() {
            PathKind::Direct => return "direct",
            PathKind::Turn => {
                if let Some(n) = self.turn.as_ref().and_then(|t| t.name()) {
                    return n;
                }
            }
            PathKind::Relay => {}
        }
        self.relay_name()
    }

    fn relay_name(&self) -> &'static str {
        match self.transport {
            Transport::Udp { .. } => "UDP",
            Transport::Ws(_) => "WebSocket",
        }
    }

    /// Live path information (kind, round trip), shared with the HUD.
    pub fn link(&self) -> std::sync::Arc<std::sync::Mutex<LinkInfo>> {
        self.link.clone()
    }

    /// Why the direct path is not used (when known).
    pub fn direct_off(&self) -> Option<crate::direct::Off> {
        self.direct.as_ref().and_then(|d| d.off())
    }

    #[doc(hidden)]
    pub fn hooks(&self) -> std::sync::Arc<Hooks> {
        self.hooks.clone()
    }

    fn apply_hooks(&mut self) {
        use std::sync::atomic::Ordering;
        if self.hooks.sever_direct.swap(false, Ordering::Relaxed) {
            self.sever_direct();
        }
        let n = self.hooks.forge.swap(0, Ordering::Relaxed);
        for i in 0..n {
            // right magic and slot, wrong tag; a replayed-looking header
            let mut d = vec![0xD7, 3, self.slot, 0, 0, 0, i as u8];
            d.extend([0x5A; 24]);
            self.direct_raw(&d);
        }
    }

    /// Diagnostic (tests): break the direct path as a NAT timeout would.
    #[doc(hidden)]
    pub fn sever_direct(&mut self) {
        if let Some(d) = self.direct.as_mut() {
            d.sever();
        }
    }

    /// Diagnostic (tests): send raw bytes to the peer's direct address.
    #[doc(hidden)]
    pub fn direct_raw(&self, d: &[u8]) {
        if let Some(x) = self.direct.as_ref() {
            x.raw_to_peer(d);
        }
    }

    /// Kind of candidate the direct path runs on (host = same network).
    pub fn direct_candidate(&self) -> Option<crate::direct::CandKind> {
        self.direct.as_ref().and_then(|d| d.selected_kind())
    }

    /// Our NAT maps per destination (symmetric), when STUN told us.
    pub fn nat_symmetric(&self) -> Option<bool> {
        self.direct.as_ref().and_then(|d| d.symmetric())
    }

    /// Datagrams the direct path dropped as forged or replayed.
    pub fn direct_forged(&self) -> u64 {
        self.direct.as_ref().map_or(0, |d| d.forged)
    }

    fn samples(&self) -> Vec<PathSample> {
        let mut v = Vec::with_capacity(3);
        if let Some(d) = &self.direct
            && d.carries(1 - self.slot.min(1))
            && let Some(r) = d.srtt()
        {
            v.push(PathSample {
                kind: PathKind::Direct,
                rtt_ms: r.rtt,
                jitter_ms: r.jitter,
                healthy: d.healthy(),
            });
        }
        if let Some(t) = &self.turn
            && t.name().is_some()
        {
            let r = t.srtt();
            v.push(PathSample {
                kind: PathKind::Turn,
                rtt_ms: r.rtt.or(t.rtt().map(|x| x as f32)),
                jitter_ms: r.jitter,
                healthy: t.usable(),
            });
        }
        v.push(PathSample {
            kind: PathKind::Relay,
            rtt_ms: Some(self.relay_srtt.rtt.or(self.relay_rtt).unwrap_or(600.0)),
            jitter_ms: 0.0,
            healthy: true,
        });
        v
    }

    /// Re-evaluate the path (a few times a second) and publish it.
    fn update_path(&mut self, initial: bool) {
        let now = Instant::now();
        if !initial && now.duration_since(self.last_select) < Duration::from_millis(100) {
            return;
        }
        self.last_select = now;
        let samples = self.samples();
        let kind = if initial {
            self.selector.choose(&samples)
        } else {
            self.selector.update(now, &samples)
        };
        let rtt = samples
            .iter()
            .find(|s| s.kind == kind)
            .and_then(|s| s.rtt_ms)
            .unwrap_or(0.0);
        let transport = self.transport_name();
        let forged = self.direct_forged();
        if let Ok(mut l) = self.link.lock() {
            l.kind = kind;
            l.transport = transport;
            l.rtt_ms = (rtt + 0.5) as u32;
            l.switches = self.selector.switches;
            l.forged = forged;
        }
    }

    fn send_frame(&mut self, kind: u8, to: u8, payload: &[u8]) {
        let h = Header {
            kind,
            from: self.slot,
            to,
            session: self.session,
        };
        let bytes = wire::encode_client(h, &self.token, payload);
        match &mut self.transport {
            Transport::Udp { socket, server } => {
                let _ = socket.send_to(&bytes, *server);
            }
            Transport::Ws(ws) => {
                // WouldBlock means "queued"; `flush` in `recv_frame` finishes it
                let _ = ws.send(Message::Binary(bytes.into()));
            }
        }
    }

    /// Next frame from the server (or from the peer over the direct path
    /// or TURN, presented as a relayed `DATA` frame), non-blocking.
    fn recv_frame(&mut self) -> Option<(Header, Vec<u8>)> {
        if let Some(f) = self.direct_frame() {
            return Some(f);
        }
        if let Some(f) = self.turn_frame() {
            return Some(f);
        }
        loop {
            let raw: Vec<u8> = match &mut self.transport {
                Transport::Udp { socket, server } => match socket.recv_from(&mut self.buf) {
                    Ok((n, from)) if from == *server => self.buf[..n].to_vec(),
                    Ok(_) => continue,
                    Err(e) if would_block(&e) => return None,
                    Err(e) => {
                        log::warn!("relay recv: {e}");
                        return None;
                    }
                },
                Transport::Ws(ws) => {
                    let _ = ws.flush();
                    match ws.read() {
                        Ok(Message::Binary(b)) => b.to_vec(),
                        Ok(_) => continue,
                        Err(tungstenite::Error::Io(e)) if would_block(&e) => return None,
                        Err(e) => {
                            log::debug!("relay websocket: {e}");
                            return None;
                        }
                    }
                }
            };
            let Some((h, p)) = wire::decode_server(&raw) else {
                continue;
            };
            if h.session != self.session {
                continue;
            }
            match h.kind {
                wire::ACK | wire::PEERS => {
                    if let Some(&m) = p.first() {
                        self.connected = m;
                    }
                }
                _ => {}
            }
            return Some((h, p.to_vec()));
        }
    }

    fn hello_until_ack(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let mut last = start - Duration::from_secs(1);
        while start.elapsed() < timeout {
            if last.elapsed() >= Duration::from_millis(200) {
                self.send_frame(wire::HELLO, 0, &[]);
                last = Instant::now();
            }
            while let Some((h, _)) = self.recv_frame() {
                if h.kind == wire::ACK {
                    self.last_hello = Instant::now();
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(rz("the relay did not answer"))
    }

    /// Wait until every slot in `mask` is connected to the session.
    pub fn wait_for(
        &mut self,
        mask: u8,
        timeout: Duration,
        progress: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let start = Instant::now();
        progress("waiting for the other player to reach the server");
        while start.elapsed() < timeout {
            if self.connected & mask == mask {
                return Ok(());
            }
            if self.last_hello.elapsed() >= Duration::from_millis(500) {
                self.send_frame(wire::HELLO, 0, &[]);
                self.last_hello = Instant::now();
            }
            while let Some((h, p)) = self.recv_frame() {
                self.answer(h, &p);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(rz(format!(
            "the other player did not connect within {} s",
            timeout.as_secs()
        )))
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    fn now_us(&self) -> u64 {
        self.clock.elapsed().as_micros() as u64
    }

    /// Answer pings from other slots; returns a pong's round trip.
    fn answer(&mut self, h: Header, p: &[u8]) -> Option<u32> {
        if h.kind != wire::DATA || p.is_empty() {
            return None;
        }
        match p[0] {
            T_GGRS => {
                if self.early.len() < 256
                    && let Ok(msg) = bincode::deserialize(&p[1..])
                {
                    self.early.push((slot_addr(h.from), msg));
                }
                None
            }
            T_PING => {
                let mut pong = p.to_vec();
                pong[0] = T_PONG;
                self.send_payload(h.from, &pong);
                None
            }
            crate::turn::T_TURN => {
                if let Some(t) = self.turn.as_mut()
                    && h.from == t.peer
                {
                    t.on_signal(&p[1..]);
                }
                None
            }
            crate::direct::T_DIRECT => {
                if let Some(d) = self.direct.as_mut()
                    && h.from == d.peer
                {
                    d.on_signal(&p[1..]);
                }
                None
            }
            T_RPING if p.len() == 9 => {
                let mut pong = p.to_vec();
                pong[0] = T_RPONG;
                self.send_frame(wire::DATA, h.from, &pong);
                None
            }
            T_RPONG if p.len() == 9 => {
                let t = u64::from_be_bytes(p[1..9].try_into().ok()?);
                let us = self.now_us().saturating_sub(t);
                self.relay_srtt.sample(us as f32 / 1000.0);
                self.rpongs += 1;
                None
            }
            T_PONG if p.len() == 9 => {
                // our own clock in µs (the peer only echoes it)
                let t = u64::from_be_bytes(p[1..9].try_into().ok()?);
                Some(self.now_us().saturating_sub(t) as u32)
            }
            _ => None,
        }
    }

    /// Median round trip to the player in `peer` through the server
    /// (that player answers automatically while connecting or playing).
    pub fn peer_rtt(&mut self, peer: u8, samples: usize, timeout: Duration) -> Option<u32> {
        let mut got = self.peer_rtt_samples(peer, samples, timeout);
        median(&mut got).map(|us| (us + 500) / 1000)
    }

    /// Round-trip samples to `peer` in microseconds (pings every 40 ms
    /// over the path match traffic takes: TURN once it is up).
    pub fn peer_rtt_samples(&mut self, peer: u8, samples: usize, timeout: Duration) -> Vec<u32> {
        let start = Instant::now();
        let mut last = start - Duration::from_secs(1);
        let mut got = Vec::new();
        while start.elapsed() < timeout && got.len() < samples {
            if last.elapsed() >= Duration::from_millis(40) {
                let mut p = vec![T_PING];
                p.extend(self.now_us().to_be_bytes());
                self.send_payload(peer, &p);
                last = Instant::now();
            }
            while let Some((h, p)) = self.recv_frame() {
                // only pings sent by this call: late answers to earlier pings
                // (e.g. through the slow relay) would skew the sample
                if let Some(rtt) = self.answer(h, &p)
                    && h.from == peer
                    && u128::from(rtt) <= start.elapsed().as_micros()
                {
                    got.push(rtt);
                }
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        got
    }

    /// Median round trip to the server itself.
    pub fn server_rtt(&mut self, samples: usize, timeout: Duration) -> Option<u32> {
        let start = Instant::now();
        let mut last = start - Duration::from_secs(1);
        let mut got = Vec::new();
        while start.elapsed() < timeout && got.len() < samples {
            if last.elapsed() >= Duration::from_millis(40) {
                let t = self.now_ms().to_be_bytes();
                self.send_frame(wire::PING, 0, &t);
                last = Instant::now();
            }
            while let Some((h, p)) = self.recv_frame() {
                if h.kind == wire::PONG
                    && let Ok(b) = <[u8; 8]>::try_from(p.as_slice())
                {
                    got.push(self.now_ms().saturating_sub(u64::from_be_bytes(b)) as u32);
                } else {
                    self.answer(h, &p);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        median(&mut got)
    }

    // ---- TURN overlay (crate::turn)

    /// Send a `DATA` payload to slot `to` on the selected path; while that
    /// path is quiet, also on the next one down (direct, TURN, relay), so
    /// nothing waits for a dead path.
    fn send_payload(&mut self, to: u8, p: &[u8]) {
        let path = self.selector.current();
        if path == PathKind::Direct
            && let Some(d) = self.direct.as_mut()
            && d.carries(to)
        {
            d.send(p);
            if d.healthy() {
                return;
            }
        }
        if path <= PathKind::Turn
            && let Some(t) = self.turn.as_mut()
            && t.carries(to)
        {
            t.send(p);
            if t.healthy() || (path == PathKind::Direct && t.usable()) {
                return;
            }
        }
        self.send_frame(wire::DATA, to, p);
    }

    /// Advance the direct path (signalling goes through the server) and
    /// return the next payload the peer sent on it.
    fn direct_frame(&mut self) -> Option<(Header, Vec<u8>)> {
        let d = self.direct.as_mut()?;
        if d.inbox.is_empty() {
            let signals = d.pump();
            d.poll();
            let peer = d.peer;
            for s in signals {
                self.send_frame(wire::DATA, peer, &s);
            }
        }
        let d = self.direct.as_mut()?;
        let p = d.inbox.pop_front()?;
        let h = Header {
            kind: wire::DATA,
            from: d.peer,
            to: self.slot,
            session: self.session,
        };
        Some((h, p))
    }

    /// Advance the TURN setup (signalling goes through the server) and
    /// return the next payload the peer sent over TURN.
    fn turn_frame(&mut self) -> Option<(Header, Vec<u8>)> {
        let t = self.turn.as_mut()?;
        if t.inbox.is_empty() {
            let signals = t.pump();
            t.poll();
            let peer = t.peer;
            for s in signals {
                self.send_frame(wire::DATA, peer, &s);
            }
        }
        let t = self.turn.as_mut()?;
        let p = t.inbox.pop_front()?;
        let h = Header {
            kind: wire::DATA,
            from: t.peer,
            to: self.slot,
            session: self.session,
        };
        Some((h, p))
    }

    /// Wait (at most `timeout`) until the player ↔ player TURN path works
    /// or is ruled out; true when match traffic now goes over TURN. Call
    /// after [`wait_for`](Self::wait_for). Without TURN credentials in the
    /// ticket this returns false at once. The overlay keeps trying in the
    /// background after the timeout and switches over mid-match.
    pub fn settle_turn(&mut self, timeout: Duration) -> bool {
        self.settle_paths(timeout);
        self.turn.as_ref().is_some_and(|t| t.name().is_some())
    }

    /// Match start: wait (at most `timeout`) for the direct and TURN paths
    /// to work or be ruled out, then pick the fastest working path. A
    /// working direct path ends the wait at once (it is preferred; a
    /// faster TURN path, rare, still takes over later with hysteresis).
    /// The paths keep trying in the background and the match switches
    /// mid-way when one comes up. Call after [`wait_for`](Self::wait_for).
    pub fn settle_paths(&mut self, timeout: Duration) -> PathKind {
        let start = Instant::now();
        // the relay round trip, for comparison (the peer answers pings)
        if self.relay_rtt.is_none() {
            // pings the peer answers through the relay whatever path it uses
            let mut last = start - Duration::from_secs(1);
            let before = self.rpongs;
            while self.rpongs < before + 3 && start.elapsed() < Duration::from_millis(800) {
                if last.elapsed() >= Duration::from_millis(40) {
                    let mut p = vec![T_RPING];
                    p.extend(self.now_us().to_be_bytes());
                    let peer = 1 - self.slot.min(1);
                    self.send_frame(wire::DATA, peer, &p);
                    last = Instant::now();
                }
                while let Some((h, p)) = self.recv_frame() {
                    self.answer(h, &p);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            self.relay_rtt = self.relay_srtt.rtt;
        }
        while start.elapsed() < timeout {
            let direct_done = self.direct.as_ref().is_none_or(|d| !d.pending());
            let direct_up = self.direct.as_ref().is_some_and(|d| d.healthy());
            let turn_done = self.turn.as_ref().is_none_or(|t| !t.pending());
            if direct_up || (direct_done && turn_done) {
                break;
            }
            while let Some((h, p)) = self.recv_frame() {
                self.answer(h, &p);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.update_path(true);
        // measure the relay again right away under match conditions (the
        // other paths' round trips include the per-tick polling too)
        self.last_rping = Instant::now() - Duration::from_secs(3);
        self.selector.current()
    }

    /// Bytes this side moved through TURN so far.
    pub fn turn_stats(&self) -> Option<crate::turn::TurnStats> {
        self.turn.as_ref().map(|t| t.stats())
    }

    /// Live TURN byte counters that stay readable after the socket moved
    /// into a ggrs session.
    pub fn turn_meter(&self) -> Option<std::sync::Arc<std::sync::Mutex<crate::turn::TurnStats>>> {
        self.turn.as_ref().map(|t| t.meter())
    }

    /// Diagnostic: send ggrs messages over TURN one per datagram in the
    /// plain encoding (what the coalescing / compact encoding saves).
    #[doc(hidden)]
    pub fn set_turn_plain(&mut self, plain: bool) {
        if let Some(t) = self.turn.as_mut() {
            t.set_plain(plain);
        }
    }

    /// Round trip to the peer measured over TURN during setup.
    pub fn turn_rtt(&self) -> Option<u32> {
        self.turn.as_ref().and_then(|t| t.rtt())
    }

    /// HELLO payload: TURN bytes received / sent (wire estimate, u64 BE
    /// each); empty without TURN (older servers ignore hello payloads).
    fn turn_usage_payload(&self) -> Vec<u8> {
        match self.turn_stats() {
            Some(s) if s.tx_packets > 0 => {
                let mut p = s.rx_wire.to_be_bytes().to_vec();
                p.extend(s.tx_wire.to_be_bytes());
                p
            }
            _ => Vec::new(),
        }
    }

    pub fn bye(&mut self) {
        // release the TURN allocation at once (stops any metering)
        self.turn = None;
        self.direct = None;
        self.send_frame(wire::BYE, 0, &[]);
        if let Transport::Ws(ws) = &mut self.transport {
            let _ = ws.flush();
        }
    }
}

impl Drop for RelaySocket {
    fn drop(&mut self) {
        self.bye();
    }
}

impl NonBlockingSocket<SocketAddr> for RelaySocket {
    fn send_to(&mut self, msg: &ggrs::Message, addr: &SocketAddr) {
        let Some(to) = addr_slot(addr) else {
            return;
        };
        if let Ok(buf) = bincode::serialize(msg) {
            let mut p = Vec::with_capacity(buf.len() + 1);
            p.push(T_GGRS);
            p.extend_from_slice(&buf);
            self.send_payload(to, &p);
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, ggrs::Message)> {
        // keep the UDP mapping / WebSocket alive even when ggrs is quiet;
        // with TURN in use the hello carries our TURN byte counters so the
        // server can enforce its budget. The relay session must stay open
        // while the match runs on the direct path or TURN (signalling,
        // fallback): the server expires sessions idle for 5 s, so hello
        // every 2 s; BYE only when the socket is dropped (match over).
        if self.last_hello.elapsed() >= Duration::from_secs(2) {
            let usage = self.turn_usage_payload();
            self.send_frame(wire::HELLO, 0, &usage);
            self.last_hello = Instant::now();
        }
        let mut out = std::mem::take(&mut self.early);
        if let Some(d) = self.direct.as_mut()
            && d.inbox.is_empty()
        {
            // flush ggrs messages held for coalescing even when no input
            // went out this tick
            d.flush_due();
        }
        self.apply_hooks();
        // the relay's round trip every 3 s (players only), for comparison
        if self.slot < 2 && self.last_rping.elapsed() >= Duration::from_secs(3) {
            let mut p = vec![T_RPING];
            p.extend(self.now_us().to_be_bytes());
            self.send_frame(wire::DATA, 1 - self.slot, &p);
            self.last_rping = Instant::now();
        }
        self.update_path(false);
        while let Some((h, p)) = self.recv_frame() {
            if h.kind != wire::DATA || p.is_empty() {
                continue;
            }
            if p[0] == T_GGRS {
                if let Ok(msg) = bincode::deserialize(&p[1..]) {
                    out.push((slot_addr(h.from), msg));
                }
            } else {
                self.answer(h, &p);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_addresses() {
        assert_eq!(addr_slot(&slot_addr(0)), Some(0));
        assert_eq!(addr_slot(&slot_addr(3)), Some(3));
        assert_eq!(addr_slot(&"127.0.0.1:7000".parse().unwrap()), None);
        assert_eq!(
            relay_ws_url("https://r.example.com/"),
            "wss://r.example.com/relay"
        );
        assert_eq!(
            relay_ws_url("http://1.2.3.4:8080"),
            "ws://1.2.3.4:8080/relay"
        );
    }

    /// A minimal relay on loopback UDP: forwards DATA by slot, answers HELLO.
    fn fake_relay(session: u32, tokens: [[u8; 16]; 2]) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = s.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut routes: [Option<SocketAddr>; 2] = [None, None];
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = s.recv_from(&mut buf) else {
                    return;
                };
                let Some((h, t, p)) = wire::decode_client(&buf[..n]) else {
                    continue;
                };
                if h.session != session || t != tokens[h.from as usize] {
                    continue;
                }
                routes[h.from as usize] = Some(from);
                let mask = routes
                    .iter()
                    .enumerate()
                    .fold(0u8, |m, (i, r)| m | (u8::from(r.is_some()) << i));
                let reply = |kind, p: &[u8]| {
                    wire::encode_server(
                        Header {
                            kind,
                            from: 0xFF,
                            to: h.from,
                            session,
                        },
                        p,
                    )
                };
                match h.kind {
                    wire::HELLO => {
                        let _ = s.send_to(&reply(wire::ACK, &[mask]), from);
                        if let Some(o) = routes[1 - h.from as usize] {
                            let peers = wire::encode_server(
                                Header {
                                    kind: wire::PEERS,
                                    from: 0xFF,
                                    to: 0,
                                    session,
                                },
                                &[mask],
                            );
                            let _ = s.send_to(&peers, o);
                        }
                    }
                    wire::PING => {
                        let _ = s.send_to(&reply(wire::PONG, p), from);
                    }
                    wire::DATA => {
                        if let Some(to) = routes[h.to as usize] {
                            let _ = s.send_to(&wire::encode_server(h, p), to);
                        }
                    }
                    _ => {}
                }
            }
        });
        addr
    }

    #[test]
    fn two_clients_meet_and_measure_rtt_through_the_relay() {
        let tokens = [[1u8; 16], [2u8; 16]];
        let server = fake_relay(77, tokens);
        let ticket = move |slot: u8| SessionTicket {
            session: 77,
            slot,
            token: rugen_proto::to_hex(&tokens[slot as usize]),
            spectators: 0,
            udp: Some(server.to_string()),
            transports: Vec::new(),
        };
        let mut a = RelaySocket::connect(
            &ticket(0),
            "http://unused",
            Duration::from_secs(2),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(a.transport_name(), "UDP");
        let t = std::thread::spawn(move || {
            let mut b =
                RelaySocket::connect_udp(&ticket(1), &server.to_string(), Duration::from_secs(2))
                    .unwrap();
            b.wait_for(0b11, Duration::from_secs(5), &mut |_| {})
                .unwrap();
            let rtt = b.peer_rtt(0, 5, Duration::from_secs(3));
            // keep answering a's pings for a moment
            let end = Instant::now() + Duration::from_millis(500);
            while Instant::now() < end {
                let _ = b.receive_all_messages();
                std::thread::sleep(Duration::from_millis(5));
            }
            rtt
        });
        a.wait_for(0b11, Duration::from_secs(5), &mut |_| {})
            .unwrap();
        let rtt_a = a.peer_rtt(1, 5, Duration::from_secs(3));
        assert!(rtt_a.is_some_and(|r| r < 200));
        assert!(a.server_rtt(3, Duration::from_secs(2)).is_some());
        assert!(t.join().unwrap().is_some());
    }

    #[test]
    fn bad_tickets_are_refused() {
        let t = SessionTicket {
            token: "xyz".into(),
            ..Default::default()
        };
        assert!(RelaySocket::connect_udp(&t, "127.0.0.1:9", Duration::from_millis(100)).is_err());
    }
}
