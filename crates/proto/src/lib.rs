//! JSON types exchanged between `rugen-server` and the game. Everything is
//! plain serde data; the HTTP layer on both sides is a few dozen lines.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const API_VERSION: u32 = 1;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub api: u32,
    pub name: String,
    /// Legacy UDP rendezvous port on the same host (0 = disabled).
    pub udp_port: u16,
    pub online: usize,
    pub queued: usize,
    /// Public `host:port` of the optional UDP relay (fallback when hole punching fails).
    #[serde(default)]
    pub udp_relay: Option<String>,
    /// The server can mint TURN credentials (`GET /turn`).
    #[serde(default)]
    pub turn: bool,
    /// Region hint the server sees for this request (Cloudflare `CF-IPCountry`).
    #[serde(default)]
    pub region: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RoomInfo {
    pub code: String,
    /// 1 = creator, 2 = joiner.
    pub side: u8,
    pub ready: bool,
    pub p1_char: String,
    pub p2_char: String,
    pub stage: String,
    pub opponent: Option<ProfileInfo>,
    /// Unique id both sides report results under.
    pub match_id: u64,
    /// Server estimate of the round trip to the opponent, if any.
    #[serde(default)]
    pub est_ping_ms: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Server estimate of the round trip to the opponent (region history), if any.
    #[serde(default)]
    pub est_ping_ms: Option<u32>,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// Short-lived TURN credentials minted by the server (Cloudflare Realtime TURN).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnCredentials {
    /// `turn:host:port?transport=udp` style URLs.
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
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

/// WebSocket (`GET /ws?key=`) messages from the game to the server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// "I am about to play in `code`; reach me at these UDP endpoints."
    /// `side` (1 or 2) is only used for codes the server does not know
    /// (rooms and ranked matches assign sides themselves).
    Signal {
        code: String,
        side: u8,
        candidates: Vec<String>,
    },
    /// Hole punching failed: my TURN relayed address (`None` = no TURN here).
    Relay {
        code: String,
        addr: Option<String>,
    },
    Ping,
}

/// WebSocket messages from the server to the game.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        id: u64,
        name: String,
    },
    /// Both sides signalled: the opponent's endpoints and the fallbacks on offer.
    Peer {
        code: String,
        /// Our side, 1 or 2.
        side: u8,
        candidates: Vec<String>,
        turn: bool,
        udp_relay: Option<String>,
    },
    /// The opponent's TURN relayed address (or `None`: try the UDP relay).
    PeerRelay {
        code: String,
        addr: Option<String>,
    },
    /// Presence: a room we created was joined.
    Room {
        room: RoomInfo,
    },
    /// Presence: the ranked queue paired us.
    Found {
        found: MatchInfo,
    },
    Pong,
    Error {
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_tagged_json() {
        let m = ClientMsg::Signal {
            code: "ABCDE".into(),
            side: 1,
            candidates: vec!["1.2.3.4:7000".into()],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.starts_with(r#"{"t":"signal""#), "{s}");
        assert_eq!(serde_json::from_str::<ClientMsg>(&s).unwrap(), m);
        let p: ServerMsg = serde_json::from_str(r#"{"t":"pong"}"#).unwrap();
        assert_eq!(p, ServerMsg::Pong);
    }

    #[test]
    fn old_server_info_still_decodes() {
        let i: ServerInfo =
            serde_json::from_str(r#"{"api":1,"name":"x","udp_port":7777,"online":0,"queued":0}"#)
                .unwrap();
        assert!(!i.turn && i.udp_relay.is_none());
    }

    #[test]
    fn bars() {
        assert_eq!(ping_bars(10), 4);
        assert_eq!(ping_bars(60), 3);
        assert_eq!(ping_bars(120), 2);
        assert_eq!(ping_bars(400), 1);
    }
}
