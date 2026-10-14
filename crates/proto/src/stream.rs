//! One match format: a header and the confirmed input stream.
//!
//! A live match, a match a late viewer catches up on and a shared replay
//! are the same thing: a [`MatchHeader`] (what to load and how to
//! simulate it) and the inputs of both players per frame. The simulation
//! is deterministic, so the inputs reproduce the match exactly.
//!
//! Saved replays stay JSON (the game's `replays/*.json`); on the wire the
//! stream is compact binary, 2 bytes per player and frame (about 14 KB a
//! minute), in [`Chunk`]s:
//!
//! ```text
//! "RGM1"  flags:u8 (1 = the match ended, 2 = a header follows)
//! total:u32le  frames the server holds
//! from:u32le   first frame of this chunk
//! count:u32le  frames in this chunk
//! [len:u32le header JSON]          when flags & 2
//! count × (p1:u16le, p2:u16le)     the inputs (`Buttons::bits`)
//! ```
//!
//! Players feed the server through their relay session ([`crate::relay`]
//! `STREAM` / `STREAM_ACK`); viewers read chunks over HTTP.

use serde::{Deserialize, Serialize};

use crate::Rules;

/// Version of [`MatchHeader`].
pub const STREAM_VERSION: u32 = 1;
pub const MAGIC: [u8; 4] = *b"RGM1";
/// Frames a server keeps of one match (15 minutes); longer matches stop
/// being kept (the players are unaffected).
pub const MAX_FRAMES: usize = 60 * 60 * 15;
/// Frames of one `STREAM` datagram at most (the relay's payload limit).
pub const DATAGRAM_FRAMES: usize = (crate::relay::MAX_PAYLOAD - 4) / 4;

/// The seed of an online match, from the server's match id: both peers,
/// spectators, stored replays and `rugen-server`'s re-simulation derive the
/// same one without exchanging anything (SplitMix64 of the id, so
/// consecutive ids give unrelated seeds; never 0).
pub fn online_seed(match_id: u64) -> u64 {
    let mut z = match_id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)).max(1)
}

/// A fighter or the stage of a streamed match: its content fingerprint
/// (fetched through the package sync when missing), the name its player
/// declared and the package size.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamContent {
    pub fingerprint: String,
    pub name: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamPlayer {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub verified: bool,
}

/// Everything a viewer needs besides the inputs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MatchHeader {
    pub v: u32,
    pub match_id: u64,
    /// The game version both players ran (`rugen 0.7.0`): a viewer of
    /// another version may simulate it differently.
    pub engine: String,
    /// The match seed ([`online_seed`] of the id).
    pub seed: u64,
    /// Rounds, round time, mode and fighter size the match was played with.
    pub rules: Rules,
    pub fighters: [StreamContent; 2],
    pub stage: StreamContent,
    pub players: [StreamPlayer; 2],
    #[serde(default)]
    pub ranked: bool,
    /// Unix time the match started.
    #[serde(default)]
    pub created: u64,
}

impl MatchHeader {
    /// "NARUTO VS SASUKE"
    pub fn title(&self) -> String {
        format!(
            "{} VS {}",
            self.fighters[0].name.to_uppercase(),
            self.fighters[1].name.to_uppercase()
        )
    }
}

/// A piece of a match stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chunk {
    pub header: Option<MatchHeader>,
    /// The match is over: no frame will follow `total`.
    pub ended: bool,
    /// Frames the server holds.
    pub total: u32,
    /// First frame of `inputs`.
    pub from: u32,
    /// `[p1 bits, p2 bits]` per frame.
    pub inputs: Vec<[u16; 2]>,
}

pub fn encode_chunk(c: &Chunk) -> Vec<u8> {
    let header = c.header.as_ref().and_then(|h| serde_json::to_vec(h).ok());
    let mut out =
        Vec::with_capacity(17 + header.as_ref().map_or(0, |h| h.len() + 4) + c.inputs.len() * 4);
    out.extend_from_slice(&MAGIC);
    out.push(u8::from(c.ended) | if header.is_some() { 2 } else { 0 });
    out.extend_from_slice(&c.total.to_le_bytes());
    out.extend_from_slice(&c.from.to_le_bytes());
    out.extend_from_slice(&(c.inputs.len() as u32).to_le_bytes());
    if let Some(h) = header {
        out.extend_from_slice(&(h.len() as u32).to_le_bytes());
        out.extend_from_slice(&h);
    }
    out.extend_from_slice(&inputs_to_bytes(&c.inputs));
    out
}

pub fn decode_chunk(b: &[u8]) -> Option<Chunk> {
    if b.len() < 17 || b[..4] != MAGIC {
        return None;
    }
    let u32_at =
        |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let flags = b[4];
    let total = u32_at(5)?;
    let from = u32_at(9)?;
    let count = u32_at(13)? as usize;
    let mut at = 17;
    let header = if flags & 2 != 0 {
        let len = u32_at(at)? as usize;
        let json = b.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        Some(serde_json::from_slice(json).ok()?)
    } else {
        None
    };
    if count > MAX_FRAMES || b.len() != at + count * 4 {
        return None;
    }
    Some(Chunk {
        header,
        ended: flags & 1 != 0,
        total,
        from,
        inputs: inputs_from_bytes(&b[at..]),
    })
}

/// 4 bytes per frame (P1 then P2, little endian): the evidence format.
pub fn inputs_to_bytes(inputs: &[[u16; 2]]) -> Vec<u8> {
    inputs
        .iter()
        .flat_map(|[a, b]| {
            let [a0, a1] = a.to_le_bytes();
            let [b0, b1] = b.to_le_bytes();
            [a0, a1, b0, b1]
        })
        .collect()
}

pub fn inputs_from_bytes(b: &[u8]) -> Vec<[u16; 2]> {
    b.chunks_exact(4)
        .map(|c| {
            [
                u16::from_le_bytes([c[0], c[1]]),
                u16::from_le_bytes([c[2], c[3]]),
            ]
        })
        .collect()
}

/// `STREAM` datagram payload: the first frame, then the frames.
pub fn encode_upload(from: u32, inputs: &[[u16; 2]]) -> Vec<u8> {
    let mut p = from.to_le_bytes().to_vec();
    p.extend(inputs_to_bytes(
        &inputs[..inputs.len().min(DATAGRAM_FRAMES)],
    ));
    p
}

pub fn decode_upload(p: &[u8]) -> Option<(u32, Vec<[u16; 2]>)> {
    let from = u32::from_le_bytes(p.get(..4)?.try_into().ok()?);
    let rest = &p[4..];
    (rest.len() % 4 == 0).then(|| (from, inputs_from_bytes(rest)))
}

/// A live match a viewer can open (`GET /v1/live`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct LiveMatch {
    pub match_id: u64,
    pub players: [String; 2],
    pub fighters: [String; 2],
    pub stage: String,
    #[serde(default)]
    pub ranked: bool,
    /// Frames played so far.
    pub frames: u32,
    /// The room it is played in (public rooms only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// Viewers following it.
    #[serde(default)]
    pub viewers: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> MatchHeader {
        MatchHeader {
            v: STREAM_VERSION,
            match_id: 77,
            engine: "rugen 0.7.0".into(),
            seed: online_seed(77),
            fighters: [
                StreamContent {
                    fingerprint: "a".repeat(64),
                    name: "Naruto".into(),
                    size: 10,
                },
                StreamContent {
                    fingerprint: "b".repeat(64),
                    name: "Sasuke".into(),
                    size: 20,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn chunks_round_trip_compactly() {
        let inputs: Vec<[u16; 2]> = (0..600).map(|i| [i as u16, 0x7ff - i as u16]).collect();
        let c = Chunk {
            header: Some(header()),
            ended: false,
            total: 900,
            from: 300,
            inputs: inputs.clone(),
        };
        let b = encode_chunk(&c);
        assert_eq!(decode_chunk(&b), Some(c.clone()));
        // 2 bytes per player per frame, plus the header once
        let bare = encode_chunk(&Chunk {
            header: None,
            ended: true,
            ..c.clone()
        });
        assert_eq!(bare.len(), 17 + 600 * 4);
        let back = decode_chunk(&bare).unwrap();
        assert!(back.ended && back.header.is_none());
        assert_eq!(back.inputs, inputs);
        // garbage and truncation never decode
        assert!(decode_chunk(b"RGM0aaaaaaaaaaaaaaaa").is_none());
        assert!(decode_chunk(&bare[..bare.len() - 1]).is_none());
        assert!(decode_chunk(&b[..30]).is_none());
        assert_eq!(header().title(), "NARUTO VS SASUKE");
    }

    #[test]
    fn uploads_fit_a_datagram() {
        let inputs: Vec<[u16; 2]> = (0..1000).map(|i| [i as u16, 1]).collect();
        let p = encode_upload(42, &inputs);
        assert!(p.len() <= crate::relay::MAX_PAYLOAD);
        let (from, got) = decode_upload(&p).unwrap();
        assert_eq!((from, got.len()), (42, DATAGRAM_FRAMES));
        assert_eq!(got[..], inputs[..DATAGRAM_FRAMES]);
        assert!(decode_upload(&[1, 2]).is_none());
        assert!(decode_upload(&[0, 0, 0, 0, 1]).is_none());
    }

    #[test]
    fn the_seed_is_the_simulations() {
        assert_eq!(online_seed(0), 0xE220_A839_7B1D_CDAF);
        assert_ne!(online_seed(1), online_seed(2));
        assert!((0..1000).all(|i| online_seed(i) != 0));
    }
}
