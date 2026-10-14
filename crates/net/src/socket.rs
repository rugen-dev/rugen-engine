//! Direct UDP socket for LAN / explicit-IP matches (`--net ip:port`, HOST /
//! JOIN DIRECT MATCH). Online play through a server uses
//! [`crate::relay::RelaySocket`] instead and never exposes addresses.
//!
//! Before ggrs takes the socket over, [`Socket::handshake`] waits for the
//! peer and measures the round trip (for automatic input delay).

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use ggrs::NonBlockingSocket;

use crate::{Error, Result};

const PUNCH: &[u8] = b"rugen1 punch ";
const PONG: &[u8] = b"rugen1 pong ";

/// Outcome of [`Socket::handshake`].
#[derive(Clone, Debug)]
pub struct Punched {
    pub remote: SocketAddr,
    /// Median round trip of the exchange.
    pub rtt_ms: Option<u32>,
    /// Median / p95 / jitter of the exchange.
    pub rtt: Option<crate::RttStats>,
    /// The match rules the peer sent ([`HandshakeRules::Receive`]); `None`
    /// when it sent none (an older build) or none were asked for.
    pub rules: Option<String>,
    /// The peer's LAN protocol ([`LAN_PROTOCOL`]); `None` for a build that
    /// sends none (0.6.1 and earlier).
    pub peer_protocol: Option<u32>,
}

/// Match rules carried by [`Socket::handshake_with`]: the host sends its own
/// (a short ASCII string without spaces), the other side receives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeRules<'a> {
    None,
    Send(&'a str),
    Receive,
}

/// Marker of the rules inside a punch: `rugen1 punch <ms> v=<n> r=<rules>`.
const RULES: &str = " r=";
/// Marker of the sender's LAN protocol inside a punch (before the rules).
const PROTOCOL: &str = " v=";
/// LAN protocol of this build, sent in every punch. 2: the host's rules
/// carry the engine movement (air jumps, air dash, fast fall) next to the
/// fighter size. 3: they carry the match seed too. Builds before 2 (0.6.1
/// and earlier) send no number.
pub const LAN_PROTOCOL: u32 = 4;

/// The oldest LAN protocol a peer may run: a protocol 2 peer agrees on
/// everything but the seed, so both sides play seed 1 with it.
pub const LAN_PROTOCOL_MIN: u32 = 2;

/// The first LAN protocol whose host sends the match seed.
pub const LAN_PROTOCOL_SEED: u32 = 3;

/// First LAN protocol whose host sends the LIFE setting (rugen 0.6.4); an
/// older peer plays each fighter's own life, so the host does too.
pub const LAN_PROTOCOL_LIFE: u32 = 4;

pub struct Socket {
    socket: UdpSocket,
    buffer: [u8; 4096],
}

fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
    )
}

/// Median of round-trip samples.
pub fn median(samples: &mut [u32]) -> Option<u32> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    Some(samples[samples.len() / 2])
}

/// This machine's LAN address (route lookup, no packet is sent).
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

impl Socket {
    pub fn bind(port: u16) -> Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            buffer: [0; 4096],
        })
    }

    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Exchange small datagrams with `peer` until it answers, then a few
    /// more to measure the round trip. Both sides call this.
    pub fn handshake(
        &mut self,
        peer: SocketAddr,
        timeout: Duration,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Punched> {
        self.handshake_with(peer, timeout, HandshakeRules::None, progress)
    }

    /// [`handshake`](Self::handshake) that also carries the host's match
    /// rules: `Send` appends them to every punch, `Receive` keeps going (up
    /// to 3 s after contact) until they arrive. With rules either way it also
    /// waits (as long) for the peer's protocol number, so the caller can
    /// refuse an older build.
    pub fn handshake_with(
        &mut self,
        peer: SocketAddr,
        timeout: Duration,
        rules: HandshakeRules,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Punched> {
        let suffix = match rules {
            HandshakeRules::Send(r) => format!("{PROTOCOL}{LAN_PROTOCOL}{RULES}{r}"),
            _ => format!("{PROTOCOL}{LAN_PROTOCOL}"),
        };
        let mut got: Option<String> = None;
        let mut peer_protocol: Option<u32> = None;
        progress(&format!("waiting for {peer}"));
        let start = Instant::now();
        let ms = || start.elapsed().as_millis() as u64;
        let mut last_send = start - Duration::from_secs(1);
        let mut contact: Option<Instant> = None;
        let mut samples: Vec<u32> = Vec::new();
        let mut buf = [0u8; 512];
        while start.elapsed() < timeout {
            let missing = (rules == HandshakeRules::Receive && got.is_none())
                || (rules != HandshakeRules::None && peer_protocol.is_none());
            let waiting_rules =
                missing && contact.is_some_and(|c| c.elapsed() < Duration::from_secs(3));
            if let Some(c) = contact
                && (samples.len() >= 20 || c.elapsed() > Duration::from_millis(1500))
                && !waiting_rules
            {
                for _ in 0..3 {
                    let _ = self
                        .socket
                        .send_to(format!("rugen1 punch {}{suffix}", ms()).as_bytes(), peer);
                }
                let us: Vec<u32> = samples.iter().map(|ms| ms * 1000).collect();
                return Ok(Punched {
                    remote: peer,
                    rtt_ms: median(&mut samples),
                    rtt: crate::RttStats::from_us(&us),
                    rules: got,
                    peer_protocol,
                });
            }
            let every = if contact.is_some() { 30 } else { 100 };
            if last_send.elapsed() >= Duration::from_millis(every) {
                let _ = self
                    .socket
                    .send_to(format!("rugen1 punch {}{suffix}", ms()).as_bytes(), peer);
                last_send = Instant::now();
            }
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) if from == peer => {
                    let d = &buf[..n];
                    if let Some(t) = d.strip_prefix(PUNCH) {
                        let text = std::str::from_utf8(t).unwrap_or("");
                        let (stamp, sent) = match text.split_once(RULES) {
                            Some((a, r)) => (a, Some(r.trim())),
                            None => (text, None),
                        };
                        let stamp = match stamp.split_once(PROTOCOL) {
                            Some((a, v)) => {
                                if let Ok(v) = v.trim().parse::<u32>() {
                                    peer_protocol = Some(v);
                                }
                                a
                            }
                            None => stamp,
                        };
                        if rules == HandshakeRules::Receive
                            && let Some(r) = sent
                        {
                            got = Some(r.to_string());
                        }
                        let mut pong = PONG.to_vec();
                        pong.extend_from_slice(stamp.trim().as_bytes());
                        let _ = self.socket.send_to(&pong, from);
                    } else if let Some(t) = d.strip_prefix(PONG)
                        && let Some(t) = std::str::from_utf8(t)
                            .ok()
                            .and_then(|t| t.split_whitespace().next()?.parse::<u64>().ok())
                    {
                        samples.push(ms().saturating_sub(t) as u32);
                    }
                    if contact.is_none() {
                        progress(&format!("connected to {peer}"));
                        contact = Some(Instant::now());
                    }
                }
                Ok(_) => {}
                Err(e) if would_block(&e) => std::thread::sleep(Duration::from_millis(2)),
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Rendezvous(format!(
            "{peer} did not answer within {} s",
            timeout.as_secs()
        )))
    }
}

impl NonBlockingSocket<SocketAddr> for Socket {
    fn send_to(&mut self, msg: &ggrs::Message, addr: &SocketAddr) {
        if let Ok(buf) = bincode::serialize(msg) {
            let _ = self.socket.send_to(&buf, addr);
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, ggrs::Message)> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.buffer) {
                Ok((n, from)) => {
                    let data = &self.buffer[..n];
                    if let Some(t) = data.strip_prefix(PUNCH) {
                        // the peer is still handshaking: answer so it can finish
                        let mut pong = PONG.to_vec();
                        let t = t.split(|b| *b == b' ').next().unwrap_or(t);
                        pong.extend_from_slice(t);
                        let _ = self.socket.send_to(&pong, from);
                        continue;
                    }
                    if data.starts_with(b"rugen1") {
                        continue;
                    }
                    if let Ok(msg) = bincode::deserialize(data) {
                        out.push((from, msg));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return out,
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                Err(e) => {
                    log::warn!("udp recv: {e}");
                    return out;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_handshake_measures_rtt() {
        let mut a = Socket::bind(0).unwrap();
        let mut b = Socket::bind(0).unwrap();
        let (pa, pb) = (a.local_port(), b.local_port());
        let t = std::thread::spawn(move || {
            b.handshake(
                SocketAddr::from(([127, 0, 0, 1], pa)),
                Duration::from_secs(5),
                &mut |_| {},
            )
            .unwrap()
        });
        let ra = a
            .handshake(
                SocketAddr::from(([127, 0, 0, 1], pb)),
                Duration::from_secs(5),
                &mut |_| {},
            )
            .unwrap();
        let rb = t.join().unwrap();
        assert_eq!(ra.remote.port(), pb);
        assert_eq!(rb.remote.port(), pa);
        assert!(ra.rtt_ms.unwrap() < 200);
    }

    #[test]
    fn handshake_carries_the_hosts_rules() {
        let mut a = Socket::bind(0).unwrap();
        let mut b = Socket::bind(0).unwrap();
        let (pa, pb) = (a.local_port(), b.local_port());
        let t = std::thread::spawn(move || {
            b.handshake_with(
                SocketAddr::from(([127, 0, 0, 1], pa)),
                Duration::from_secs(5),
                HandshakeRules::Receive,
                &mut |_| {},
            )
            .unwrap()
        });
        let ra = a
            .handshake_with(
                SocketAddr::from(([127, 0, 0, 1], pb)),
                Duration::from_secs(5),
                HandshakeRules::Send("fighter_size=chibi"),
                &mut |_| {},
            )
            .unwrap();
        let rb = t.join().unwrap();
        assert_eq!(rb.rules.as_deref(), Some("fighter_size=chibi"));
        assert_eq!(ra.rules, None);
        assert_eq!(ra.peer_protocol, Some(LAN_PROTOCOL));
        assert_eq!(rb.peer_protocol, Some(LAN_PROTOCOL));
        assert!(
            ra.rtt_ms.is_some(),
            "pongs still parse with rules in the punch"
        );
    }

    /// A 0.6.1 host: punches carry the rules but no protocol number. The
    /// new side reports `peer_protocol: None` so the caller can refuse it,
    /// and an old peer still parses our punches' stamps.
    #[test]
    fn handshake_reports_an_older_peer() {
        let mut b = Socket::bind(0).unwrap();
        let old = UdpSocket::bind("127.0.0.1:0").unwrap();
        old.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let (po, pb) = (old.local_addr().unwrap().port(), b.local_port());
        let t = std::thread::spawn(move || {
            b.handshake_with(
                SocketAddr::from(([127, 0, 0, 1], po)),
                Duration::from_secs(8),
                HandshakeRules::Receive,
                &mut |_| {},
            )
            .unwrap()
        });
        let to = SocketAddr::from(([127, 0, 0, 1], pb));
        let start = Instant::now();
        let mut stamps_ok = true;
        let mut buf = [0u8; 512];
        while !t.is_finished() && start.elapsed() < Duration::from_secs(8) {
            let ms = start.elapsed().as_millis();
            // exactly what 0.6.1 sends as host
            let _ = old.send_to(
                format!("rugen1 punch {ms} r=fighter_size=chibi").as_bytes(),
                to,
            );
            if let Ok((n, _)) = old.recv_from(&mut buf) {
                let d = &buf[..n];
                if let Some(text) = d.strip_prefix(PUNCH) {
                    // 0.6.1's parse: split at " r=", pong the trimmed stamp
                    let text = std::str::from_utf8(text).unwrap();
                    let stamp = text.split_once(RULES).map_or(text, |(a, _)| a).trim();
                    stamps_ok &= stamp
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse::<u64>()
                        .is_ok();
                    let _ = old.send_to(format!("rugen1 pong {stamp}").as_bytes(), to);
                }
            }
        }
        let rb = t.join().unwrap();
        assert_eq!(rb.rules.as_deref(), Some("fighter_size=chibi"));
        assert_eq!(rb.peer_protocol, None);
        assert!(stamps_ok);
    }

    #[test]
    fn handshake_times_out_without_a_peer() {
        let mut a = Socket::bind(0).unwrap();
        let r = a.handshake(
            "127.0.0.1:9".parse().unwrap(),
            Duration::from_millis(300),
            &mut |_| {},
        );
        assert!(r.is_err());
    }

    #[test]
    fn median_of_samples() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [30, 10, 20]), Some(20));
    }
}
