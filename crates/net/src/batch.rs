//! Compact datagrams for the player ↔ player paths (TURN, direct): all ggrs
//! messages of one tick coalesced into one datagram in bincode's varint
//! encoding, with the messages a batch makes redundant left out (an input
//! carries the ack frame; any message keeps the connection alive).
//!
//! Payloads in and out are in the relay `DATA` format (`T_GGRS` + bincode,
//! `T_PING`, ...); a batch datagram is `T_BATCH` followed by
//! length-prefixed messages (1 byte, or `0xFF` + u16 BE for long ones).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bincode::Options as _;

/// A batch datagram: every ggrs message of one tick, varint-encoded.
pub(crate) const T_BATCH: u8 = 4;

/// ggrs message kinds (bincode variant index of `MessageBody`).
const MSG_INPUT: u8 = 2;
const MSG_INPUT_ACK: u8 = 3;
const MSG_KEEP_ALIVE: u8 = 7;

/// Messages wait at most this long for the tick's input to go out.
const HOLD: Duration = Duration::from_millis(4);

fn compact() -> impl bincode::Options {
    bincode::DefaultOptions::new()
}

fn variant(msg: &ggrs::Message) -> Option<u8> {
    // bincode 1 (fixint): u16 magic, u32 variant index, fields
    let b = bincode::serialize(msg).ok()?;
    b.get(2).copied()
}

/// Holds ggrs messages until the tick's input goes out, then emits them
/// as one datagram.
#[derive(Default)]
pub(crate) struct Batcher {
    batch: Vec<(u8, Vec<u8>)>,
    since: Option<Instant>,
    /// Diagnostic: one datagram per message, plain encoding.
    pub(crate) plain: bool,
}

impl Batcher {
    pub(crate) fn new(plain: bool) -> Self {
        Self {
            plain,
            ..Default::default()
        }
    }

    /// Queue a relay-format payload. Returns datagrams to send now: the
    /// payload itself when it is not a ggrs message (or `plain`), the
    /// batch when the payload was the tick's input.
    pub(crate) fn push(&mut self, p: &[u8]) -> Option<Vec<u8>> {
        if p.is_empty() {
            return None;
        }
        if p[0] != crate::relay::T_GGRS || self.plain {
            return Some(p.to_vec());
        }
        let msg = bincode::deserialize::<ggrs::Message>(&p[1..]).ok()?;
        let (Some(kind), Ok(bytes)) = (variant(&msg), compact().serialize(&msg)) else {
            return None;
        };
        self.batch.push((kind, bytes));
        self.since.get_or_insert_with(Instant::now);
        if kind == MSG_INPUT {
            return self.flush();
        }
        None
    }

    /// Held messages waited long enough (no input this tick).
    pub(crate) fn due(&self) -> bool {
        self.since.is_some_and(|t| t.elapsed() >= HOLD)
    }

    /// The held messages as one datagram.
    pub(crate) fn flush(&mut self) -> Option<Vec<u8>> {
        self.since = None;
        if self.batch.is_empty() {
            return None;
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
        Some(out)
    }
}

/// Unpack a received datagram into relay-format payloads: a batch into its
/// ggrs messages, anything else as is.
pub(crate) fn unpack(d: &[u8], out: &mut VecDeque<Vec<u8>>) {
    if d.first() != Some(&T_BATCH) {
        if !d.is_empty() {
            out.push_back(d.to_vec());
        }
        return;
    }
    let mut i = 1;
    while i < d.len() {
        let (len, hdr) = match d[i] {
            0xFF if i + 2 < d.len() => (u16::from_be_bytes([d[i + 1], d[i + 2]]) as usize, 3),
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
            out.push_back(p);
        }
        i += hdr + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ggrs_payloads_pass_through() {
        let mut b = Batcher::default();
        assert_eq!(b.push(&[1, 2, 3]), Some(vec![1, 2, 3]));
        assert_eq!(b.push(&[]), None);
        assert!(!b.due());
        assert_eq!(b.flush(), None);
        let mut q = VecDeque::new();
        unpack(&[1, 2, 3], &mut q);
        assert_eq!(q.pop_front(), Some(vec![1, 2, 3]));
    }
}
