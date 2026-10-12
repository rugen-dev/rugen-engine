//! TURN (RFC 8656) client and the match overlay that carries ggrs traffic
//! between the two players through TURN relays (Cloudflare Realtime TURN
//! on the official server) instead of through `rugen-server`.
//!
//! Privacy: both players always relay. Each one allocates a relayed address
//! on the TURN service and tells the other only that address (a TURN
//! server's address, never a player's), over the authenticated
//! `rugen-server` relay session. Permissions and channels are installed for
//! the peer's relayed address exactly as the peer's TURN server reported it
//! (XOR-RELAYED-ADDRESS is the address other hosts see).
//!
//! Cost: every byte through a commercial TURN service is metered, so the
//! overlay keeps traffic minimal: ChannelData only (4-byte header, no Send
//! indications), all ggrs messages of a tick coalesced into one datagram in
//! a compact varint encoding, redundant input acks / keep-alives dropped,
//! refreshes only shortly before the allocation / permission expires, and
//! the allocation is released (Refresh lifetime 0) as soon as the socket is
//! dropped.
//!
//! Transports: `turn:` over UDP first, then `turn:…?transport=tcp`, then
//! `turns:` (TLS). The relay leg is always UDP.

use std::collections::VecDeque;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bincode::Options as _;

use crate::stun::{self, Builder, Class, Message, attr, method};

/// Credentials and URLs of one TURN service (a `turn` ticket transport).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TurnCredentials {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

// never print the secret
impl std::fmt::Display for TurnCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} url(s)", self.urls.len())
    }
}

impl std::fmt::Debug for TurnCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnCredentials")
            .field("urls", &self.urls)
            .field("username", &"<redacted>")
            .field("credential", &"<redacted>")
            .finish()
    }
}

/// How the client reaches the TURN server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Proto {
    Udp,
    Tcp,
    Tls,
}

impl Proto {
    pub fn name(self) -> &'static str {
        match self {
            Proto::Udp => "TURN/UDP",
            Proto::Tcp => "TURN/TCP",
            Proto::Tls => "TURN/TLS",
        }
    }

    /// IP + transport header bytes per datagram / segment (for byte
    /// accounting; TCP/TLS rounded up generously).
    fn overhead(self) -> u64 {
        match self {
            Proto::Udp => 28,
            Proto::Tcp => 40,
            Proto::Tls => 69,
        }
    }
}

/// A parsed `turn:` / `turns:` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnUrl {
    pub proto: Proto,
    pub host: String,
    pub port: u16,
}

impl TurnUrl {
    pub fn parse(url: &str) -> Option<Self> {
        let (secure, rest) = if let Some(r) = url.strip_prefix("turns:") {
            (true, r)
        } else {
            (false, url.strip_prefix("turn:")?)
        };
        let (hostport, query) = rest.split_once('?').unwrap_or((rest, ""));
        let tcp = query
            .split('&')
            .any(|kv| kv.eq_ignore_ascii_case("transport=tcp"));
        let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
            let (h, p) = h.split_once(']')?;
            (h, p.strip_prefix(':'))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (hostport, None),
            }
        };
        if host.is_empty() {
            return None;
        }
        let proto = match (secure, tcp) {
            (true, _) => Proto::Tls,
            (false, true) => Proto::Tcp,
            (false, false) => Proto::Udp,
        };
        let port = match port {
            Some(p) => p.parse().ok()?,
            None if secure => 5349,
            None => 3478,
        };
        Some(Self {
            proto,
            host: host.to_string(),
            port,
        })
    }

    /// URLs in the order to try: UDP, then TCP, then TLS (stable within a
    /// kind, so the server's order breaks ties). Port 53 is skipped
    /// (often intercepted).
    pub fn plan(urls: &[String]) -> Vec<Self> {
        let mut v: Vec<Self> = urls
            .iter()
            .filter_map(|u| Self::parse(u))
            .filter(|u| u.port != 53)
            .collect();
        v.sort_by_key(|u| u.proto);
        v.dedup();
        v
    }
}

fn would_block(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn tid() -> [u8; 12] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let s = RandomState::new();
    let n = N.fetch_add(1, Ordering::Relaxed);
    let a = s.hash_one((n, Instant::now()));
    let b = s.hash_one((a, n ^ 0x9E37_79B9_7F4A_7C15));
    let mut t = [0u8; 12];
    t[..8].copy_from_slice(&a.to_le_bytes());
    t[8..].copy_from_slice(&b.to_le_bytes()[..4]);
    t
}

/// The connection to the TURN server.
enum Link {
    Udp(UdpSocket),
    Stream {
        tcp: TcpStream,
        tls: Option<Box<rustls::ClientConnection>>,
        rbuf: Vec<u8>,
        wbuf: Vec<u8>,
    },
}

impl Link {
    fn resolve(u: &TurnUrl) -> io::Result<SocketAddr> {
        let mut addrs: Vec<SocketAddr> = (u.host.as_str(), u.port).to_socket_addrs()?.collect();
        addrs.sort_by_key(|a| !a.is_ipv4());
        addrs
            .first()
            .copied()
            .ok_or_else(|| err(format!("cannot resolve {}", u.host)))
    }

    fn open(u: &TurnUrl, deadline: Instant) -> io::Result<Self> {
        let server = Self::resolve(u)?;
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| err("timed out"))?;
        match u.proto {
            Proto::Udp => {
                let bind = if server.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                let s = UdpSocket::bind(bind)?;
                s.connect(server)?;
                s.set_nonblocking(true)?;
                Ok(Link::Udp(s))
            }
            Proto::Tcp | Proto::Tls => {
                let tcp = TcpStream::connect_timeout(&server, left)?;
                let _ = tcp.set_nodelay(true);
                let tls = if u.proto == Proto::Tls {
                    let mut c = tls_client(&u.host)?;
                    tcp.set_read_timeout(Some(left))?;
                    tcp.set_write_timeout(Some(left))?;
                    let mut t = &tcp;
                    while c.is_handshaking() {
                        c.complete_io(&mut t)?;
                        if Instant::now() > deadline {
                            return Err(err("TLS handshake timed out"));
                        }
                    }
                    Some(Box::new(c))
                } else {
                    None
                };
                tcp.set_nonblocking(true)?;
                Ok(Link::Stream {
                    tcp,
                    tls,
                    rbuf: Vec::new(),
                    wbuf: Vec::new(),
                })
            }
        }
    }

    fn is_stream(&self) -> bool {
        matches!(self, Link::Stream { .. })
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Link::Stream { tcp, tls, wbuf, .. } = self {
            match tls {
                Some(c) => {
                    while c.wants_write() {
                        match c.write_tls(tcp) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(e) if would_block(&e) => break,
                            Err(e) => return Err(e),
                        }
                    }
                }
                None => {
                    while !wbuf.is_empty() {
                        match tcp.write(wbuf) {
                            Ok(0) => return Err(err("TURN connection closed")),
                            Ok(n) => {
                                wbuf.drain(..n);
                            }
                            Err(e) if would_block(&e) => break,
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Link::Udp(s) => match s.send(bytes) {
                Ok(_) => Ok(()),
                Err(e) if would_block(&e) => Ok(()),
                Err(e) => Err(e),
            },
            Link::Stream { tls, wbuf, .. } => {
                match tls {
                    Some(c) => c.writer().write_all(bytes)?,
                    None => {
                        // a stalled stream must not grow without bound
                        if wbuf.len() > 256 * 1024 {
                            return Err(err("TURN connection stalled"));
                        }
                        wbuf.extend_from_slice(bytes)
                    }
                }
                self.flush()
            }
        }
    }

    /// Next complete frame (STUN message or ChannelData), non-blocking.
    fn recv(&mut self, scratch: &mut [u8]) -> io::Result<Option<Vec<u8>>> {
        match self {
            Link::Udp(s) => match s.recv(scratch) {
                Ok(n) => Ok(Some(scratch[..n].to_vec())),
                Err(e) if would_block(&e) || e.kind() == io::ErrorKind::ConnectionRefused => {
                    Ok(None)
                }
                Err(e) => Err(e),
            },
            Link::Stream { tcp, tls, rbuf, .. } => loop {
                if let Some(r) = stun::stream_frame_len(rbuf) {
                    let n = r.map_err(|_| err("corrupt TURN stream"))?;
                    return Ok(Some(rbuf.drain(..n).collect()));
                }
                let got = match tls {
                    Some(c) => match c.read_tls(tcp) {
                        Ok(0) => return Err(err("TURN connection closed")),
                        Ok(_) => {
                            c.process_new_packets().map_err(|e| err(e.to_string()))?;
                            let mut any = false;
                            loop {
                                match c.reader().read(scratch) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        rbuf.extend_from_slice(&scratch[..n]);
                                        any = true;
                                    }
                                    Err(e) if would_block(&e) => break,
                                    Err(e) => return Err(e),
                                }
                            }
                            any
                        }
                        Err(e) if would_block(&e) => return Ok(None),
                        Err(e) => return Err(e),
                    },
                    None => match tcp.read(scratch) {
                        Ok(0) => return Err(err("TURN connection closed")),
                        Ok(n) => {
                            rbuf.extend_from_slice(&scratch[..n]);
                            true
                        }
                        Err(e) if would_block(&e) => return Ok(None),
                        Err(e) => return Err(e),
                    },
                };
                if !got {
                    return Ok(None);
                }
            },
        }
    }
}

fn tls_client(host: &str) -> io::Result<rustls::ClientConnection> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| err(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| err(e.to_string()))?;
    rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| err(e.to_string()))
}

/// Bytes through the TURN server, both directions. `*_wire` add the IP /
/// transport headers, which is what metered services bill (roughly).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnStats {
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_wire: u64,
    pub rx_wire: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Refresh,
    Bind(usize),
    Close,
}

/// A background request waiting for its answer.
struct Pending {
    tid: [u8; 12],
    kind: Kind,
    first: Instant,
    last: Instant,
    req: Vec<u8>,
}

struct Channel {
    peer: SocketAddr,
    number: u16,
    /// Last successful bind / refresh (keeps the permission alive too).
    bound: Instant,
    refreshing: bool,
}

/// Allocation lifetime the client asks for (servers may lower it).
const LIFETIME: u32 = 600;
/// Permissions expire after 300 s (RFC 8656 §9); channel bindings after
/// 600 s. A ChannelBind refresh renews both, so refresh well before 300 s.
const CHANNEL_REFRESH: Duration = Duration::from_secs(240);

/// One TURN allocation.
pub struct TurnClient {
    link: Link,
    proto: Proto,
    username: String,
    password: String,
    realm: String,
    nonce: String,
    key: [u8; 16],
    relayed: SocketAddr,
    mapped: Option<SocketAddr>,
    lifetime: u32,
    refreshed: Instant,
    refreshing: bool,
    channels: Vec<Channel>,
    pending: Vec<Pending>,
    stats: TurnStats,
    scratch: Vec<u8>,
    closed: bool,
}

impl TurnClient {
    /// Allocate a relayed address, trying `creds.urls` in [`TurnUrl::plan`]
    /// order within `budget` (UDP attempts get a short slice each).
    pub fn allocate(creds: &TurnCredentials, budget: Duration) -> io::Result<Self> {
        let plan = TurnUrl::plan(&creds.urls);
        if plan.is_empty() {
            return Err(err("no usable TURN url"));
        }
        let end = Instant::now() + budget;
        let mut last = err("no usable TURN url");
        for (i, u) in plan.iter().enumerate() {
            let now = Instant::now();
            if now >= end {
                break;
            }
            let left = end - now;
            let rest = (plan.len() - i) as u32;
            // UDP fails by timeout: do not let it eat the whole budget
            let slice = match u.proto {
                Proto::Udp => (left / rest).max(Duration::from_millis(600)).min(left),
                _ => left,
            };
            match Self::allocate_at(u, creds, now + slice) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    log::info!("turn: {} {}:{} failed: {e}", u.proto.name(), u.host, u.port);
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn allocate_at(u: &TurnUrl, creds: &TurnCredentials, deadline: Instant) -> io::Result<Self> {
        let link = Link::open(u, deadline)?;
        let mut c = Self {
            link,
            proto: u.proto,
            username: creds.username.clone(),
            password: creds.credential.clone(),
            realm: String::new(),
            nonce: String::new(),
            key: [0; 16],
            relayed: SocketAddr::from(([0, 0, 0, 0], 0)),
            mapped: None,
            lifetime: LIFETIME,
            refreshed: Instant::now(),
            refreshing: false,
            channels: Vec::new(),
            pending: Vec::new(),
            stats: TurnStats::default(),
            scratch: vec![0; 4096],
            closed: false,
        };
        let m = c.transact(
            method::ALLOCATE,
            &|b| {
                b.attr(attr::REQUESTED_TRANSPORT, &[17, 0, 0, 0])
                    .u32_attr(attr::LIFETIME, LIFETIME)
            },
            deadline,
        )?;
        c.relayed = m
            .xor_addr(attr::XOR_RELAYED_ADDRESS)
            .ok_or_else(|| err("allocate: no relayed address"))?;
        c.mapped = m.xor_addr(attr::XOR_MAPPED_ADDRESS);
        c.lifetime = m.u32_attr(attr::LIFETIME).unwrap_or(LIFETIME).max(60);
        c.refreshed = Instant::now();
        Ok(c)
    }

    pub fn relayed(&self) -> SocketAddr {
        self.relayed
    }

    pub fn proto(&self) -> Proto {
        self.proto
    }

    pub fn stats(&self) -> TurnStats {
        self.stats
    }

    fn request(&self, m: u16, attrs: &dyn Fn(Builder) -> Builder, t: [u8; 12]) -> Vec<u8> {
        let mut b = attrs(Builder::new(m, Class::Request, t));
        if !self.realm.is_empty() {
            b = b
                .attr(attr::USERNAME, self.username.as_bytes())
                .attr(attr::REALM, self.realm.as_bytes())
                .attr(attr::NONCE, self.nonce.as_bytes())
                .integrity(&self.key);
        }
        b.finish()
    }

    fn send_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stats.tx_bytes += bytes.len() as u64;
        self.stats.tx_packets += 1;
        self.stats.tx_wire += bytes.len() as u64 + self.proto.overhead();
        self.link.send(bytes)
    }

    fn recv_raw(&mut self) -> io::Result<Option<Vec<u8>>> {
        let r = self.link.recv(&mut self.scratch)?;
        if let Some(b) = &r {
            self.stats.rx_bytes += b.len() as u64;
            self.stats.rx_packets += 1;
            self.stats.rx_wire += b.len() as u64 + self.proto.overhead();
        }
        Ok(r)
    }

    /// Take realm / nonce from a 401 / 438 answer; true if a retry makes sense.
    fn learn_auth(&mut self, m: &Message) -> bool {
        let Some((code, _)) = m.error() else {
            return false;
        };
        if !matches!(code, 401 | 438) {
            return false;
        }
        if let Some(n) = m.text(attr::NONCE) {
            self.nonce = n;
        }
        if let Some(r) = m.text(attr::REALM) {
            self.realm = r;
        }
        if self.realm.is_empty() || self.nonce.is_empty() {
            return false;
        }
        self.key = stun::long_term_key(&self.username, &self.realm, &self.password);
        true
    }

    /// Blocking request / response with retransmission (UDP) and one
    /// authentication retry (401 / stale nonce 438).
    fn transact(
        &mut self,
        m: u16,
        attrs: &dyn Fn(Builder) -> Builder,
        deadline: Instant,
    ) -> io::Result<Message> {
        let mut retries = 0;
        'outer: loop {
            let t = tid();
            let req = self.request(m, attrs, t);
            let mut rto = Duration::from_millis(150);
            let mut sent = Instant::now();
            self.send_raw(&req)?;
            loop {
                if Instant::now() >= deadline {
                    return Err(err("TURN server did not answer"));
                }
                if !self.link.is_stream() && sent.elapsed() >= rto {
                    self.send_raw(&req)?;
                    sent = Instant::now();
                    rto = (rto * 2).min(Duration::from_millis(800));
                }
                self.link.flush()?;
                let Some(raw) = self.recv_raw()? else {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                };
                let Some(resp) = Message::decode(&raw) else {
                    continue;
                };
                if resp.tid != t {
                    continue;
                }
                match resp.class {
                    Class::Success => {
                        if !self.realm.is_empty() && !stun::check_integrity(&raw, &self.key) {
                            return Err(err("TURN answer failed integrity check"));
                        }
                        return Ok(resp);
                    }
                    Class::Error => {
                        if retries < 2 && self.learn_auth(&resp) {
                            retries += 1;
                            continue 'outer;
                        }
                        let (code, why) = resp.error().unwrap_or((0, String::new()));
                        return Err(err(format!("TURN error {code} {why}")));
                    }
                    _ => continue,
                }
            }
        }
    }

    /// Install a permission and a channel for `peer` (blocking). Returns
    /// the channel number.
    pub fn bind_peer(&mut self, peer: SocketAddr, timeout: Duration) -> io::Result<u16> {
        let deadline = Instant::now() + timeout;
        if let Some(c) = self.channels.iter().find(|c| c.peer == peer) {
            return Ok(c.number);
        }
        self.transact(
            method::CREATE_PERMISSION,
            &|b| b.xor_addr(attr::XOR_PEER_ADDRESS, peer),
            deadline,
        )?;
        let number = stun::CHANNEL_MIN + self.channels.len() as u16;
        self.transact(
            method::CHANNEL_BIND,
            &|b| {
                b.u32_attr(attr::CHANNEL_NUMBER, (number as u32) << 16)
                    .xor_addr(attr::XOR_PEER_ADDRESS, peer)
            },
            deadline,
        )?;
        self.channels.push(Channel {
            peer,
            number,
            bound: Instant::now(),
            refreshing: false,
        });
        Ok(number)
    }

    /// Send `data` to a bound peer as ChannelData. Unbound peers are not
    /// sent to (no Send indications: they cost 36+ extra bytes each).
    pub fn send(&mut self, peer: SocketAddr, data: &[u8]) -> bool {
        let Some(ch) = self.channels.iter().find(|c| c.peer == peer) else {
            return false;
        };
        let frame = stun::channel_data(ch.number, data, self.link.is_stream());
        self.send_raw(&frame).is_ok()
    }

    /// Everything that arrived from peers, non-blocking; also handles the
    /// answers to background refreshes.
    pub fn poll(&mut self, out: &mut Vec<(SocketAddr, Vec<u8>)>) {
        if let Err(e) = self.link.flush() {
            log::debug!("turn: {e}");
        }
        loop {
            let raw = match self.recv_raw() {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    log::debug!("turn: {e}");
                    break;
                }
            };
            if let Some((ch, data)) = stun::parse_channel_data(&raw) {
                if let Some(c) = self.channels.iter().find(|c| c.number == ch) {
                    out.push((c.peer, data.to_vec()));
                }
                continue;
            }
            let Some(m) = Message::decode(&raw) else {
                continue;
            };
            match (m.method, m.class) {
                (method::DATA, Class::Indication) => {
                    if let (Some(from), Some(d)) =
                        (m.xor_addr(attr::XOR_PEER_ADDRESS), m.get(attr::DATA))
                    {
                        out.push((from, d.to_vec()));
                    }
                }
                (_, Class::Success | Class::Error) => self.on_answer(&m, &raw),
                _ => {}
            }
        }
    }

    fn on_answer(&mut self, m: &Message, raw: &[u8]) {
        let Some(i) = self.pending.iter().position(|p| p.tid == m.tid) else {
            return;
        };
        let kind = self.pending.remove(i).kind;
        if m.class == Class::Success {
            if !stun::check_integrity(raw, &self.key) {
                return;
            }
            match kind {
                Kind::Refresh => {
                    self.refreshed = Instant::now();
                    self.refreshing = false;
                    if let Some(l) = m.u32_attr(attr::LIFETIME) {
                        self.lifetime = l.max(60);
                    }
                }
                Kind::Bind(c) => {
                    if let Some(ch) = self.channels.get_mut(c) {
                        ch.bound = Instant::now();
                        ch.refreshing = false;
                    }
                }
                Kind::Close => {}
            }
            return;
        }
        // stale nonce: learn it and resend at once; anything else retries
        // at the next tick
        let stale = self.learn_auth(m);
        match kind {
            Kind::Refresh => {
                self.refreshing = false;
                if stale {
                    self.refresh(self.lifetime);
                }
            }
            Kind::Bind(c) => {
                if let Some(ch) = self.channels.get_mut(c) {
                    ch.refreshing = false;
                }
                if stale {
                    self.rebind(c);
                }
            }
            Kind::Close => {}
        }
    }

    fn start(&mut self, m: u16, attrs: &dyn Fn(Builder) -> Builder, kind: Kind) {
        let t = tid();
        let req = self.request(m, attrs, t);
        let _ = self.send_raw(&req);
        let now = Instant::now();
        self.pending.push(Pending {
            tid: t,
            kind,
            first: now,
            last: now,
            req,
        });
    }

    fn refresh(&mut self, lifetime: u32) {
        self.refreshing = true;
        self.start(
            method::REFRESH,
            &|b| b.u32_attr(attr::LIFETIME, lifetime),
            Kind::Refresh,
        );
    }

    fn rebind(&mut self, i: usize) {
        let Some(ch) = self.channels.get_mut(i) else {
            return;
        };
        ch.refreshing = true;
        let (number, peer) = (ch.number, ch.peer);
        self.start(
            method::CHANNEL_BIND,
            &|b| {
                b.u32_attr(attr::CHANNEL_NUMBER, (number as u32) << 16)
                    .xor_addr(attr::XOR_PEER_ADDRESS, peer)
            },
            Kind::Bind(i),
        );
    }

    /// Keep the allocation and channels alive with as few requests as the
    /// lifetimes allow; retransmit unanswered refreshes (UDP).
    pub fn tick(&mut self) {
        if self.closed {
            return;
        }
        let now = Instant::now();
        let margin = Duration::from_secs(60).min(Duration::from_secs(self.lifetime as u64 / 4));
        let due = Duration::from_secs(self.lifetime as u64).saturating_sub(margin);
        if !self.refreshing && now.duration_since(self.refreshed) >= due {
            self.refresh(self.lifetime);
        }
        for i in 0..self.channels.len() {
            let c = &self.channels[i];
            if !c.refreshing && now.duration_since(c.bound) >= CHANNEL_REFRESH {
                self.rebind(i);
            }
        }
        // retransmit unanswered requests (UDP); forget them after 10 s so
        // they are tried again from scratch
        let udp = !self.link.is_stream();
        let mut resend = Vec::new();
        self.pending.retain_mut(|p| {
            if now.duration_since(p.first) > Duration::from_secs(10) {
                return false;
            }
            if udp && now.duration_since(p.last) >= Duration::from_millis(500) {
                p.last = now;
                resend.push(p.req.clone());
            }
            true
        });
        for r in resend {
            let _ = self.send_raw(&r);
        }
        if self.refreshing && !self.pending.iter().any(|p| p.kind == Kind::Refresh) {
            self.refreshing = false;
        }
        for (i, c) in self.channels.iter_mut().enumerate() {
            if c.refreshing && !self.pending.iter().any(|p| p.kind == Kind::Bind(i)) {
                c.refreshing = false;
            }
        }
    }

    /// Release the allocation now (Refresh with lifetime 0), best effort.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.start(
            method::REFRESH,
            &|b| b.u32_attr(attr::LIFETIME, 0),
            Kind::Close,
        );
        let _ = self.link.flush();
        self.closed = true;
    }
}

impl Drop for TurnClient {
    fn drop(&mut self) {
        self.close();
    }
}

// ---- the match overlay

/// Relay payload types (first byte of a relay `DATA` payload); 0-2 are
/// defined in [`crate::relay`].
pub(crate) const T_TURN: u8 = 3;
/// TURN datagram: every ggrs message of one tick, varint-encoded.
const T_BATCH: u8 = 4;
/// TURN path check (overlay internal), payload = sender clock u64.
const T_PROBE: u8 = 5;
const T_PROBE_ACK: u8 = 6;

const SIG_OFFER: u8 = 0;
const SIG_UNAVAILABLE: u8 = 1;

/// ggrs message kinds (bincode variant index of `MessageBody`).
const MSG_INPUT: u8 = 2;
const MSG_INPUT_ACK: u8 = 3;
const MSG_KEEP_ALIVE: u8 = 7;

/// Without a TURN packet from the peer for this long the overlay counts as
/// unhealthy and traffic goes through the server relay as well.
const HEALTHY: Duration = Duration::from_millis(1500);
/// Give up on TURN when it is not working after this long.
const GIVE_UP: Duration = Duration::from_secs(12);
const ALLOC_BUDGET: Duration = Duration::from_secs(4);

fn compact() -> impl bincode::Options {
    bincode::DefaultOptions::new()
}

fn variant(msg: &ggrs::Message) -> Option<u8> {
    // bincode 1 (fixint): u16 magic, u32 variant index, fields
    let b = bincode::serialize(msg).ok()?;
    b.get(2).copied()
}

enum State {
    Allocating(Receiver<io::Result<TurnClient>>),
    Allocated(TurnClient),
    Binding(Receiver<(TurnClient, io::Result<u16>)>),
    Ready(TurnClient),
    Failed,
}

/// Lifts the ggrs traffic between the two players onto TURN once both
/// sides have a working channel; until then (and whenever TURN goes quiet)
/// the server relay carries it.
pub(crate) struct Overlay {
    pub(crate) peer: u8,
    state: State,
    proto: Option<Proto>,
    peer_addr: Option<SocketAddr>,
    peer_gave_up: bool,
    gave_up: bool,
    started: Instant,
    last_offer: Option<Instant>,
    last_probe: Option<Instant>,
    verified: bool,
    heard: bool,
    last_rx: Instant,
    clock: Instant,
    rtt: Option<u32>,
    batch: Vec<(u8, Vec<u8>)>,
    batch_since: Option<Instant>,
    pub(crate) inbox: VecDeque<Vec<u8>>,
    rx: Vec<(SocketAddr, Vec<u8>)>,
    meter: Arc<Mutex<TurnStats>>,
    /// Diagnostic: one datagram per ggrs message, plain encoding
    /// (`RUGEN_TURN_PLAIN=1`, to measure what the optimisations save).
    pub(crate) plain: bool,
}

impl Overlay {
    /// The overlay for a ticket with a `turn` transport (players only).
    pub(crate) fn from_ticket(t: &rugen_proto::SessionTicket) -> Option<Self> {
        let tr = t.turn()?;
        let creds = TurnCredentials {
            urls: tr.urls.clone(),
            username: tr.username.clone(),
            credential: tr.credential.clone(),
        };
        Self::start(creds, t.slot)
    }

    /// Start allocating in the background (players only).
    pub(crate) fn start(creds: TurnCredentials, slot: u8) -> Option<Self> {
        if slot > 1 || creds.urls.is_empty() {
            return None;
        }
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("turn-allocate".into())
            .spawn(move || {
                let _ = tx.send(TurnClient::allocate(&creds, ALLOC_BUDGET));
            })
            .ok()?;
        let now = Instant::now();
        Some(Self {
            peer: 1 - slot,
            state: State::Allocating(rx),
            proto: None,
            peer_addr: None,
            peer_gave_up: false,
            gave_up: false,
            started: now,
            last_offer: None,
            last_probe: None,
            verified: false,
            heard: false,
            last_rx: now,
            clock: now,
            rtt: None,
            batch: Vec::new(),
            batch_since: None,
            inbox: VecDeque::new(),
            rx: Vec::new(),
            meter: Arc::default(),
            plain: std::env::var_os("RUGEN_TURN_PLAIN").is_some(),
        })
    }

    /// TURN carries traffic to `to`.
    pub(crate) fn carries(&self, to: u8) -> bool {
        self.verified && to == self.peer && matches!(self.state, State::Ready(_))
    }

    pub(crate) fn healthy(&self) -> bool {
        self.last_rx.elapsed() < HEALTHY
    }

    /// Still working towards a TURN path (or using it).
    pub(crate) fn pending(&self) -> bool {
        !self.verified && !self.gave_up
    }

    pub(crate) fn name(&self) -> Option<&'static str> {
        self.verified.then(|| self.proto.map(Proto::name)).flatten()
    }

    pub(crate) fn rtt(&self) -> Option<u32> {
        self.rtt
    }

    pub(crate) fn stats(&self) -> TurnStats {
        self.meter.lock().map(|m| *m).unwrap_or_default()
    }

    /// Shared byte counters (stay readable after the socket moved into ggrs).
    pub(crate) fn meter(&self) -> Arc<Mutex<TurnStats>> {
        self.meter.clone()
    }

    fn record(&self) {
        if let State::Ready(c) | State::Allocated(c) = &self.state
            && let Ok(mut m) = self.meter.lock()
        {
            *m = c.stats();
        }
    }

    fn fail(&mut self, why: &str, signals: &mut Vec<Vec<u8>>) {
        self.record();
        // dropping the client releases the allocation
        self.state = State::Failed;
        self.verified = false;
        if !self.gave_up {
            self.gave_up = true;
            log::info!("turn: not used ({why}); staying on the server relay");
            signals.push(vec![T_TURN, SIG_UNAVAILABLE]);
        }
    }

    /// Advance the setup / keep-alive state machine. Returns relay payloads
    /// to send to the peer (signalling through `rugen-server`).
    pub(crate) fn pump(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let now = Instant::now();
        if !self.verified && now.duration_since(self.started) > GIVE_UP {
            self.fail("no working TURN path", &mut out);
        }
        let offer_due = self
            .last_offer
            .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250));
        let state = std::mem::replace(&mut self.state, State::Failed);
        self.state = match state {
            State::Allocating(rx) => match rx.try_recv() {
                Ok(Ok(c)) => {
                    log::info!("turn: allocated over {}", c.proto().name());
                    self.proto = Some(c.proto());
                    State::Allocated(c)
                }
                Ok(Err(e)) => {
                    self.fail(&format!("allocation failed: {e}"), &mut out);
                    return out;
                }
                Err(TryRecvError::Empty) => State::Allocating(rx),
                Err(TryRecvError::Disconnected) => {
                    self.fail("allocation thread died", &mut out);
                    return out;
                }
            },
            State::Allocated(mut c) => {
                if offer_due {
                    out.push(offer(c.relayed()));
                    self.last_offer = Some(now);
                }
                c.tick();
                if let Some(peer) = self.peer_addr {
                    let (tx, rx) = channel();
                    std::thread::spawn(move || {
                        let r = c.bind_peer(peer, Duration::from_secs(3));
                        let _ = tx.send((c, r));
                    });
                    State::Binding(rx)
                } else {
                    State::Allocated(c)
                }
            }
            State::Binding(rx) => match rx.try_recv() {
                Ok((c, Ok(_))) => State::Ready(c),
                Ok((c, Err(e))) => {
                    self.state = State::Allocated(c);
                    self.fail(&format!("channel bind failed: {e}"), &mut out);
                    return out;
                }
                Err(TryRecvError::Empty) => State::Binding(rx),
                Err(TryRecvError::Disconnected) => {
                    self.fail("bind thread died", &mut out);
                    return out;
                }
            },
            State::Ready(mut c) => {
                // the peer needs our address until it has reached us
                if !self.heard && offer_due {
                    out.push(offer(c.relayed()));
                    self.last_offer = Some(now);
                }
                if !self.verified
                    && self
                        .last_probe
                        .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    && let Some(p) = self.peer_addr
                {
                    let mut probe = vec![T_PROBE];
                    probe.extend(self.now_ms().to_be_bytes());
                    c.send(p, &probe);
                    self.last_probe = Some(now);
                }
                c.tick();
                State::Ready(c)
            }
            State::Failed => State::Failed,
        };
        if self.peer_gave_up && !self.verified {
            self.fail("the other player has no TURN path", &mut out);
        }
        self.record();
        out
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// A signalling payload from the peer (relay `DATA`, after `T_TURN`).
    pub(crate) fn on_signal(&mut self, p: &[u8]) {
        match p.first() {
            Some(&SIG_OFFER) => {
                let Some(a) = std::str::from_utf8(&p[1..])
                    .ok()
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                else {
                    return;
                };
                if self.peer_addr.is_none() {
                    log::debug!("turn: the other player's relayed address arrived");
                    self.peer_addr = Some(a);
                }
            }
            Some(&SIG_UNAVAILABLE) => self.peer_gave_up = true,
            _ => {}
        }
    }

    /// Receive from the peer over TURN into [`Self::inbox`] (relay payload
    /// format: `T_GGRS` + bincode, `T_PING`, `T_PONG`).
    pub(crate) fn poll(&mut self) {
        let State::Ready(c) = &mut self.state else {
            return;
        };
        self.rx.clear();
        c.poll(&mut self.rx);
        let Some(peer) = self.peer_addr else {
            return;
        };
        let mut acks = Vec::new();
        for (from, d) in self.rx.drain(..) {
            // only the peer's relayed address (others cannot pass the
            // permission anyway)
            if from != peer || d.is_empty() {
                continue;
            }
            self.last_rx = Instant::now();
            self.heard = true;
            match d[0] {
                T_PROBE if d.len() == 9 => {
                    let mut a = d.clone();
                    a[0] = T_PROBE_ACK;
                    acks.push(a);
                }
                T_PROBE_ACK if d.len() == 9 => {
                    let t = u64::from_be_bytes(d[1..9].try_into().unwrap_or_default());
                    let rtt = self.clock.elapsed().as_millis() as u64;
                    self.rtt = Some(rtt.saturating_sub(t) as u32);
                    if !self.verified {
                        log::info!(
                            "turn: path to the other player works ({} ms), switching",
                            self.rtt.unwrap_or(0)
                        );
                    }
                    self.verified = true;
                }
                T_BATCH => {
                    let mut i = 1;
                    while i < d.len() {
                        let (len, hdr) = match d[i] {
                            0xFF if i + 2 < d.len() => {
                                (u16::from_be_bytes([d[i + 1], d[i + 2]]) as usize, 3)
                            }
                            n => (n as usize, 1),
                        };
                        let Some(m) = d.get(i + hdr..i + hdr + len) else {
                            break;
                        };
                        if let Ok(msg) = compact().deserialize::<ggrs::Message>(m)
                            && let Ok(b) = bincode::serialize(&msg)
                        {
                            let mut p = Vec::with_capacity(b.len() + 1);
                            p.push(crate::relay::T_GGRS);
                            p.extend(b);
                            self.inbox.push_back(p);
                        }
                        i += hdr + len;
                    }
                }
                _ => self.inbox.push_back(d),
            }
        }
        for a in acks {
            c.send(peer, &a);
        }
        if self
            .batch_since
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(4))
        {
            self.flush();
        }
        self.record();
    }

    /// Send a relay-format payload to the peer over TURN. ggrs messages are
    /// held and coalesced until the tick's input goes out.
    pub(crate) fn send(&mut self, p: &[u8]) {
        if p.is_empty() {
            return;
        }
        if p[0] != crate::relay::T_GGRS || self.plain {
            if let (State::Ready(c), Some(peer)) = (&mut self.state, self.peer_addr) {
                c.send(peer, p);
            }
            return;
        }
        let Ok(msg) = bincode::deserialize::<ggrs::Message>(&p[1..]) else {
            return;
        };
        let (Some(kind), Ok(bytes)) = (variant(&msg), compact().serialize(&msg)) else {
            return;
        };
        self.batch.push((kind, bytes));
        self.batch_since.get_or_insert_with(Instant::now);
        if kind == MSG_INPUT {
            self.flush();
        }
    }

    /// Send the held ggrs messages as one datagram, dropping the ones the
    /// batch makes redundant (an input carries the ack frame; any message
    /// keeps the connection alive).
    pub(crate) fn flush(&mut self) {
        self.batch_since = None;
        if self.batch.is_empty() {
            return;
        }
        let has_input = self.batch.iter().any(|(k, _)| *k == MSG_INPUT);
        let n = self.batch.len();
        let mut out = vec![T_BATCH];
        for (k, b) in self.batch.drain(..) {
            if (has_input && k == MSG_INPUT_ACK) || (n > 1 && k == MSG_KEEP_ALIVE) {
                continue;
            }
            if b.len() < 0xFF {
                out.push(b.len() as u8);
            } else {
                out.push(0xFF);
                out.extend((b.len() as u16).to_be_bytes());
            }
            out.extend(b);
        }
        if let (State::Ready(c), Some(peer)) = (&mut self.state, self.peer_addr) {
            c.send(peer, &out);
        }
    }
}

fn offer(relayed: SocketAddr) -> Vec<u8> {
    let mut p = vec![T_TURN, SIG_OFFER];
    p.extend(relayed.to_string().as_bytes());
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_parsed_and_ordered() {
        let u = TurnUrl::parse("turn:turn.cloudflare.com:3478?transport=udp").unwrap();
        assert_eq!(
            u,
            TurnUrl {
                proto: Proto::Udp,
                host: "turn.cloudflare.com".into(),
                port: 3478
            }
        );
        assert_eq!(
            TurnUrl::parse("turns:turn.cloudflare.com:443?transport=tcp")
                .unwrap()
                .proto,
            Proto::Tls
        );
        assert_eq!(TurnUrl::parse("turn:[::1]").unwrap().port, 3478);
        assert_eq!(TurnUrl::parse("turn:[::1]:9").unwrap().host, "::1");
        assert!(TurnUrl::parse("stun:stun.cloudflare.com:3478").is_none());
        assert!(TurnUrl::parse("turn::3478").is_none());
        let cf: Vec<String> = [
            "turn:turn.cloudflare.com:3478?transport=udp",
            "turn:turn.cloudflare.com:53?transport=udp",
            "turn:turn.cloudflare.com:443?transport=udp",
            "turn:turn.cloudflare.com:3478?transport=tcp",
            "turns:turn.cloudflare.com:5349?transport=tcp",
            "turns:turn.cloudflare.com:443?transport=tcp",
            "stun:stun.cloudflare.com:3478",
        ]
        .map(String::from)
        .to_vec();
        let p = TurnUrl::plan(&cf);
        let got: Vec<(Proto, u16)> = p.iter().map(|u| (u.proto, u.port)).collect();
        assert_eq!(
            got,
            vec![
                (Proto::Udp, 3478),
                (Proto::Udp, 443),
                (Proto::Tcp, 3478),
                (Proto::Tls, 5349),
                (Proto::Tls, 443)
            ]
        );
    }

    #[test]
    fn credentials_never_print_the_secret() {
        let c = TurnCredentials {
            urls: vec!["turn:x".into()],
            username: "u".into(),
            credential: "hunter2".into(),
        };
        assert!(!format!("{c}").contains("hunter2"));
    }
}
