//! Binary framing of relayed match traffic. The same datagrams travel over
//! UDP (fast path) or as binary WebSocket frames on `/relay` (fallback that
//! works through an HTTP-only tunnel).
//!
//! ```text
//! client → server:  'R' 'G' kind from to session:u32be token:[16] payload
//! server → client:  'R' 'G' kind from to session:u32be           payload
//! ```
//!
//! `from` / `to` are slots: 0 = P1, 1 = P2, 2.. = spectators. The server
//! checks the token of `from`, then forwards `DATA` to the slot `to`. It
//! never looks inside the payload (ggrs messages) and never tells a client
//! where another slot connects from.

/// Forward the payload to slot `to`.
pub const DATA: u8 = 0;
/// Register this transport for the slot; answered with [`ACK`].
pub const HELLO: u8 = 1;
/// Server → client: registered. Payload: bitmask of connected slots.
pub const ACK: u8 = 2;
/// Echo request to the server itself (client ↔ server round trip).
pub const PING: u8 = 3;
/// Server → client: the payload of a [`PING`], unchanged.
pub const PONG: u8 = 4;
/// Server → client: the set of connected slots changed. Payload: bitmask.
pub const PEERS: u8 = 5;
/// Leaving the session.
pub const BYE: u8 = 6;
/// Player → server: confirmed inputs of the match for the server's input
/// log ([`crate::stream::encode_upload`]: first frame, then 4 bytes per
/// frame). Only sent when the ticket says the server keeps one
/// ([`crate::SessionTicket::live`]); older servers drop unknown kinds.
pub const STREAM: u8 = 7;
/// Server → player: frames of this player's stream the server holds
/// (u32 LE); the player resumes from there.
pub const STREAM_ACK: u8 = 8;

pub const MAGIC: [u8; 2] = *b"RG";
pub const TOKEN_LEN: usize = 16;
pub const CLIENT_HEADER: usize = 9 + TOKEN_LEN;
pub const SERVER_HEADER: usize = 9;
/// Payload limit (ggrs messages are far smaller).
pub const MAX_PAYLOAD: usize = 1200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub from: u8,
    pub to: u8,
    pub session: u32,
}

fn head(out: &mut Vec<u8>, h: Header) {
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&[h.kind, h.from, h.to]);
    out.extend_from_slice(&h.session.to_be_bytes());
}

pub fn encode_client(h: Header, token: &[u8; TOKEN_LEN], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CLIENT_HEADER + payload.len());
    head(&mut out, h);
    out.extend_from_slice(token);
    out.extend_from_slice(payload);
    out
}

pub fn encode_server(h: Header, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SERVER_HEADER + payload.len());
    head(&mut out, h);
    out.extend_from_slice(payload);
    out
}

fn parse_head(buf: &[u8]) -> Option<Header> {
    if buf.len() < SERVER_HEADER || buf[..2] != MAGIC {
        return None;
    }
    Some(Header {
        kind: buf[2],
        from: buf[3],
        to: buf[4],
        session: u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]),
    })
}

/// Client datagram → (header, token, payload).
pub fn decode_client(buf: &[u8]) -> Option<(Header, [u8; TOKEN_LEN], &[u8])> {
    let h = parse_head(buf)?;
    if buf.len() < CLIENT_HEADER || buf.len() - CLIENT_HEADER > MAX_PAYLOAD {
        return None;
    }
    let mut token = [0u8; TOKEN_LEN];
    token.copy_from_slice(&buf[SERVER_HEADER..CLIENT_HEADER]);
    Some((h, token, &buf[CLIENT_HEADER..]))
}

/// Server datagram → (header, payload).
pub fn decode_server(buf: &[u8]) -> Option<(Header, &[u8])> {
    let h = parse_head(buf)?;
    Some((h, &buf[SERVER_HEADER..]))
}

/// Constant-time token comparison.
pub fn token_eq(a: &[u8; TOKEN_LEN], b: &[u8; TOKEN_LEN]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let h = Header {
            kind: DATA,
            from: 1,
            to: 0,
            session: 0xDEAD_BEEF,
        };
        let t = [7u8; TOKEN_LEN];
        let c = encode_client(h, &t, b"ggrs");
        assert_eq!(c.len(), CLIENT_HEADER + 4);
        let (h2, t2, p) = decode_client(&c).unwrap();
        assert_eq!((h2, t2, p), (h, t, &b"ggrs"[..]));
        let s = encode_server(h, b"x");
        assert_eq!(decode_server(&s).unwrap(), (h, &b"x"[..]));
        assert!(decode_client(b"RG").is_none());
        assert!(decode_client(&s).is_none());
        assert!(decode_server(b"XX123456789").is_none());
        assert!(token_eq(&t, &[7u8; TOKEN_LEN]));
        assert!(!token_eq(&t, &[8u8; TOKEN_LEN]));
        let big = encode_client(h, &t, &[0u8; MAX_PAYLOAD + 1]);
        assert!(decode_client(&big).is_none());
    }
}
