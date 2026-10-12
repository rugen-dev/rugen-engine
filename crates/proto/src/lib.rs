//! Types exchanged between `rugen-server` and the game: JSON (serde) for the
//! lobby API and WebSocket, plus the tiny binary framing of relayed match
//! traffic in [`relay`].
//!
//! Privacy rule: no payload ever carries another player's IP address or
//! endpoint. Players only ever talk to the server they chose.

#![forbid(unsafe_code)]

pub mod evidence;
pub mod fair;
pub mod relay;

pub use fair::*;

use serde::{Deserialize, Serialize};

pub const API_VERSION: u32 = 3;

/// Rank band name for a rating.
pub fn rank_name(rating: i32) -> &'static str {
    match rating {
        i32::MIN..=999 => "TIN",
        1000..=1199 => "BRONZE",
        1200..=1399 => "SILVER",
        1400..=1599 => "GOLD",
        1600..=1799 => "PLATINUM",
        1800..=1999 => "DIAMOND",
        _ => "LEGEND",
    }
}

/// Signal-strength style quality from a round trip in milliseconds: 4 bars
/// up to 40 ms, 3 up to 80, 2 up to 150, else 1.
pub fn ping_bars(rtt_ms: u32) -> u8 {
    match rtt_ms {
        0..=40 => 4,
        41..=80 => 3,
        81..=150 => 2,
        _ => 1,
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileInfo {
    pub id: u64,
    pub name: String,
    pub avatar: u32,
    pub rating: i32,
    pub best_rating: i32,
    pub wins: u32,
    pub losses: u32,
    pub rank: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Registered {
    pub id: u64,
    pub key: String,
}

/// One way to reach a server's match relay. Servers list them in their
/// order of preference; clients try the kinds they know in that order and
/// ignore unknown ones (so new transports, e.g. TURN, need no API break).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RelayTransport {
    /// `udp` (addr = `host:port`) or `ws` (addr = path like `/relay` on the
    /// server's own URL, or an absolute `ws(s)://` URL).
    pub kind: String,
    pub addr: String,
    /// `turn`: TURN URLs (`turn:` / `turns:`) of a TURN service both players
    /// relay through (the server only signals; it never sees the packets).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// `turn`: short-lived long-term credential for this player only.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub credential: String,
    /// `turn`: unix time the credential stops working.
    #[serde(skip_serializing_if = "is_zero")]
    pub expires: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

// TURN credentials must never end up in logs
impl std::fmt::Debug for RelayTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("RelayTransport");
        d.field("kind", &self.kind).field("addr", &self.addr);
        if !self.urls.is_empty() {
            d.field("urls", &self.urls)
                .field("credential", &"<redacted>")
                .field("expires", &self.expires);
        }
        d.finish()
    }
}

impl RelayTransport {
    pub fn udp(addr: &str) -> Self {
        Self {
            kind: "udp".into(),
            addr: addr.into(),
            ..Default::default()
        }
    }

    pub fn ws(addr: &str) -> Self {
        Self {
            kind: "ws".into(),
            addr: addr.into(),
            ..Default::default()
        }
    }

    pub fn turn(urls: Vec<String>, username: &str, credential: &str, expires: u64) -> Self {
        Self {
            kind: "turn".into(),
            urls,
            username: username.into(),
            credential: credential.into(),
            expires,
            ..Default::default()
        }
    }
}

/// The transports a server offers: UDP first when it has a public UDP
/// relay, then the `/relay` WebSocket (always).
pub fn relay_transports(udp: Option<&str>) -> Vec<RelayTransport> {
    udp.map(RelayTransport::udp)
        .into_iter()
        .chain(std::iter::once(RelayTransport::ws("/relay")))
        .collect()
}

/// `GET /` and `GET /health`: what the server list shows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerInfo {
    pub api: u32,
    pub name: String,
    #[serde(default)]
    pub motd: String,
    #[serde(default)]
    pub version: String,
    /// Players seen in the last few minutes or connected right now.
    pub online: usize,
    pub queued: usize,
    #[serde(default)]
    pub rooms: usize,
    /// Matches being relayed right now.
    #[serde(default)]
    pub matches: usize,
    /// Public `host:port` of the server's UDP relay (fast path), if enabled.
    /// This is the server's own address, never a player's.
    #[serde(default)]
    pub udp: Option<String>,
}

/// Room rules chosen by the host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rules {
    /// Rounds to win the match.
    pub rounds: u8,
    /// Round time in seconds (0 = infinite).
    pub round_time: u16,
    /// "classic" or "arena".
    pub mode: String,
    /// "free" (each picks), "mirror" (joiner plays the host's fighter).
    pub chars: String,
    /// Stage id ("" = server/host default).
    pub stage: String,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            rounds: 2,
            round_time: 99,
            mode: "classic".into(),
            chars: "free".into(),
            stage: String::new(),
        }
    }
}

/// Everything a client needs to join a relay session: which session, which
/// slot (0 = P1, 1 = P2, 2.. = spectators), and the secret token that
/// authenticates its datagrams. Only ever sent to that slot's owner.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionTicket {
    pub session: u32,
    pub slot: u8,
    /// 32 hex characters.
    pub token: String,
    /// Spectator slots that exist in this session (2 .. 2 + spectators).
    pub spectators: u8,
    /// Server UDP relay `host:port` (fast path); WebSocket `/relay` otherwise.
    pub udp: Option<String>,
    /// Transports in order of preference (API 4); when present, clients use
    /// these instead of `udp`. Unknown kinds are skipped.
    #[serde(default)]
    pub transports: Vec<RelayTransport>,
}

impl SessionTicket {
    /// The `turn` transport of this ticket, if the server issued one.
    pub fn turn(&self) -> Option<&RelayTransport> {
        self.transports
            .iter()
            .find(|t| t.kind == "turn" && !t.urls.is_empty())
    }
}

/// One entry of the public lobby (`GET /rooms`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RoomSummary {
    pub code: String,
    pub name: String,
    pub host: String,
    pub rules: Rules,
    pub players: u8,
    pub spectators: u8,
    pub max_spectators: u8,
    pub locked: bool,
    pub in_match: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RoomMember {
    pub profile: ProfileInfo,
    /// 0 = host, 1 = challenger, 2 = spectator.
    pub role: u8,
    /// Server-measured round trip to the server, if known.
    pub rtt_ms: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RoomInfo {
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub rules: Rules,
    /// 1 = host, 2 = challenger, 3 = spectator.
    pub side: u8,
    /// A match is running (or about to): see `ticket`.
    pub ready: bool,
    pub p1_char: String,
    pub p2_char: String,
    pub stage: String,
    pub opponent: Option<ProfileInfo>,
    #[serde(default)]
    pub members: Vec<RoomMember>,
    #[serde(default)]
    pub max_spectators: u8,
    /// Unique id both sides report results under.
    pub match_id: u64,
    /// Estimated round trip between the players through the server.
    #[serde(default)]
    pub est_ping_ms: Option<u32>,
    /// This member's relay ticket for the running match.
    #[serde(default)]
    pub ticket: Option<SessionTicket>,
    /// Pick / verdict / file sync phase before the match (API 3).
    #[serde(default)]
    pub matchup: Option<MatchupView>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueueStatus {
    pub queued: bool,
    pub waiting_secs: u64,
    pub players_in_queue: usize,
    pub found: Option<MatchInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MatchInfo {
    pub match_id: u64,
    pub code: String,
    pub side: u8,
    pub my_char: String,
    pub opponent: ProfileInfo,
    pub opponent_char: String,
    pub stage: String,
    #[serde(default)]
    pub ranked: bool,
    /// Estimated round trip to the opponent through the server.
    #[serde(default)]
    pub est_ping_ms: Option<u32>,
    #[serde(default)]
    pub ticket: Option<SessionTicket>,
    /// Pick / verdict / file sync phase; the ticket arrives in it (API 3).
    #[serde(default)]
    pub matchup: Option<MatchupView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Season {
    pub number: u32,
    pub name: String,
    pub ends_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Leaderboard {
    pub season: Season,
    pub rows: Vec<ProfileInfo>,
    pub me: Option<(usize, ProfileInfo)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultAck {
    pub accepted: bool,
    /// Set once both sides agreed and ratings moved.
    pub rating: Option<i32>,
    pub delta: Option<i32>,
    /// The two reports (winner, inputs or final checksum) disagree.
    #[serde(default)]
    pub disputed: bool,
}

/// A stored ranked replay (`GET /replay/{match_id}`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Replay {
    pub match_id: u64,
    pub created: u64,
    pub p1: ProfileInfo,
    pub p2: ProfileInfo,
    pub p1_char: String,
    pub p2_char: String,
    pub stage: String,
    pub winner: Option<u64>,
    /// Confirmed inputs, 4 bytes per frame (P1 u16 LE, P2 u16 LE), hex.
    pub inputs: String,
    /// Fingerprints of P1, P2 and the stage (API 3).
    #[serde(default)]
    pub fingerprints: Vec<String>,
    /// The server still holds every package of this match.
    #[serde(default)]
    pub files_available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// Lobby WebSocket (`GET /ws?key=`) messages from the game.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Ping,
}

/// Lobby WebSocket messages from the server (presence pushes).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        id: u64,
        name: String,
    },
    /// A room we are in changed (joined, match started, member left).
    Room {
        room: RoomInfo,
    },
    /// The queue paired us.
    Found {
        found: MatchInfo,
    },
    /// The matchup phase of a match we play or watch changed.
    Matchup {
        matchup: MatchupView,
    },
    /// The host removed us from a room.
    Kicked {
        code: String,
    },
    Pong,
    Error {
        error: String,
    },
}

/// Hex helpers for tokens and input streams.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_tagged_json() {
        let s = serde_json::to_string(&ClientMsg::Ping).unwrap();
        assert_eq!(s, r#"{"t":"ping"}"#);
        let p: ServerMsg = serde_json::from_str(r#"{"t":"pong"}"#).unwrap();
        assert_eq!(p, ServerMsg::Pong);
    }

    #[test]
    fn old_server_info_still_decodes() {
        let i: ServerInfo =
            serde_json::from_str(r#"{"api":1,"name":"x","udp_port":7777,"online":0,"queued":0}"#)
                .unwrap();
        assert!(i.udp.is_none() && i.motd.is_empty());
    }

    #[test]
    fn turn_transport_travels_in_the_ticket() {
        let mut t = SessionTicket::default();
        assert!(t.turn().is_none());
        t.transports = relay_transports(None);
        t.transports.insert(
            0,
            RelayTransport::turn(vec!["turn:t.example:3478".into()], "u", "secret", 99),
        );
        let s = serde_json::to_string(&t).unwrap();
        assert!(s.contains(r#""kind":"turn""#) && s.contains(r#""expires":99"#));
        // the other kinds stay as small as before
        assert!(s.contains(r#"{"kind":"ws","addr":"/relay"}"#));
        let back: SessionTicket = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.turn().unwrap().username, "u");
        assert!(!format!("{t:?}").contains("secret"));
        // an API 3 ticket still decodes
        let old: SessionTicket =
            serde_json::from_str(r#"{"session":1,"slot":0,"token":"","spectators":0,"udp":"h:1"}"#)
                .unwrap();
        assert!(old.transports.is_empty() && old.turn().is_none());
    }

    #[test]
    fn bars() {
        assert_eq!(ping_bars(10), 4);
        assert_eq!(ping_bars(60), 3);
        assert_eq!(ping_bars(120), 2);
        assert_eq!(ping_bars(400), 1);
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(to_hex(&[0, 255, 16]), "00ff10");
        assert_eq!(from_hex("00ff10").unwrap(), vec![0, 255, 16]);
        assert!(from_hex("0").is_none());
        assert!(from_hex("zz").is_none());
    }
}
