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
    /// UDP rendezvous port on the same host.
    pub udp_port: u16,
    pub online: usize,
    pub queued: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueStatus {
    pub queued: bool,
    pub waiting_secs: u64,
    pub players_in_queue: usize,
    pub found: Option<MatchInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchInfo {
    pub match_id: u64,
    pub code: String,
    pub side: u8,
    pub my_char: String,
    pub opponent: ProfileInfo,
    pub opponent_char: String,
    pub stage: String,
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
