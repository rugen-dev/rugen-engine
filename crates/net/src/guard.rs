//! What the other side sends is checked before ggrs sees it.
//!
//! ggrs 0.11 trusts its peer: a message with a field out of range ends in
//! an `assert!`, an index out of bounds or an `expect`, and the game with
//! it. One datagram was enough to crash an opponent (and win a ranked
//! match by it). [`Guard`] wraps the session's socket and lets through
//! only messages ggrs can process:
//!
//! - an input message names every player's status (ggrs indexes them),
//!   starts where the last one ended or before (ggrs asserts it), decodes
//!   (ggrs `expect`s it) to whole inputs, to a bounded number of them, and
//!   does not run further ahead of this side than ggrs' input queue holds;
//! - a quality reply echoes a time that is not in the future (ggrs asserts
//!   it) and not so old that the round trip overflows;
//! - a checksum report is for a frame that can exist;
//! - frames are never negative or near `i32::MAX`.
//!
//! To know where "the last one ended", the guard keeps the same record of
//! received inputs as ggrs does (the inputs are delta-encoded against it),
//! and follows the synchronization handshake to know the magic number ggrs
//! filters on, so both always agree on which messages count.
//!
//! The wire format is not changed: a message that passes is handed over as
//! it came, or with its reported statuses made harmless (see [`Guard`]).
//! A message that does not pass is dropped, as if lost: for an honest peer
//! none ever is.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

use ggrs::NonBlockingSocket;
use serde::{Deserialize, Serialize};

/// ggrs' "no frame".
const NULL_FRAME: i32 = -1;
/// No frame is beyond this (a match at 60 Hz for 100 days): keeps every
/// `frame + n` of ggrs far from overflow.
const MAX_FRAME: i32 = i32::MAX / 4;
/// ggrs: sync round trips before an endpoint runs (`NUM_SYNC_PACKETS`).
const SYNC_ROUNDTRIPS: u32 = 5;
/// The encoded inputs of one message are never longer (ggrs holds at most
/// 128 pending inputs; they compress to a few bytes each).
const MAX_ENCODED: usize = 4096;
/// The inputs one message may carry (ggrs: `PENDING_OUTPUT_SIZE`).
const MAX_INPUTS: usize = 128;
/// A quality reply older than this is dropped (ms).
const MAX_PONG_AGE: u128 = 60_000;
/// A checksum report this far behind the newest is dropped (frames).
const CHECKSUM_WINDOW: i32 = 4096;

// ggrs' messages, field for field (its own types are private). bincode
// writes no names, so these read and write the same bytes.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Status {
    pub(crate) disconnected: bool,
    pub(crate) last_frame: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Input {
    pub(crate) statuses: Vec<Status>,
    pub(crate) disconnect_requested: bool,
    pub(crate) start_frame: i32,
    pub(crate) ack_frame: i32,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Body {
    SyncRequest(u32),
    SyncReply(u32),
    Input(Input),
    InputAck(i32),
    QualityReport { frame_advantage: i16, ping: u128 },
    QualityReply(u128),
    ChecksumReport { checksum: u128, frame: i32 },
    KeepAlive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Msg {
    pub(crate) magic: u16,
    pub(crate) body: Body,
}

impl Msg {
    fn of(msg: &ggrs::Message) -> Option<Self> {
        bincode::deserialize(&bincode::serialize(msg).ok()?).ok()
    }

    fn into_ggrs(self) -> Option<ggrs::Message> {
        bincode::deserialize(&bincode::serialize(&self).ok()?).ok()
    }
}

/// Who an address is, in this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The other player: sends its own inputs.
    Player,
    /// Someone watching us: never sends inputs.
    Watcher,
    /// The host we are watching: sends every player's inputs.
    Host,
}

/// What the guard has to know of the session it protects.
#[derive(Clone, Debug)]
pub(crate) struct Rules {
    pub num_players: usize,
    /// Our player (None when we only watch).
    pub local_handle: Option<usize>,
    pub max_prediction: usize,
    /// Bytes of one player's input on the wire.
    pub input_size: usize,
    pub peers: Vec<(SocketAddr, Role)>,
}

impl Rules {
    /// How far past our own newest input the other player's may run. An
    /// honest peer stops `max_prediction` frames after the last input it
    /// has from us; ggrs' input queue holds 128.
    fn lead(&self) -> i32 {
        (self.max_prediction as i32 * 2 + 32).min(96)
    }
}

struct Peer {
    role: Role,
    /// Sync requests sent to it and not answered yet.
    asked: HashSet<u32>,
    roundtrips_left: u32,
    /// The magic ggrs filters on once synchronized (0: not yet).
    magic: u16,
    /// The inputs received from it, by frame (the same record as ggrs').
    recv: HashMap<i32, Vec<u8>>,
    newest_checksum: i32,
    dropped: u64,
}

/// The session's socket, with every received message checked (see the
/// module). Statuses a player reports of others are passed with their
/// frames in range and, in a two-machine match, without "disconnected":
/// there the only player it can report gone is itself, which ggrs learns
/// from the disconnect request or from silence, and a false report made
/// ggrs roll back to a frame it no longer holds.
pub(crate) struct Guard<S> {
    inner: S,
    rules: Rules,
    peers: HashMap<SocketAddr, Peer>,
    /// The frame of our newest input sent.
    local_frame: i32,
}

impl<S> Guard<S> {
    pub(crate) fn new(inner: S, rules: Rules) -> Self {
        let peers = rules
            .peers
            .iter()
            .map(|(addr, role)| {
                let players = match role {
                    Role::Host => rules.num_players,
                    Role::Player | Role::Watcher => 1,
                };
                let mut recv = HashMap::new();
                recv.insert(NULL_FRAME, vec![0u8; rules.input_size * players]);
                (
                    *addr,
                    Peer {
                        role: *role,
                        asked: HashSet::new(),
                        roundtrips_left: SYNC_ROUNDTRIPS,
                        magic: 0,
                        recv,
                        newest_checksum: NULL_FRAME,
                        dropped: 0,
                    },
                )
            })
            .collect();
        Self {
            inner,
            rules,
            peers,
            local_frame: NULL_FRAME,
        }
    }

    /// What we send tells the guard what ggrs expects back.
    fn sent(&mut self, msg: &ggrs::Message, to: &SocketAddr) {
        let Some(m) = Msg::of(msg) else { return };
        match m.body {
            Body::SyncRequest(r) => {
                if let Some(p) = self.peers.get_mut(to) {
                    p.asked.insert(r);
                }
            }
            Body::Input(i) => {
                if let Some(s) = self.rules.local_handle.and_then(|h| i.statuses.get(h)) {
                    self.local_frame = self.local_frame.max(s.last_frame);
                }
            }
            _ => {}
        }
    }

    /// The message as ggrs may see it, or None to drop it.
    fn check(&mut self, from: &SocketAddr, msg: ggrs::Message) -> Option<ggrs::Message> {
        let lead = self.rules.lead();
        let local_frame = self.local_frame.max(0);
        let peer = self.peers.get_mut(from)?;
        let verdict = Msg::of(&msg).and_then(|m| judge(&self.rules, peer, m, lead, local_frame));
        match verdict {
            Some(Verdict::AsIs) => Some(msg),
            Some(Verdict::Changed(m)) => m.into_ggrs(),
            None => {
                peer.dropped += 1;
                if peer.dropped <= 3 || peer.dropped.is_power_of_two() {
                    log::warn!(
                        "netplay: dropped a message from {from} that the session cannot process ({} so far)",
                        peer.dropped
                    );
                }
                None
            }
        }
    }
}

enum Verdict {
    AsIs,
    Changed(Msg),
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

fn judge(rules: &Rules, peer: &mut Peer, m: Msg, lead: i32, local_frame: i32) -> Option<Verdict> {
    // once synchronized, ggrs ignores any other magic: so do we, before
    // the message can change what we know
    if peer.magic != 0 && m.magic != peer.magic {
        return None;
    }
    match m.body {
        Body::SyncRequest(_) | Body::KeepAlive | Body::QualityReport { .. } => Some(Verdict::AsIs),
        Body::SyncReply(r) => {
            if peer.magic == 0 && peer.asked.remove(&r) {
                peer.roundtrips_left = peer.roundtrips_left.saturating_sub(1);
                if peer.roundtrips_left == 0 {
                    // 0 is "not yet" for ggrs too: such a peer never syncs
                    peer.magic = m.magic;
                }
            }
            Some(Verdict::AsIs)
        }
        Body::InputAck(_) => Some(Verdict::AsIs),
        Body::QualityReply(pong) => {
            let now = now_ms();
            (pong <= now && now - pong <= MAX_PONG_AGE).then_some(Verdict::AsIs)
        }
        Body::ChecksumReport { frame, .. } => {
            if !(0..=MAX_FRAME).contains(&frame)
                || frame < peer.newest_checksum.saturating_sub(CHECKSUM_WINDOW)
            {
                return None;
            }
            peer.newest_checksum = peer.newest_checksum.max(frame);
            Some(Verdict::AsIs)
        }
        Body::Input(input) => judge_input(rules, peer, m.magic, input, lead, local_frame),
    }
}

fn judge_input(
    rules: &Rules,
    peer: &mut Peer,
    magic: u16,
    mut input: Input,
    lead: i32,
    local_frame: i32,
) -> Option<Verdict> {
    // someone watching has no input to give (ggrs asserts on one)
    if peer.role == Role::Watcher {
        return None;
    }
    // ggrs reads one status per player
    if input.statuses.len() < rules.num_players || input.statuses.len() > 16 {
        return None;
    }
    if input.bytes.len() > MAX_ENCODED || !(0..=MAX_FRAME).contains(&input.start_frame) {
        return None;
    }
    let before = input.statuses.clone();
    for s in &mut input.statuses {
        s.last_frame = s.last_frame.clamp(NULL_FRAME, MAX_FRAME);
        if peer.role == Role::Player {
            s.disconnected = false;
        }
    }
    let changed = before != input.statuses;

    let last = peer.recv.keys().copied().max().unwrap_or(NULL_FRAME);
    // inputs are deltas of the one before: a gap cannot be decoded
    if last != NULL_FRAME && last + 1 < input.start_frame {
        return None;
    }
    let decode_frame = if last == NULL_FRAME {
        NULL_FRAME
    } else {
        input.start_frame - 1
    };
    if let Some(reference) = peer.recv.get(&decode_frame) {
        let size = reference.len();
        let plain = rle_decode(&input.bytes, MAX_INPUTS * size)?;
        if size == 0 || plain.len() % size != 0 {
            return None;
        }
        let count = (plain.len() / size) as i32;
        let newest = input.start_frame + count - 1;
        if peer.role == Role::Player {
            // the first input is for frame 0, or the input delay
            if last == NULL_FRAME && input.start_frame > lead {
                return None;
            }
            if newest > local_frame + lead {
                return None;
            }
        }
        let reference = reference.clone();
        for (i, chunk) in plain.chunks_exact(size).enumerate() {
            let frame = input.start_frame + i as i32;
            if frame <= last {
                continue;
            }
            let bytes = chunk.iter().zip(&reference).map(|(a, b)| a ^ b).collect();
            peer.recv.insert(frame, bytes);
        }
        let last = peer.recv.keys().copied().max().unwrap_or(NULL_FRAME);
        let keep = last - 2 * rules.max_prediction as i32;
        peer.recv.retain(|k, _| *k >= keep);
    }
    Some(if changed {
        Verdict::Changed(Msg {
            magic,
            body: Body::Input(input),
        })
    } else {
        Verdict::AsIs
    })
}

/// One LEB128 number at `at`: its value and length. None when it runs off
/// the end or is longer than any length this guard accepts.
fn varint(buf: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for i in 0..5 {
        let byte = *buf.get(at + i)?;
        value |= u64::from(byte & 127) << (7 * i);
        if byte & 128 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Decode a `bitfield-rle` buffer as ggrs does, but refusing what makes
/// that decoder panic or allocate without bound: a number cut short, a run
/// past the end of the buffer, more than `max` bytes out.
fn rle_decode(buf: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let (next, used) = varint(buf, at)?;
        at += used;
        let repeat = next & 1 == 1;
        let len = usize::try_from(if repeat { next >> 2 } else { next >> 1 }).ok()?;
        if len > max - out.len() {
            return None;
        }
        if repeat {
            let fill = if next & 2 == 2 { 255 } else { 0 };
            out.resize(out.len() + len, fill);
        } else {
            out.extend_from_slice(buf.get(at..at + len)?);
            at += len;
        }
    }
    Some(out)
}

impl<S: NonBlockingSocket<SocketAddr>> NonBlockingSocket<SocketAddr> for Guard<S> {
    fn send_to(&mut self, msg: &ggrs::Message, addr: &SocketAddr) {
        self.sent(msg, addr);
        self.inner.send_to(msg, addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, ggrs::Message)> {
        let all = self.inner.receive_all_messages();
        all.into_iter()
            .filter_map(|(from, msg)| self.check(&from, msg).map(|m| (from, m)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Wire {
        inbox: Vec<(SocketAddr, ggrs::Message)>,
        sent: Vec<(SocketAddr, ggrs::Message)>,
    }

    impl NonBlockingSocket<SocketAddr> for Wire {
        fn send_to(&mut self, msg: &ggrs::Message, addr: &SocketAddr) {
            self.sent.push((*addr, msg.clone()));
        }
        fn receive_all_messages(&mut self) -> Vec<(SocketAddr, ggrs::Message)> {
            std::mem::take(&mut self.inbox)
        }
    }

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], n))
    }

    fn guard(role: Role) -> Guard<Wire> {
        Guard::new(
            Wire {
                inbox: Vec::new(),
                sent: Vec::new(),
            },
            Rules {
                num_players: 2,
                local_handle: (role != Role::Host).then_some(0),
                max_prediction: 8,
                input_size: 2,
                peers: vec![(addr(1), role)],
            },
        )
    }

    fn msg(body: Body) -> ggrs::Message {
        Msg { magic: 7, body }.into_ggrs().unwrap()
    }

    /// `bitfield-rle` of plain bytes, runs not compressed.
    fn rle_plain(bytes: &[u8]) -> Vec<u8> {
        if bytes.is_empty() {
            return Vec::new();
        }
        let mut n = (bytes.len() as u64) << 1;
        let mut out = Vec::new();
        while n > 127 {
            out.push(n as u8 | 128);
            n >>= 7;
        }
        out.push(n as u8);
        out.extend_from_slice(bytes);
        out
    }

    fn input(start: i32, frames: &[[u8; 2]]) -> Input {
        // deltas against the frame before `start`, which these tests keep
        // at zero: the plain bytes are the inputs
        let plain: Vec<u8> = frames.iter().flatten().copied().collect();
        Input {
            statuses: vec![
                Status {
                    disconnected: false,
                    last_frame: NULL_FRAME,
                };
                2
            ],
            disconnect_requested: false,
            start_frame: start,
            ack_frame: NULL_FRAME,
            bytes: rle_plain(&plain),
        }
    }

    fn passes(g: &mut Guard<Wire>, body: Body) -> bool {
        g.check(&addr(1), msg(body)).is_some()
    }

    #[test]
    fn the_mirror_types_are_ggrs_bytes() {
        // every kind survives the trip into ggrs' type and back
        for body in [
            Body::SyncRequest(9),
            Body::SyncReply(9),
            Body::Input(input(0, &[[1, 2]])),
            Body::InputAck(4),
            Body::QualityReport {
                frame_advantage: -3,
                ping: 12,
            },
            Body::QualityReply(12),
            Body::ChecksumReport {
                checksum: 5,
                frame: 60,
            },
            Body::KeepAlive,
        ] {
            let m = Msg {
                magic: 7,
                body: body.clone(),
            };
            let g = m.clone().into_ggrs().expect("ggrs reads it");
            assert_eq!(Msg::of(&g), Some(m));
        }
    }

    #[test]
    fn a_pong_from_the_future_is_dropped() {
        let mut g = guard(Role::Player);
        assert!(!passes(&mut g, Body::QualityReply(u128::MAX)));
        assert!(!passes(&mut g, Body::QualityReply(now_ms() + 60_000)));
        // so old that ggrs' ping would not fit an i32
        assert!(!passes(&mut g, Body::QualityReply(0)));
        assert!(passes(&mut g, Body::QualityReply(now_ms() - 30)));
    }

    #[test]
    fn an_input_without_a_status_per_player_is_dropped() {
        let mut g = guard(Role::Player);
        for n in [0, 1] {
            let mut i = input(0, &[[0, 0]]);
            i.statuses.truncate(n);
            assert!(!passes(&mut g, Body::Input(i)), "{n} statuses");
        }
        let mut i = input(0, &[[0, 0]]);
        i.statuses = vec![i.statuses[0]; 1000];
        assert!(!passes(&mut g, Body::Input(i)));
        assert!(passes(&mut g, Body::Input(input(0, &[[0, 0]]))));
    }

    #[test]
    fn inputs_that_do_not_decode_are_dropped() {
        let mut g = guard(Role::Player);
        let bad: [&[u8]; 6] = [
            // a number cut short
            &[0x80],
            // a run of plain bytes longer than the buffer
            &[20 << 1, 1, 2],
            // a repeat of 2^33 bytes
            &[0x81, 0x80, 0x80, 0x80, 0x80, 0x01],
            // a repeat of a million bytes
            &[0x81, 0x89, 0x7a],
            // half an input
            &[1 << 1, 5],
            // one byte more than whole inputs
            &[3 << 1, 5, 6, 7],
        ];
        for bytes in bad {
            let mut i = input(0, &[]);
            i.bytes = bytes.to_vec();
            assert!(!passes(&mut g, Body::Input(i)), "{bytes:?}");
        }
        let mut i = input(0, &[]);
        i.bytes = vec![0; MAX_ENCODED + 1];
        assert!(!passes(&mut g, Body::Input(i)));
        // and nothing of those was taken for received
        assert!(passes(&mut g, Body::Input(input(0, &[[0, 0]]))));
    }

    #[test]
    fn inputs_follow_each_other() {
        let mut g = guard(Role::Player);
        assert!(passes(&mut g, Body::Input(input(0, &[[0, 0], [0, 0]]))));
        // a gap: frame 2 is missing (ggrs asserts)
        assert!(!passes(&mut g, Body::Input(input(3, &[[0, 0]]))));
        // the same again, and the next
        assert!(passes(&mut g, Body::Input(input(0, &[[0, 0], [0, 0]]))));
        assert!(passes(&mut g, Body::Input(input(2, &[[0, 0]]))));
        for frame in [-1, -5, i32::MIN, i32::MAX, MAX_FRAME + 1] {
            assert!(!passes(&mut g, Body::Input(input(frame, &[[0, 0]]))));
        }
    }

    #[test]
    fn a_first_input_far_ahead_is_dropped() {
        // ggrs fills its queue of 128 from frame 0 up to it
        let mut g = guard(Role::Player);
        assert!(!passes(&mut g, Body::Input(input(1000, &[[0, 0]]))));
        assert!(!passes(&mut g, Body::Input(input(49, &[[0, 0]]))));
        // the input delay of the other side
        assert!(passes(&mut g, Body::Input(input(3, &[[0, 0]]))));
    }

    #[test]
    fn inputs_do_not_run_ahead_of_ours() {
        let mut g = guard(Role::Player);
        let lead = g.rules.lead();
        let mut next = 0;
        let mut feed = |g: &mut Guard<Wire>, n: i32| {
            let frames = vec![[0u8, 0u8]; n as usize];
            let ok = passes(g, Body::Input(input(next, &frames)));
            if ok {
                next += n;
            }
            ok
        };
        assert!(feed(&mut g, lead));
        assert!(feed(&mut g, 1));
        // frame lead + 1 while we are at frame 0
        assert!(!feed(&mut g, 1));
        // we send our input for frame 10: ten more may come
        let mut ours = input(10, &[[0, 0]]);
        ours.statuses[0].last_frame = 10;
        g.send_to(&msg(Body::Input(ours)), &addr(1));
        assert!(feed(&mut g, 10));
        assert!(!feed(&mut g, 1));
    }

    #[test]
    fn a_host_may_be_far_ahead_of_a_watcher() {
        let mut g = guard(Role::Host);
        let mut i = input(0, &[]);
        // both players' inputs, 100 frames
        i.bytes = vec![
            // a repeat of 400 zeros: 1601 as a two-byte number
            0xc1, 0x0c,
        ];
        assert!(passes(&mut g, Body::Input(i)));
        // half a frame of two players is refused
        let mut g = guard(Role::Host);
        assert!(!passes(&mut g, Body::Input(input(0, &[[0, 0]]))));
    }

    #[test]
    fn someone_watching_sends_no_input() {
        let mut g = guard(Role::Watcher);
        assert!(!passes(&mut g, Body::Input(input(0, &[[0, 0]]))));
        assert!(passes(&mut g, Body::InputAck(3)));
    }

    #[test]
    fn reported_statuses_are_made_harmless() {
        let mut g = guard(Role::Player);
        let mut i = input(0, &[[0, 0]]);
        i.statuses[0] = Status {
            disconnected: true,
            last_frame: i32::MIN,
        };
        i.statuses[1] = Status {
            disconnected: true,
            last_frame: i32::MAX,
        };
        let out = g.check(&addr(1), msg(Body::Input(i))).expect("passes");
        let Some(Msg {
            body: Body::Input(i),
            ..
        }) = Msg::of(&out)
        else {
            panic!("an input")
        };
        assert_eq!(
            i.statuses,
            vec![
                Status {
                    disconnected: false,
                    last_frame: NULL_FRAME
                },
                Status {
                    disconnected: false,
                    last_frame: MAX_FRAME
                },
            ]
        );
        // a host's report of who left is kept for those watching
        let mut g = guard(Role::Host);
        let mut i = input(0, &[[0, 0], [0, 0]]);
        i.statuses[1].disconnected = true;
        let out = g.check(&addr(1), msg(Body::Input(i.clone()))).unwrap();
        assert_eq!(Msg::of(&out).map(|m| m.body), Some(Body::Input(i)));
    }

    #[test]
    fn checksum_frames_exist() {
        let mut g = guard(Role::Player);
        for frame in [-1, i32::MIN, i32::MAX] {
            assert!(!passes(&mut g, Body::ChecksumReport { checksum: 1, frame }));
        }
        assert!(passes(
            &mut g,
            Body::ChecksumReport {
                checksum: 1,
                frame: 9000
            }
        ));
        assert!(passes(
            &mut g,
            Body::ChecksumReport {
                checksum: 1,
                frame: 8940
            }
        ));
        assert!(!passes(
            &mut g,
            Body::ChecksumReport {
                checksum: 1,
                frame: 60
            }
        ));
    }

    #[test]
    fn after_the_handshake_only_its_magic_counts() {
        let mut g = guard(Role::Player);
        let other = |body| Msg { magic: 8, body }.into_ggrs().unwrap();
        // before: ggrs takes any magic, and so does the guard
        assert!(g.check(&addr(1), other(Body::KeepAlive)).is_some());
        for r in 0..SYNC_ROUNDTRIPS {
            g.send_to(&msg(Body::SyncRequest(r)), &addr(1));
            // a reply to nothing asked does not count
            assert!(passes(&mut g, Body::SyncReply(1000 + r)));
            assert!(passes(&mut g, Body::SyncReply(r)));
        }
        assert_eq!(g.peers[&addr(1)].magic, 7);
        assert!(g.check(&addr(1), other(Body::KeepAlive)).is_none());
        // an input under another magic is not taken for received
        assert!(
            g.check(&addr(1), other(Body::Input(input(0, &[[0, 0]]))))
                .is_none()
        );
        assert!(passes(&mut g, Body::Input(input(0, &[[0, 0]]))));
    }

    #[test]
    fn strangers_are_dropped() {
        let mut g = guard(Role::Player);
        assert!(g.check(&addr(2), msg(Body::KeepAlive)).is_none());
    }

    #[test]
    fn deltas_are_decoded_against_the_frame_before() {
        let mut g = guard(Role::Player);
        // frame 0 = [5, 0]; then frame 1 as a delta of it
        assert!(passes(&mut g, Body::Input(input(0, &[[5, 0]]))));
        assert!(passes(&mut g, Body::Input(input(1, &[[1, 0]]))));
        let recv = &g.peers[&addr(1)].recv;
        assert_eq!(recv[&0], vec![5, 0]);
        assert_eq!(recv[&1], vec![4, 0]);
        // old frames go as ggrs drops them
        for next in 2..42 {
            let mut ours = input(next, &[[0, 0]]);
            ours.statuses[0].last_frame = next;
            g.send_to(&msg(Body::Input(ours)), &addr(1));
            assert!(passes(&mut g, Body::Input(input(next, &[[0, 0]]))));
        }
        let recv = &g.peers[&addr(1)].recv;
        assert_eq!(recv.keys().min(), Some(&(41 - 16)));
    }

    #[test]
    fn the_decoder_agrees_with_ggrs_encoder() {
        // run forms: zeros, ones, plain
        assert_eq!(rle_decode(&[4 << 2 | 1], 64), Some(vec![0; 4]));
        assert_eq!(rle_decode(&[4 << 2 | 3], 64), Some(vec![255; 4]));
        assert_eq!(
            rle_decode(&[2 << 1, 9, 8, 3 << 2 | 1], 64),
            Some(vec![9, 8, 0, 0, 0])
        );
        assert_eq!(rle_decode(&[], 64), Some(vec![]));
        // a repeat of 65 bytes into room for 64
        assert_eq!(rle_decode(&[0x85, 0x02], 64), None);
        // any bytes at all: never a panic, never more than the limit
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let mut buf = Vec::new();
            for _ in 0..(seed % 12) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                buf.push(seed as u8);
            }
            seed = seed.wrapping_mul(0x2545_f491_4f6c_dd1d).wrapping_add(1);
            if let Some(out) = rle_decode(&buf, 256) {
                assert!(out.len() <= 256);
            }
        }
    }
}
