//! Types exchanged between `rugen-server` and the game: JSON (serde) for the
//! lobby API and WebSocket, plus the tiny binary framing of relayed match
//! traffic in [`relay`].
//!
//! Privacy rule: no payload ever carries another player's IP address or
//! endpoint. Players only ever talk to the server they chose.

#![forbid(unsafe_code)]

pub mod account;
pub mod evidence;
pub mod fair;
pub mod relay;
pub mod room_code;
pub mod size;
pub mod stream;
pub mod trust;

pub use account::{AccountInfo, Session};
pub use fair::*;
pub use trust::*;

use serde::{Deserialize, Serialize};

/// Lobby API version. 5: fingerprints are `rugen-fp-2` (sprites by their
/// pixels, sounds left out) and packages are `.rgc` / `.rgs` content files
/// instead of RGPK archives; servers and clients of different versions
/// cannot share fighters.
pub const API_VERSION: u32 = 5;

/// Request header carrying the client's [`API_VERSION`] on every HTTP call.
/// Servers that serve several APIs ([`ServerInfo::api_min`]) treat a
/// request without it as API 4 (clients up to 0.5 never sent it).
pub const API_HEADER: &str = "X-Rugen-Api";

/// The game release players know a lobby API by (for messages).
pub fn release_of(api: u32) -> &'static str {
    match api {
        0..=4 => "0.5",
        5 => "0.6",
        _ => "a newer version",
    }
}

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
    /// `name` is the account's username (API 5 named account, unique on
    /// the server): clients show a badge. Free-form names of anonymous
    /// accounts never carry it, so a username cannot be impersonated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub verified: bool,
    /// How the player finished the last season it played in (the badge
    /// kept on the profile and the boards); absent from older servers and
    /// for players who never finished a season.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_season: Option<SeasonBadge>,
}

/// A finished season's standing, kept as a badge: the tier of the rating
/// the player ended the season with ([`rank_name`]).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeasonBadge {
    pub season: u32,
    /// Final rating of that season (before the soft reset).
    pub rating: i32,
    /// Final leaderboard position (1-based).
    pub rank: u32,
    /// [`rank_name`] of `rating`.
    pub tier: String,
}

impl SeasonBadge {
    pub fn new(season: u32, rating: i32, rank: u32) -> Self {
        Self {
            season,
            rating,
            rank,
            tier: rank_name(rating).to_string(),
        }
    }
}

pub(crate) fn is_false(v: &bool) -> bool {
    !*v
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
    /// Match relay transports in order of preference (API 4; `udp` above
    /// stays for older clients). Empty from older servers.
    #[serde(default)]
    pub transports: Vec<RelayTransport>,
    /// The server's Ed25519 public key (hex) when it signs (API 4). A claim
    /// only: clients trust a server once it signed their identity nonce.
    #[serde(default)]
    pub key: Option<String>,
    /// The server claims to be the official one (API 4; verified by key).
    #[serde(default)]
    pub official: bool,
    /// The oldest API the server still serves next to `api` (players of
    /// different APIs never meet there). 0 from servers that serve `api`
    /// only.
    #[serde(default)]
    pub api_min: u32,
    /// The server offers named accounts ([`account`]: username + password,
    /// API 5). False from older servers: anonymous accounts only.
    #[serde(default)]
    pub accounts: bool,
    /// Days the server keeps the fighters players send it before deleting
    /// them (the consent dialog says it). 0 from older servers.
    #[serde(default)]
    pub pkg_days: u32,
}

impl ServerInfo {
    /// The oldest API this server serves.
    pub fn min_api(&self) -> u32 {
        if self.api_min == 0 || self.api_min > self.api {
            self.api
        } else {
            self.api_min
        }
    }

    /// Whether clients of `api` can play here.
    pub fn serves(&self, api: u32) -> bool {
        (self.min_api()..=self.api).contains(&api)
    }
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
    /// FIGHTER SIZE both sides simulate with: "none", "chibi", "small" or
    /// "normal". Missing (servers and clients before it) = "normal", which
    /// quick match and ranked always use.
    pub fighter_size: String,
    /// LIFE every fighter gets instead of its own `[Data] life` (1000 to
    /// 10000 in steps of 500; 0 = each fighter's own). Missing (servers and
    /// clients before it) = 5000, which quick match and ranked always use.
    pub life: u16,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            rounds: 2,
            round_time: 99,
            mode: "classic".into(),
            chars: "free".into(),
            stage: String::new(),
            fighter_size: "normal".into(),
            life: LIFE_DEFAULT,
        }
    }
}

/// [`Rules::life`] of rooms that do not say, quick match and ranked.
pub const LIFE_DEFAULT: u16 = 5000;

/// [`Rules::fighter_size`] values.
pub const FIGHTER_SIZES: [&str; 4] = ["none", "chibi", "small", "normal"];

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
    /// The server keeps this match's input log for viewers: players send
    /// their confirmed inputs on the relay session ([`relay::STREAM`]).
    /// False from older servers (nothing is sent then).
    #[serde(default, skip_serializing_if = "is_false")]
    pub live: bool,
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
    /// The host's API on servers serving several ([`ServerInfo::api_min`]);
    /// only players of the same API can join. 0 when the server does not say.
    #[serde(default)]
    pub api: u32,
    /// `host` is a username ([`ProfileInfo::verified`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub host_verified: bool,
    /// The room runs a tournament bracket (absent: a 1 v 1 room, or an
    /// older server). Older clients cannot join such a room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tournament: Option<TournamentBrief>,
    /// The match running in the room when the server keeps its input log:
    /// viewers open it with `GET /v1/live/{id}` (0: none, or an older server).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub live_match: u64,
}

/// A tournament room in the lobby list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TournamentBrief {
    /// Bracket size: 8 or 16.
    pub size: u8,
    pub entrants: u8,
    /// `open` (taking entrants), `running` or `done`.
    pub state: String,
}

/// One tournament entrant (seed order once drawn, join order before).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Entrant {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub verified: bool,
    pub rating: i32,
    /// 1-based seed once the bracket is drawn, 0 before.
    pub seed: u8,
}

/// One match of a bracket.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BracketMatch {
    /// Accounts of the two sides (None: a bye or not decided yet).
    pub p1: Option<u64>,
    pub p2: Option<u64>,
    /// Their seeds (0 = empty).
    pub seeds: [u8; 2],
    /// Rounds won per side (0-0 when unknown).
    pub score: [u8; 2],
    pub winner: Option<u64>,
    /// `wait` (a side not decided), `next` (both known), `called` (the
    /// server called it: matchup phase), `live` (being fought), `done`,
    /// `bye`.
    pub state: String,
    /// The match id once called (0 before).
    pub match_id: u64,
}

/// A room's tournament as members see it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TournamentView {
    pub size: u8,
    /// `open`, `running` or `done`.
    pub state: String,
    pub entrants: Vec<Entrant>,
    /// Rounds from the first to the final (empty before the draw).
    pub rounds: Vec<Vec<BracketMatch>>,
    /// (round, index) of the called / live match.
    pub live: Option<[u8; 2]>,
    pub champion: Option<u64>,
}

impl TournamentView {
    /// The round's name for a bracket of `rounds` rounds: FINAL,
    /// SEMI-FINALS, QUARTER-FINALS, ROUND OF 16.
    pub fn round_name(rounds: usize, round: usize) -> &'static str {
        match rounds.saturating_sub(round) {
            1 => "FINAL",
            2 => "SEMI-FINALS",
            3 => "QUARTER-FINALS",
            4 => "ROUND OF 16",
            _ => "ROUND",
        }
    }
}

impl RoomSummary {
    /// Whether a client of `api` can join (unknown counts as yes).
    pub fn joinable_by(&self, api: u32) -> bool {
        self.api == 0 || self.api == api
    }
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
    /// The room's tournament (absent: a 1 v 1 room). Members who do not
    /// play the called match have `side` 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tournament: Option<Box<TournamentView>>,
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

/// A ranked season: ratings soft-reset toward the start rating when it
/// ends ([`season_reset`]).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Season {
    pub number: u32,
    pub name: String,
    pub ends_at: u64,
    /// Unix time it started (0 from older servers).
    #[serde(default)]
    pub started_at: u64,
    /// Seconds left when the server answered (0: over, or an older
    /// server: use `ends_at`). Immune to a wrong client clock.
    #[serde(default)]
    pub secs_left: u64,
}

impl Season {
    /// "ENDS IN 12 DAYS" / "ENDS IN 5 HOURS" / "ENDED"; `now` = Unix time
    /// of the client, used only when the server did not say.
    pub fn ends_line(&self, now: u64) -> String {
        let left = if self.secs_left > 0 {
            self.secs_left
        } else {
            self.ends_at.saturating_sub(now)
        };
        match left {
            0 => "ENDED".into(),
            s if s >= 2 * 86_400 => format!("ENDS IN {} DAYS", s / 86_400),
            s if s >= 2 * 3600 => format!("ENDS IN {} HOURS", s / 3600),
            _ => "ENDS SOON".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Leaderboard {
    pub season: Season,
    pub rows: Vec<ProfileInfo>,
    pub me: Option<(usize, ProfileInfo)>,
    /// Finished seasons the server keeps final standings of, newest first
    /// (`GET /leaderboard?season=N`); empty from older servers.
    #[serde(default)]
    pub past: Vec<u32>,
    /// `rows` are a finished season's final standings (ratings as they
    /// ended it), not the live board.
    #[serde(default)]
    pub finished: bool,
    /// A per-fighter board (`GET /leaderboard?fighter=FP`): the fighter it
    /// ranks; `rows` then carry each player's rating and record with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fighter: Option<FighterEntry>,
    /// The fighters with a board that season, most played first (asked with
    /// `fighters=1` or `fighter=`); empty from older servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fighters: Vec<FighterEntry>,
}

/// A fighter (content fingerprint) with a per-fighter leaderboard.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FighterEntry {
    pub fingerprint: String,
    /// The name its players declared.
    pub name: String,
    /// Players ranked with it this season.
    pub players: u32,
    /// Ranked appearances this season.
    pub games: u32,
}

/// One player's line in a season reset: the rating and record it ends
/// the season with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    pub account: u64,
    pub rating: i32,
    pub wins: u32,
    pub losses: u32,
}

/// What a season reset decides ([`season_reset`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeasonReset {
    /// Final standings of the players who played the season (a win or a
    /// loss), best first: (account, final rating, wins, losses, 1-based
    /// position).
    pub results: Vec<(u64, i32, u32, u32, u32)>,
    /// New rating of every account (records restart at 0-0).
    pub ratings: Vec<(u64, i32)>,
}

/// Rating a season reset pulls everyone toward (the start rating).
pub const SEASON_PIVOT: i32 = 1200;

/// A rating after the soft reset: halfway to [`SEASON_PIVOT`] (integer
/// division, rounded toward the pivot).
pub fn soft_reset(rating: i32) -> i32 {
    let d = rating - SEASON_PIVOT;
    SEASON_PIVOT + d / 2
}

/// The end of a season, as a pure function of the standings: the final
/// positions of the players who played (rating, then the older account
/// first, like the leaderboard) and everyone's new rating.
pub fn season_reset(players: &[Standing]) -> SeasonReset {
    let mut played: Vec<&Standing> = players.iter().filter(|p| p.wins + p.losses > 0).collect();
    played.sort_by_key(|p| (std::cmp::Reverse(p.rating), p.account));
    SeasonReset {
        results: played
            .iter()
            .enumerate()
            .map(|(i, p)| (p.account, p.rating, p.wins, p.losses, i as u32 + 1))
            .collect(),
        ratings: players
            .iter()
            .map(|p| (p.account, soft_reset(p.rating)))
            .collect(),
    }
}

/// Which season `now` falls in when season 1 started at `first_start` and
/// each lasts `len` seconds: (number, its start). Times before the first
/// start are season 1.
pub fn season_at(first_start: u64, len: u64, now: u64) -> (u32, u64) {
    let len = len.max(1);
    let n = now.saturating_sub(first_start) / len;
    (n as u32 + 1, first_start + n * len)
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
    /// [`ResultStatement`] signed by the server once the result is settled (API 4).
    #[serde(default)]
    pub signed: Option<SignedBlob>,
    /// How the match ended when it ended early, as the server settled it:
    /// [`Ending::as_str`] (`"flee"`, `"disconnect"`); absent for a match
    /// played to the end and from older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<String>,
    /// The account that fled or disconnected (with `ending`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fled: Option<u64>,
}

/// How a match ended early (the `ending` of a result report).
///
/// The player who leaves reports [`Ending::Flee`] with the other player as
/// the winner (a forfeit); the player left behind reports itself as the
/// winner with [`Ending::Flee`] (a flee message or the relay's announcement
/// of the other player's BYE) or [`Ending::Disconnect`] (silence on every
/// path). A server only settles the second kind on its own view of the
/// relay session (the other slot said BYE or went silent first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ending {
    Flee,
    Disconnect,
}

impl Ending {
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::Flee => "flee",
            Ending::Disconnect => "disconnect",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "flee" => Some(Ending::Flee),
            "disconnect" => Some(Ending::Disconnect),
            _ => None,
        }
    }
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
    fn verified_flags_are_additive() {
        // older payloads (and API 4 servers) carry no flag: not verified
        let p: ProfileInfo = serde_json::from_str(
            r#"{"id":1,"name":"x","avatar":0,"rating":1200,"best_rating":1200,"wins":0,"losses":0,"rank":"SILVER"}"#,
        )
        .unwrap();
        assert!(!p.verified);
        // an anonymous profile serializes exactly as before
        assert!(!serde_json::to_string(&p).unwrap().contains("verified"));
        let v = ProfileInfo {
            verified: true,
            ..p
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""verified":true"#));
        assert_eq!(serde_json::from_str::<ProfileInfo>(&json).unwrap(), v);
        let i: ServerInfo =
            serde_json::from_str(r#"{"api":5,"name":"x","online":0,"queued":0}"#).unwrap();
        assert!(!i.accounts);
    }

    #[test]
    fn old_server_info_still_decodes() {
        let i: ServerInfo =
            serde_json::from_str(r#"{"api":1,"name":"x","udp_port":7777,"online":0,"queued":0}"#)
                .unwrap();
        assert!(i.udp.is_none() && i.motd.is_empty());
    }

    #[test]
    fn rules_without_fighter_size_mean_normal() {
        let r: Rules = serde_json::from_str(r#"{"rounds":3,"round_time":60}"#).unwrap();
        assert_eq!(r.fighter_size, "normal");
        // and without LIFE, 5000 (what quick match and ranked play)
        assert_eq!(r.life, LIFE_DEFAULT);
        assert_eq!(LIFE_DEFAULT, 5000);
        assert_eq!(r.rounds, 3);
        let back: Rules = serde_json::from_value(
            serde_json::to_value(Rules {
                fighter_size: "chibi".into(),
                ..Rules::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(back.fighter_size, "chibi");
        assert!(FIGHTER_SIZES.contains(&Rules::default().fighter_size.as_str()));
    }

    #[test]
    fn servers_say_which_apis_they_serve() {
        let info = |json: &str| serde_json::from_str::<ServerInfo>(json).unwrap();
        // an API 4 server of the 0.5 era: 4 only
        let old = info(r#"{"api":4,"name":"x","online":0,"queued":0}"#);
        assert!(old.serves(4) && !old.serves(5));
        // a transition server: 4 and 5
        let both = info(r#"{"api":5,"api_min":4,"name":"x","online":0,"queued":0}"#);
        assert!(both.serves(4) && both.serves(5) && !both.serves(6));
        // nonsense api_min is ignored
        let odd = info(r#"{"api":5,"api_min":9,"name":"x","online":0,"queued":0}"#);
        assert!(odd.serves(5) && !odd.serves(4));
        assert_eq!(release_of(4), "0.5");
        assert_eq!(release_of(API_VERSION), "0.6");
        // rooms of servers that do not say are joinable
        let r: RoomSummary = serde_json::from_str(
            r#"{"code":"A","name":"n","host":"h","rules":{},"players":1,"spectators":0,"max_spectators":2,"locked":false,"in_match":false}"#,
        )
        .unwrap();
        assert!(r.joinable_by(API_VERSION));
        let r = RoomSummary { api: 4, ..r };
        assert!(!r.joinable_by(5) && r.joinable_by(4));
    }

    #[test]
    fn transports_are_ordered_and_extensible() {
        let t = relay_transports(Some("relay.example:7001"));
        assert_eq!(t[0], RelayTransport::udp("relay.example:7001"));
        assert_eq!(t[1], RelayTransport::ws("/relay"));
        assert_eq!(relay_transports(None), vec![RelayTransport::ws("/relay")]);
        // a future kind decodes fine (clients skip what they do not know)
        let s: SessionTicket = serde_json::from_str(
            r#"{"session":1,"slot":0,"token":"","spectators":0,"udp":null,
               "transports":[{"kind":"turn","addr":"x","creds":"y"},{"kind":"ws","addr":"/relay"}]}"#,
        )
        .unwrap();
        assert_eq!(s.transports.len(), 2);
        // and an API 3 ticket still decodes
        let old: SessionTicket =
            serde_json::from_str(r#"{"session":1,"slot":0,"token":"","spectators":0,"udp":"h:1"}"#)
                .unwrap();
        assert!(old.transports.is_empty());
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
    fn season_reset_pulls_toward_1200_and_ranks_who_played() {
        assert_eq!(soft_reset(1600), 1400);
        assert_eq!(soft_reset(1200), 1200);
        assert_eq!(soft_reset(1000), 1100);
        assert_eq!(soft_reset(1201), 1200);
        assert_eq!(soft_reset(1199), 1200);
        assert_eq!(soft_reset(1803), 1501);
        // the reset never crosses the pivot and never moves it
        for r in 0..3000 {
            let n = soft_reset(r);
            assert!((n - 1200).abs() <= (r - 1200).abs());
            assert!((n - 1200).signum() * (r - 1200).signum() >= 0);
        }
        let st = |account, rating, wins, losses| Standing {
            account,
            rating,
            wins,
            losses,
        };
        let r = season_reset(&[
            st(7, 1500, 3, 1),
            st(3, 1700, 9, 2),
            st(9, 1250, 0, 0),
            st(5, 1500, 1, 4),
        ]);
        // best first; a tie goes to the older (lower) account
        assert_eq!(
            r.results,
            vec![(3, 1700, 9, 2, 1), (5, 1500, 1, 4, 2), (7, 1500, 3, 1, 3)]
        );
        assert_eq!(r.ratings, vec![(7, 1350), (3, 1450), (9, 1225), (5, 1350)]);
        assert_eq!(season_reset(&[]), SeasonReset::default());
        let b = SeasonBadge::new(2, 1450, 4);
        assert_eq!(b.tier, "GOLD");
    }

    #[test]
    fn seasons_follow_the_clock() {
        let day = 86_400;
        assert_eq!(season_at(1000, 90 * day, 1000), (1, 1000));
        assert_eq!(season_at(1000, 90 * day, 500), (1, 1000));
        assert_eq!(season_at(1000, 90 * day, 1000 + 90 * day - 1), (1, 1000));
        assert_eq!(
            season_at(1000, 90 * day, 1000 + 90 * day),
            (2, 1000 + 90 * day)
        );
        assert_eq!(season_at(0, 10, 35), (4, 30));
        let s = Season {
            number: 3,
            name: "SEASON 3".into(),
            ends_at: 100 * day,
            started_at: 0,
            secs_left: 0,
        };
        assert_eq!(s.ends_line(88 * day), "ENDS IN 12 DAYS");
        assert_eq!(s.ends_line(100 * day - 5 * 3600), "ENDS IN 5 HOURS");
        assert_eq!(s.ends_line(100 * day), "ENDED");
        let told = Season {
            secs_left: 3 * day,
            ..s.clone()
        };
        assert_eq!(
            told.ends_line(0),
            "ENDS IN 3 DAYS",
            "the server's count wins"
        );
        // an older server's season and board still decode
        let old: Leaderboard = serde_json::from_str(
            r#"{"season":{"number":1,"name":"SEASON 1","ends_at":5},"rows":[],"me":null}"#,
        )
        .unwrap();
        assert!(old.past.is_empty() && !old.finished && old.season.started_at == 0);
        assert!(old.fighter.is_none() && old.fighters.is_empty());
        let p: ProfileInfo = serde_json::from_str(
            r#"{"id":1,"name":"x","avatar":0,"rating":1200,"best_rating":1200,"wins":0,"losses":0,"rank":"SILVER"}"#,
        )
        .unwrap();
        assert!(p.last_season.is_none());
        assert!(!serde_json::to_string(&p).unwrap().contains("last_season"));
    }

    #[test]
    fn tournaments_are_additive_in_rooms() {
        // an older server's room and lobby entry: no tournament
        let r: RoomSummary = serde_json::from_str(
            r#"{"code":"A","name":"n","host":"h","rules":{},"players":1,"spectators":0,"max_spectators":2,"locked":false,"in_match":false}"#,
        )
        .unwrap();
        assert!(r.tournament.is_none());
        assert!(!serde_json::to_string(&r).unwrap().contains("tournament"));
        let t = TournamentView {
            size: 8,
            state: "running".into(),
            live: Some([1, 0]),
            ..Default::default()
        };
        let back: TournamentView =
            serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        assert_eq!(TournamentView::round_name(3, 0), "QUARTER-FINALS");
        assert_eq!(TournamentView::round_name(3, 2), "FINAL");
        assert_eq!(TournamentView::round_name(4, 0), "ROUND OF 16");
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
