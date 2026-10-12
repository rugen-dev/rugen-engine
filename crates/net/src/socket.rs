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
#[derive(Clone, Copy, Debug)]
pub struct Punched {
    pub remote: SocketAddr,
    /// Median round trip of the exchange.
    pub rtt_ms: Option<u32>,
}

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
        progress(&format!("waiting for {peer}"));
        let start = Instant::now();
        let ms = || start.elapsed().as_millis() as u64;
        let mut last_send = start - Duration::from_secs(1);
        let mut contact: Option<Instant> = None;
        let mut samples: Vec<u32> = Vec::new();
        let mut buf = [0u8; 512];
        while start.elapsed() < timeout {
            if let Some(c) = contact
                && (samples.len() >= 8 || c.elapsed() > Duration::from_millis(1500))
            {
                for _ in 0..3 {
                    let _ = self
                        .socket
                        .send_to(format!("rugen1 punch {}", ms()).as_bytes(), peer);
                }
                return Ok(Punched {
                    remote: peer,
                    rtt_ms: median(&mut samples),
                });
            }
            let every = if contact.is_some() { 60 } else { 100 };
            if last_send.elapsed() >= Duration::from_millis(every) {
                let _ = self
                    .socket
                    .send_to(format!("rugen1 punch {}", ms()).as_bytes(), peer);
                last_send = Instant::now();
            }
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) if from == peer => {
                    let d = &buf[..n];
                    if let Some(t) = d.strip_prefix(PUNCH) {
                        let mut pong = PONG.to_vec();
                        pong.extend_from_slice(t);
                        let _ = self.socket.send_to(&pong, from);
                    } else if let Some(t) = d.strip_prefix(PONG)
                        && let Some(t) = std::str::from_utf8(t)
                            .ok()
                            .and_then(|t| t.trim().parse::<u64>().ok())
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
