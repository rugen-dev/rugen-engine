//! Minimal STUN (RFC 8489) message codec for the TURN client in
//! [`crate::turn`]: header, attributes, XOR addresses, MESSAGE-INTEGRITY
//! (HMAC-SHA1, long-term credentials), FINGERPRINT and TURN ChannelData
//! framing (RFC 8656 §12). Only what a TURN client needs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use sha1::Sha1;

pub const MAGIC: u32 = 0x2112_A442;
pub const HEADER: usize = 20;
const FINGERPRINT_XOR: u32 = 0x5354_554e;

/// STUN / TURN methods.
pub mod method {
    pub const BINDING: u16 = 0x001;
    pub const ALLOCATE: u16 = 0x003;
    pub const REFRESH: u16 = 0x004;
    pub const SEND: u16 = 0x006;
    pub const DATA: u16 = 0x007;
    pub const CREATE_PERMISSION: u16 = 0x008;
    pub const CHANNEL_BIND: u16 = 0x009;
}

/// Attribute types.
pub mod attr {
    pub const MAPPED_ADDRESS: u16 = 0x0001;
    pub const USERNAME: u16 = 0x0006;
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    pub const ERROR_CODE: u16 = 0x0009;
    pub const CHANNEL_NUMBER: u16 = 0x000C;
    pub const LIFETIME: u16 = 0x000D;
    pub const XOR_PEER_ADDRESS: u16 = 0x0012;
    pub const DATA: u16 = 0x0013;
    pub const REALM: u16 = 0x0014;
    pub const NONCE: u16 = 0x0015;
    pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
    pub const REQUESTED_TRANSPORT: u16 = 0x0019;
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
    pub const SOFTWARE: u16 = 0x8022;
    pub const FINGERPRINT: u16 = 0x8028;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Request,
    Indication,
    Success,
    Error,
}

impl Class {
    fn bits(self) -> u16 {
        match self {
            Class::Request => 0b00,
            Class::Indication => 0b01,
            Class::Success => 0b10,
            Class::Error => 0b11,
        }
    }
}

/// Message type field from method and class (the class bits interleave
/// with the method bits, RFC 8489 §5).
pub fn message_type(method: u16, class: Class) -> u16 {
    let c = class.bits();
    (method & 0x000F)
        | ((method & 0x0070) << 1)
        | ((method & 0x0F80) << 2)
        | ((c & 1) << 4)
        | ((c & 2) << 7)
}

fn split_type(t: u16) -> (u16, Class) {
    let method = (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80);
    let class = match ((t >> 4) & 1) | ((t >> 7) & 2) {
        0 => Class::Request,
        1 => Class::Indication,
        2 => Class::Success,
        _ => Class::Error,
    };
    (method, class)
}

/// Long-term credential key: MD5(username ":" realm ":" password). The
/// password is used as given (Cloudflare / coturn credentials are ASCII,
/// so SASLprep is the identity).
pub fn long_term_key(username: &str, realm: &str, password: &str) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(username.as_bytes());
    h.update(b":");
    h.update(realm.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    h.finalize().into()
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut m = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("hmac takes any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

/// Encodes one STUN message.
pub struct Builder {
    buf: Vec<u8>,
    tid: [u8; 12],
    pad: u8,
}

impl Builder {
    pub fn new(method: u16, class: Class, tid: [u8; 12]) -> Self {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&message_type(method, class).to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(&tid);
        Self { buf, tid, pad: 0 }
    }

    /// Padding byte value (any value is valid; tests reproduce vectors).
    pub fn padding(mut self, b: u8) -> Self {
        self.pad = b;
        self
    }

    fn set_len(&mut self, len: usize) {
        let l = (len as u16).to_be_bytes();
        self.buf[2] = l[0];
        self.buf[3] = l[1];
    }

    pub fn attr(mut self, t: u16, v: &[u8]) -> Self {
        self.buf.extend_from_slice(&t.to_be_bytes());
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v);
        while self.buf.len() % 4 != 0 {
            self.buf.push(self.pad);
        }
        let n = self.buf.len() - HEADER;
        self.set_len(n);
        self
    }

    pub fn u32_attr(self, t: u16, v: u32) -> Self {
        self.attr(t, &v.to_be_bytes())
    }

    pub fn xor_addr(self, t: u16, a: SocketAddr) -> Self {
        let v = xor_addr_value(a, &self.tid);
        self.attr(t, &v)
    }

    /// Append MESSAGE-INTEGRITY keyed with `key`.
    pub fn integrity(mut self, key: &[u8]) -> Self {
        let len = self.buf.len() - HEADER + 24;
        self.set_len(len);
        let mac = hmac_sha1(key, &self.buf);
        self.attr(attr::MESSAGE_INTEGRITY, &mac)
    }

    /// Append FINGERPRINT (CRC-32 of everything before it, XOR'd).
    pub fn fingerprint(mut self) -> Self {
        let len = self.buf.len() - HEADER + 8;
        self.set_len(len);
        let crc = crc32fast::hash(&self.buf) ^ FINGERPRINT_XOR;
        self.u32_attr(attr::FINGERPRINT, crc)
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

fn xor_addr_value(a: SocketAddr, tid: &[u8; 12]) -> Vec<u8> {
    let port = a.port() ^ (MAGIC >> 16) as u16;
    let mut v = Vec::with_capacity(20);
    match a.ip() {
        IpAddr::V4(ip) => {
            v.extend_from_slice(&[0, 1]);
            v.extend_from_slice(&port.to_be_bytes());
            v.extend_from_slice(&(u32::from(ip) ^ MAGIC).to_be_bytes());
        }
        IpAddr::V6(ip) => {
            v.extend_from_slice(&[0, 2]);
            v.extend_from_slice(&port.to_be_bytes());
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&MAGIC.to_be_bytes());
            key[4..].copy_from_slice(tid);
            v.extend(ip.octets().iter().zip(key).map(|(b, k)| b ^ k));
        }
    }
    v
}

fn parse_xor_addr(v: &[u8], tid: &[u8; 12]) -> Option<SocketAddr> {
    if v.len() < 4 {
        return None;
    }
    let port = u16::from_be_bytes([v[2], v[3]]) ^ (MAGIC >> 16) as u16;
    match v[1] {
        1 if v.len() >= 8 => {
            let ip = u32::from_be_bytes([v[4], v[5], v[6], v[7]]) ^ MAGIC;
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port))
        }
        2 if v.len() >= 20 => {
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&MAGIC.to_be_bytes());
            key[4..].copy_from_slice(tid);
            let mut o = [0u8; 16];
            for i in 0..16 {
                o[i] = v[4 + i] ^ key[i];
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(o)), port))
        }
        _ => None,
    }
}

/// A decoded STUN message (attributes kept raw, in order).
#[derive(Clone, Debug)]
pub struct Message {
    pub method: u16,
    pub class: Class,
    pub tid: [u8; 12],
    pub attrs: Vec<(u16, Vec<u8>)>,
}

impl Message {
    pub fn decode(raw: &[u8]) -> Option<Self> {
        if raw.len() < HEADER || raw[0] & 0xC0 != 0 {
            return None;
        }
        let len = u16::from_be_bytes([raw[2], raw[3]]) as usize;
        if u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]) != MAGIC
            || len % 4 != 0
            || raw.len() < HEADER + len
        {
            return None;
        }
        let (method, class) = split_type(u16::from_be_bytes([raw[0], raw[1]]));
        let mut tid = [0u8; 12];
        tid.copy_from_slice(&raw[8..20]);
        let mut attrs = Vec::new();
        let body = &raw[HEADER..HEADER + len];
        let mut i = 0;
        while i + 4 <= body.len() {
            let t = u16::from_be_bytes([body[i], body[i + 1]]);
            let l = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
            let v = body.get(i + 4..i + 4 + l)?;
            attrs.push((t, v.to_vec()));
            i += 4 + l.div_ceil(4) * 4;
        }
        Some(Self {
            method,
            class,
            tid,
            attrs,
        })
    }

    pub fn get(&self, t: u16) -> Option<&[u8]> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == t)
            .map(|(_, v)| v.as_slice())
    }

    pub fn text(&self, t: u16) -> Option<String> {
        self.get(t).map(|v| String::from_utf8_lossy(v).into_owned())
    }

    pub fn u32_attr(&self, t: u16) -> Option<u32> {
        let v = self.get(t)?;
        Some(u32::from_be_bytes(v.get(..4)?.try_into().ok()?))
    }

    pub fn xor_addr(&self, t: u16) -> Option<SocketAddr> {
        parse_xor_addr(self.get(t)?, &self.tid)
    }

    /// ERROR-CODE as (code, reason).
    pub fn error(&self) -> Option<(u16, String)> {
        let v = self.get(attr::ERROR_CODE)?;
        if v.len() < 4 {
            return None;
        }
        let code = (v[2] & 0x07) as u16 * 100 + v[3] as u16;
        Some((code, String::from_utf8_lossy(&v[4..]).into_owned()))
    }
}

/// Byte offset of attribute `t` in a raw message (start of its header).
fn find_attr(raw: &[u8], want: u16) -> Option<usize> {
    let len = u16::from_be_bytes([*raw.get(2)?, *raw.get(3)?]) as usize;
    let end = (HEADER + len).min(raw.len());
    let mut i = HEADER;
    while i + 4 <= end {
        let t = u16::from_be_bytes([raw[i], raw[i + 1]]);
        let l = u16::from_be_bytes([raw[i + 2], raw[i + 3]]) as usize;
        if t == want {
            return Some(i);
        }
        i += 4 + l.div_ceil(4) * 4;
    }
    None
}

/// Check MESSAGE-INTEGRITY of a raw message with `key`.
pub fn check_integrity(raw: &[u8], key: &[u8]) -> bool {
    let Some(at) = find_attr(raw, attr::MESSAGE_INTEGRITY) else {
        return false;
    };
    let Some(mac) = raw.get(at + 4..at + 24) else {
        return false;
    };
    let mut head = raw[..at].to_vec();
    let len = ((at + 24 - HEADER) as u16).to_be_bytes();
    head[2] = len[0];
    head[3] = len[1];
    let want = hmac_sha1(key, &head);
    want.iter().zip(mac).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// Check FINGERPRINT of a raw message (true when absent: it is optional).
pub fn check_fingerprint(raw: &[u8]) -> bool {
    let Some(at) = find_attr(raw, attr::FINGERPRINT) else {
        return true;
    };
    let Some(v) = raw.get(at + 4..at + 8) else {
        return false;
    };
    let mut head = raw[..at].to_vec();
    let len = ((at + 8 - HEADER) as u16).to_be_bytes();
    head[2] = len[0];
    head[3] = len[1];
    crc32fast::hash(&head) ^ FINGERPRINT_XOR == u32::from_be_bytes([v[0], v[1], v[2], v[3]])
}

/// TURN channel numbers clients may bind (RFC 8656 §12).
pub const CHANNEL_MIN: u16 = 0x4000;
pub const CHANNEL_MAX: u16 = 0x4FFF;

/// ChannelData message: channel, length, data (padded to 4 bytes when
/// `pad`, which stream transports require).
pub fn channel_data(channel: u16, data: &[u8], pad: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len() + 3);
    channel_data_into(&mut out, channel, data, pad);
    out
}

pub fn channel_data_into(out: &mut Vec<u8>, channel: u16, data: &[u8], pad: bool) {
    out.extend_from_slice(&channel.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    if pad {
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
}

/// Raw ChannelData → (channel, data).
pub fn parse_channel_data(raw: &[u8]) -> Option<(u16, &[u8])> {
    if raw.len() < 4 {
        return None;
    }
    let ch = u16::from_be_bytes([raw[0], raw[1]]);
    if !(CHANNEL_MIN..=CHANNEL_MAX).contains(&ch) {
        return None;
    }
    let len = u16::from_be_bytes([raw[2], raw[3]]) as usize;
    raw.get(4..4 + len).map(|d| (ch, d))
}

/// Length of the first complete STUN or ChannelData frame in a stream
/// buffer (TCP / TLS transports), or `None` if more bytes are needed.
/// `Some(Err(()))` means the stream is corrupt.
pub fn stream_frame_len(buf: &[u8]) -> Option<Result<usize, ()>> {
    if buf.len() < 4 {
        return None;
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let total = match buf[0] >> 6 {
        0 => HEADER + len,
        1 => 4 + len.div_ceil(4) * 4,
        _ => return Some(Err(())),
    };
    (buf.len() >= total).then_some(Ok(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    const PASSWORD: &str = "VOkJxbRl1RmTxUk/WvJxBt";

    /// RFC 5769 §2.1
    const REQUEST: &str = "000100582112a442b7e7a701bc34d686fa87dfae
        802200105354554e207465737420636c69656e74
        002400046e0001ff80290008932ff9b151263b36
        000600096576746a3a68367659202020
        000800149aeaa70cbfd8cb56781ef2b5b2d3f249c1b571a2
        80280004e57a3bcf";
    /// RFC 5769 §2.2
    const RESPONSE_V4: &str = "0101003c2112a442b7e7a701bc34d686fa87dfae
        8022000b7465737420766563746f7220
        002000080001a147e112a643
        000800142b91f599fd9e90c38c7489f92af9ba53f06be7d7
        80280004c07d4c96";
    /// RFC 5769 §2.3
    const RESPONSE_V6: &str = "010100482112a442b7e7a701bc34d686fa87dfae
        8022000b7465737420766563746f7220
        002000140002a1470113a9faa5d3f179bc25f4b5bed2b9d9
        00080014a382954e4be67bf11784c97c8292c275bfe3ed41
        80280004c8fb0b4c";
    /// RFC 5769 §2.4
    const LONG_TERM: &str = "000100602112a44278ad3433c6ad72c029da412e
        00060012e3839ee38388e383aae38383e382afe382b90000
        0015001c 662f2f34 39396b39 35346436 4f4c3334 6f4c3946 53547679 36347341
        0014000b6578616d706c652e6f726700
        00080014f67024656dd64a3e02b8e0712e85c9a28ca89666";

    #[test]
    fn rfc5769_short_term_request() {
        let raw = hex(REQUEST);
        assert!(check_integrity(&raw, PASSWORD.as_bytes()));
        assert!(!check_integrity(&raw, b"wrong"));
        assert!(check_fingerprint(&raw));
        let m = Message::decode(&raw).unwrap();
        assert_eq!((m.method, m.class), (method::BINDING, Class::Request));
        assert_eq!(m.text(attr::USERNAME).unwrap(), "evtj:h6vY");
        assert_eq!(m.text(attr::SOFTWARE).unwrap(), "STUN test client");
        // the encoder reproduces it byte for byte (spaces as padding)
        let tid: [u8; 12] = raw[8..20].try_into().unwrap();
        let built = Builder::new(method::BINDING, Class::Request, tid)
            .padding(b' ')
            .attr(attr::SOFTWARE, b"STUN test client")
            .attr(0x0024, &hex("6e0001ff"))
            .attr(0x8029, &hex("932ff9b151263b36"))
            .attr(attr::USERNAME, b"evtj:h6vY")
            .integrity(PASSWORD.as_bytes())
            .fingerprint()
            .finish();
        assert_eq!(built, raw);
    }

    #[test]
    fn rfc5769_responses() {
        let v4 = hex(RESPONSE_V4);
        assert!(check_integrity(&v4, PASSWORD.as_bytes()) && check_fingerprint(&v4));
        let m = Message::decode(&v4).unwrap();
        assert_eq!(m.class, Class::Success);
        assert_eq!(
            m.xor_addr(attr::XOR_MAPPED_ADDRESS).unwrap(),
            "192.0.2.1:32853".parse().unwrap()
        );
        let v6 = hex(RESPONSE_V6);
        assert!(check_integrity(&v6, PASSWORD.as_bytes()) && check_fingerprint(&v6));
        let m = Message::decode(&v6).unwrap();
        assert_eq!(
            m.xor_addr(attr::XOR_MAPPED_ADDRESS).unwrap(),
            "[2001:db8:1234:5678:11:2233:4455:6677]:32853"
                .parse()
                .unwrap()
        );
        // XOR address encoding round-trips through the builder
        let tid: [u8; 12] = v6[8..20].try_into().unwrap();
        let a: SocketAddr = "[2001:db8:1234:5678:11:2233:4455:6677]:32853"
            .parse()
            .unwrap();
        let b = Builder::new(method::BINDING, Class::Success, tid)
            .xor_addr(attr::XOR_MAPPED_ADDRESS, a)
            .finish();
        assert_eq!(&b[20..44], &v6[36..60]);
        // a flipped bit breaks the fingerprint
        let mut bad = v4.clone();
        bad[30] ^= 1;
        assert!(!check_fingerprint(&bad));
    }

    #[test]
    fn rfc5769_long_term_auth() {
        let raw = hex(LONG_TERM);
        let user = "\u{30DE}\u{30C8}\u{30EA}\u{30C3}\u{30AF}\u{30B9}";
        let key = long_term_key(user, "example.org", "TheMatrIX");
        assert!(check_integrity(&raw, &key));
        let m = Message::decode(&raw).unwrap();
        assert_eq!(m.text(attr::NONCE).unwrap(), "f//499k954d6OL34oL9FSTvy64sA");
        assert_eq!(m.text(attr::REALM).unwrap(), "example.org");
        let tid: [u8; 12] = raw[8..20].try_into().unwrap();
        let built = Builder::new(method::BINDING, Class::Request, tid)
            .attr(attr::USERNAME, user.as_bytes())
            .attr(attr::NONCE, b"f//499k954d6OL34oL9FSTvy64sA")
            .attr(attr::REALM, b"example.org")
            .integrity(&key)
            .finish();
        assert_eq!(built, raw);
    }

    #[test]
    fn message_types_and_errors() {
        assert_eq!(message_type(method::BINDING, Class::Request), 0x0001);
        assert_eq!(message_type(method::BINDING, Class::Success), 0x0101);
        assert_eq!(message_type(method::ALLOCATE, Class::Error), 0x0113);
        assert_eq!(message_type(method::DATA, Class::Indication), 0x0017);
        assert_eq!(message_type(method::CHANNEL_BIND, Class::Success), 0x0109);
        for m in [1u16, 3, 4, 6, 7, 8, 9, 0xABC] {
            for c in [
                Class::Request,
                Class::Indication,
                Class::Success,
                Class::Error,
            ] {
                assert_eq!(split_type(message_type(m, c)), (m, c));
            }
        }
        let e = Builder::new(method::ALLOCATE, Class::Error, [9; 12])
            .attr(attr::ERROR_CODE, &[0, 0, 4, 1, b'U', b'n'])
            .finish();
        let m = Message::decode(&e).unwrap();
        assert_eq!(m.error().unwrap(), (401, "Un".into()));
        assert!(Message::decode(&e[..19]).is_none());
    }

    #[test]
    fn channel_data_framing() {
        let d = channel_data(0x4001, b"hello", false);
        assert_eq!(d, [0x40, 0x01, 0, 5, b'h', b'e', b'l', b'l', b'o']);
        assert_eq!(parse_channel_data(&d).unwrap(), (0x4001, &b"hello"[..]));
        let p = channel_data(0x4001, b"hello", true);
        assert_eq!(p.len(), 12);
        assert_eq!(parse_channel_data(&p).unwrap().1, b"hello");
        assert!(parse_channel_data(&[0x30, 0, 0, 0]).is_none());
        assert!(parse_channel_data(&[0x40, 0, 0, 9, 1]).is_none());
        // stream framing: a padded ChannelData then a STUN message
        let stun = Builder::new(method::REFRESH, Class::Success, [1; 12])
            .u32_attr(attr::LIFETIME, 600)
            .finish();
        let mut s = p.clone();
        s.extend(&stun);
        assert_eq!(stream_frame_len(&s), Some(Ok(12)));
        assert_eq!(stream_frame_len(&s[12..]), Some(Ok(stun.len())));
        assert_eq!(stream_frame_len(&s[12..20]), None);
        assert_eq!(stream_frame_len(&[0xC0, 0, 0, 0]), Some(Err(())));
    }
}
