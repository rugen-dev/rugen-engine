//! Match traffic through `rugen-server`'s relay. ggrs sees the other
//! players as virtual addresses ([`slot_addr`]); every datagram goes to the
//! server tagged with session, slots and our secret token, and the server
//! forwards it. The players never learn each other's IP address.
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

use crate::socket::median;
use crate::{Error, Result};

/// Inner payload types of relayed `DATA` frames.
const T_GGRS: u8 = 0;
const T_PING: u8 = 1;
const T_PONG: u8 = 2;

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
        })
    }

    /// Join the relay session of `ticket`: UDP if the server's relay port
    /// answers within `udp_probe`, otherwise the WebSocket at `server_base`.
    pub fn connect(
        ticket: &SessionTicket,
        server_base: &str,
        udp_probe: Duration,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Self> {
        if let Some(udp) = ticket.udp.as_deref().filter(|_| !udp_probe.is_zero()) {
            let udp = udp_target(udp, server_base);
            match Self::connect_udp(ticket, &udp, udp_probe) {
                Ok(s) => {
                    progress("relay: UDP");
                    return Ok(s);
                }
                Err(e) => progress(&format!("relay: UDP unavailable ({e}), using WebSocket")),
            }
        }
        let s = Self::connect_ws(ticket, &relay_ws_url(server_base))?;
        progress("relay: WebSocket");
        Ok(s)
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

    pub fn transport_name(&self) -> &'static str {
        match self.transport {
            Transport::Udp { .. } => "UDP",
            Transport::Ws(_) => "WebSocket",
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

    /// Next frame from the server, non-blocking.
    fn recv_frame(&mut self) -> Option<(Header, Vec<u8>)> {
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
                self.send_frame(wire::DATA, h.from, &pong);
                None
            }
            T_PONG if p.len() == 9 => {
                let t = u64::from_be_bytes(p[1..9].try_into().ok()?);
                Some(self.now_ms().saturating_sub(t) as u32)
            }
            _ => None,
        }
    }

    /// Median round trip to the player in `peer` through the server
    /// (that player answers automatically while connecting or playing).
    pub fn peer_rtt(&mut self, peer: u8, samples: usize, timeout: Duration) -> Option<u32> {
        let start = Instant::now();
        let mut last = start - Duration::from_secs(1);
        let mut got = Vec::new();
        while start.elapsed() < timeout && got.len() < samples {
            if last.elapsed() >= Duration::from_millis(40) {
                let mut p = vec![T_PING];
                p.extend(self.now_ms().to_be_bytes());
                self.send_frame(wire::DATA, peer, &p);
                last = Instant::now();
            }
            while let Some((h, p)) = self.recv_frame() {
                if let Some(rtt) = self.answer(h, &p)
                    && h.from == peer
                {
                    got.push(rtt);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        median(&mut got)
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

    pub fn bye(&mut self) {
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
            self.send_frame(wire::DATA, to, &p);
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, ggrs::Message)> {
        // keep the UDP mapping / WebSocket alive even when ggrs is quiet
        if self.last_hello.elapsed() >= Duration::from_secs(5) {
            self.send_frame(wire::HELLO, 0, &[]);
            self.last_hello = Instant::now();
        }
        let mut out = std::mem::take(&mut self.early);
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
