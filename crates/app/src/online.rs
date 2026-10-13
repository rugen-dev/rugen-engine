//! ONLINE screens on the UI engine (`motif/ui/online.toml`).
//!
//! 1. **Servers**: the built-in **official** server, pinned first (it holds
//!    the global ranking and the global cheat blocklist), then the
//!    **community** servers of `[net] servers` with name, MOTD, players
//!    online and ping; ADD SERVER / REMOVE SERVER edit the community list.
//!    A server is shown as OFFICIAL only once it signed our nonce with the
//!    official key (see [`crate::official`]); ranked, leaderboard and
//!    ratings are labelled OFFICIAL or COMMUNITY (server name) everywhere.
//! 2. **Lobby** of the chosen server: public rooms (join / watch), create a
//!    room (name, public / private, password, rules, spectator seats), join
//!    by code (+ password), quick match, ranked, leaderboard.
//! 3. Every match opens a **matchup** first (bring your own character):
//!    pick a local fighter (the select screen in online mode), see both
//!    cards and the server's instant verdict (FAIR / UNBALANCED / BLOCKED),
//!    confirm or change fighter while the packages sync in the background
//!    (see [`crate::matchup`]), then VS and the fight with the synced files.
//! 4. Matches are **relayed by the server** (UDP fast path, WebSocket
//!    fallback): no player ever learns another's address.
//!
//! 5. **PROFILE** (from the home menu or the server list): the account on
//!    every server, official first. Offline play needs none; rooms, ranked
//!    and the leaderboard need one on that server: a username (unique
//!    there) and a password, created or logged into here, or an anonymous
//!    account of an older version **claimed** (it keeps its id, rating and
//!    history). Entering a server without an account opens this page
//!    instead of registering silently (servers without named accounts, and
//!    the `rugen play --server` harness, still register anonymously).
//!    Only the account key is stored (`profile.json`), never the password.
//!
//! DIRECT MATCH (LAN / IP) stays available without any server: each player
//! types the other's address (explicit opt-in).
//!
//! All server calls run on worker threads ([`Pending`]) so frames never wait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context as _;
use rugen_content::Library;
use rugen_net::online::{
    AccountInfo, Evidence, Leaderboard, MatchInfo, OnlineClient, OnlineResult, Pending, Presence,
    ProfileInfo, QueueStatus, Registered, ResultAck, RoomInfo, RoomOptions, RoomSummary, Rules,
    ServerInfo, ServerMsg, Session, base_url, ping_bars, rank_name,
};
use rugen_render::{Camera, Renderer};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, ScreenDef, Ui};
use serde_json::{Value, json};
use winit::keyboard::Key;

use crate::config::{Config, GOOD_PING_MS};
use crate::game::Transition;
use crate::matchup::{Driver, Shared, Xfer, XferState};
use crate::official::{self, Stamp, Trust};
use crate::play::{FightSetup, NetMode};
use crate::system::SystemData;
use crate::uiscreen::{Cached, Host, MemImage, MouseEvent, Profile, viewport};
use crate::versus::Portrait;
use rugen_net::online::SignedBlob;
use rugen_net::online::{FighterCard, VerdictKind};

/// Which online match a fight belongs to, for result reporting.
#[derive(Clone, Debug)]
pub struct OnlineMatch {
    pub server: String,
    pub match_id: u64,
    /// Account ids by side (index 0 = P1).
    pub players: [u64; 2],
    pub my_side: usize,
    pub ranked: bool,
    /// Watching only: nothing to report.
    pub spectator: bool,
    pub rules: Rules,
}

/// Where the ONLINE screen resumes after a match: (server, room code).
static RESUME: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);

/// Fighter brought to the last matchup (picked again for a rematch).
static LAST_PICK: Mutex<Option<String>> = Mutex::new(None);

/// The last reported online result: (server, match id, my account, ack).
/// Shown on the ONLINE screen with its signature stamp; fetched again
/// while the opponent has not reported yet.
pub struct LastResult {
    pub server: String,
    pub match_id: u64,
    pub me: u64,
    pub ranked: bool,
    pub won: bool,
    /// The match ended early (a flee or a disconnect).
    pub ending: Option<rugen_proto::Ending>,
    pub ack: Option<ResultAck>,
}

static LAST_RESULT: Mutex<Option<LastResult>> = Mutex::new(None);

/// Match ids already played from this machine (a stale room view never restarts one).
static STARTED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn started_before(id: u64) -> bool {
    STARTED.lock().is_ok_and(|s| s.contains(&id))
}

/// `rugen play <char> --server URL --room CODE|new`: the ONLINE screen
/// drives itself (create / join, pick, confirm unless BLOCKED) and the game
/// quits after the match.
#[derive(Clone, Debug, Default)]
pub struct OnlineAuto {
    pub server: String,
    /// Room code, or `new` to create one.
    pub room: String,
    pub char_id: String,
    pub stage: Option<String>,
    pub spectate: bool,
    /// Throwaway account (not saved to the profile).
    pub guest: bool,
}

/// A matchup on screen: the worker and what the match needs afterwards.
struct Mx {
    driver: Driver,
    /// Account ids by side (index 0 = P1).
    players: [u64; 2],
    ranked: bool,
    room: Option<String>,
    snap: Shared,
    /// We still have to pick (the select screen opens).
    need_pick: bool,
    /// Ticks since the verdict appeared (auto mode).
    verdict_ticks: u32,
    /// When the verdict appeared (screenshots).
    verdict_at: Option<std::time::Instant>,
    /// Ticks the screen has been settled (verdict shown, files synced or blocked).
    settled_ticks: u32,
    /// Auto mode: last confirm sent (tick).
    auto_confirm_at: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Servers,
    AddServer,
    /// Picking a community server to remove.
    RemoveServer,
    Menu,
    Lobby,
    Create,
    Join,
    Room,
    Queue,
    Board,
    /// Typing the peer's address for a direct (LAN / IP) match.
    Direct,
    /// Bring-your-own-character matchup before a match.
    Matchup,
    /// PROFILE: the account on every server.
    Profile,
    /// One server's account: status and actions.
    Account,
    /// Typing a username / password ([`AcctForm`]).
    AccountForm,
}

/// What the account form does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcctForm {
    Create,
    Login,
    /// Give this device's anonymous account a username and password.
    Claim,
    Password,
}

impl AcctForm {
    fn title(self) -> &'static str {
        match self {
            Self::Create => "CREATE ACCOUNT",
            Self::Login => "LOG IN",
            Self::Claim => "CLAIM THIS ACCOUNT",
            Self::Password => "CHANGE PASSWORD",
        }
    }

    fn submit(self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Login => "LOG IN",
            Self::Claim => "CLAIM",
            Self::Password => "CHANGE",
        }
    }
}

/// Why the account form cannot be sent yet (checked again by the server).
fn form_error(kind: AcctForm, user: &str, pass: &str, new: &str) -> Option<&'static str> {
    use rugen_proto::account::{password_error, username_error};
    match kind {
        AcctForm::Login if user.is_empty() || pass.is_empty() => {
            Some("TYPE YOUR USERNAME AND PASSWORD")
        }
        AcctForm::Login => None,
        AcctForm::Create | AcctForm::Claim => username_error(user).or_else(|| password_error(pass)),
        AcctForm::Password if pass.is_empty() => Some("TYPE YOUR CURRENT PASSWORD"),
        AcctForm::Password => password_error(new),
    }
}

/// What we know about this device's account on one server.
#[derive(Clone, Debug, Default)]
struct AcctView {
    checking: bool,
    info: Option<AccountInfo>,
    /// The server has no named accounts (older server): anonymous only.
    unsupported: bool,
    error: String,
}

/// The account on a server, as the PROFILE page shows it.
#[derive(Clone, Debug, PartialEq)]
enum AcctState {
    /// No key on this device.
    None,
    Checking,
    /// A key without a username (claimable).
    Anonymous,
    /// Logged in as this username.
    Named(String),
}

/// Keep a session's key (and username) for `server`. The password never
/// reaches the profile.
fn remember_session(profile: &mut Profile, server: &str, s: &Session) {
    profile
        .online_accounts
        .insert(server.to_string(), (s.id, s.key.clone()));
    if s.username.is_empty() {
        profile.online_names.remove(server);
    } else {
        profile
            .online_names
            .insert(server.to_string(), s.username.clone());
    }
}

/// Forget this device's account on `server` (logged out, or its key was
/// revoked), including the legacy single key when it belongs there.
fn forget_session(profile: &mut Profile, config: &Config, server: &str) {
    profile
        .online_accounts
        .retain(|k, _| !official::same_server(k, server));
    profile
        .online_names
        .retain(|k, _| !official::same_server(k, server));
    let legacy = if config.net.server.trim().is_empty() {
        official::server()
    } else {
        config.net.server.trim()
    };
    if official::same_server(server, legacy) {
        profile.online_key.clear();
        profile.online_id = 0;
    }
}

/// A name with the verified badge when it is a username.
fn badge_name(p: &ProfileInfo) -> String {
    let n = p.name.to_uppercase();
    if p.verified { format!("{n} ✓") } else { n }
}

enum Job {
    Register(Pending<Registered>),
    /// Account status of a server (PROFILE).
    Account(String, Pending<AccountInfo>),
    /// Account form sent to a server.
    Session(String, AcctForm, Pending<Session>),
    /// Log out (`true`: everywhere) on a server.
    Logout(String, bool, Pending<()>),
    Info(usize, Pending<(ServerInfo, u32, Option<SignedBlob>)>),
    /// The official truth feed (from the official server or a mirror).
    Truth(Pending<SignedBlob>),
    /// Our last result again (signed once the opponent reported).
    Result(Pending<ResultAck>),
    Me(Pending<ProfileInfo>),
    Rooms(Pending<Vec<RoomSummary>>),
    Room(Pending<RoomInfo>),
    Queue(Pending<QueueStatus>),
    Board(Pending<Leaderboard>),
    Done(Pending<()>),
}

/// Why a server cannot be used by this game: too old for .rgc (API 4
/// only), or newer than this game. `None` when it serves this API, which
/// includes transition servers serving 0.5 players too (`api_min`).
fn outdated_error(server: &str, info: &ServerInfo) -> Option<String> {
    let mine = rugen_proto::API_VERSION;
    if info.serves(mine) {
        None
    } else if info.api < mine {
        Some(format!(
            "{} RUNS AN OLDER VERSION (API {}, THIS GAME NEEDS {mine}): IT CANNOT TAKE .RGC FIGHTERS",
            server.to_uppercase(),
            info.api
        ))
    } else {
        Some(format!(
            "{} RUNS A NEWER VERSION (API {}, THIS GAME SPEAKS {mine}): UPDATE YOUR GAME",
            server.to_uppercase(),
            info.min_api()
        ))
    }
}

/// Why a lobby room cannot be joined by this game (its host runs another
/// version), `None` when it can.
fn room_error(room: &RoomSummary) -> Option<String> {
    let mine = rugen_proto::API_VERSION;
    if room.joinable_by(mine) {
        None
    } else if room.api < mine {
        Some(format!(
            "THIS ROOM'S HOST RUNS AN OLDER RUGEN ({}). BOTH PLAYERS NEED THE SAME VERSION.",
            rugen_proto::release_of(room.api)
        ))
    } else {
        Some(format!(
            "THIS ROOM'S HOST RUNS A NEWER RUGEN ({}). UPDATE YOUR GAME TO JOIN.",
            rugen_proto::release_of(room.api)
        ))
    }
}

/// One row of the server list.
#[derive(Clone, Debug, Default)]
struct ServerRow {
    addr: String,
    /// The built-in official entry (pinned, cannot be removed).
    official: bool,
    info: Option<ServerInfo>,
    ping: Option<u32>,
    error: bool,
    /// Who the server proved to be (identity nonce).
    trust: Trust,
    nonce: String,
}

/// The create-room form.
#[derive(Clone, Debug)]
struct Form {
    name: String,
    password: String,
    public: bool,
    rules: Rules,
    spectators: u8,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            name: String::new(),
            password: String::new(),
            public: true,
            rules: Rules::default(),
            spectators: 2,
        }
    }
}

pub struct OnlineScreen {
    ui: Ui,
    motif: PathBuf,
    textures: HashMap<String, Cached>,
    window: (u32, u32),
    mouse_px: Option<(f32, f32)>,
    client: OnlineClient,
    presence: Option<Presence>,
    lib: Library,
    config: Config,
    profile: Profile,
    root: PathBuf,
    state: State,
    status: String,
    error: String,
    servers: Vec<ServerRow>,
    me: Option<ProfileInfo>,
    rooms: Vec<RoomSummary>,
    room: Option<RoomInfo>,
    /// Ticket already used for a fight (so a stale poll does not restart it).
    used_ticket: Option<u32>,
    typed: String,
    password: String,
    form: Form,
    queue: Option<QueueStatus>,
    ranked: bool,
    board: Option<Leaderboard>,
    jobs: Vec<Job>,
    tick: u32,
    /// Set when a match is about to start; the shell takes it.
    pub started: Option<OnlineMatch>,
    /// Direct match: our side (0 = host / 1P) and the typed peer address.
    direct_side: usize,
    addr: String,
    /// This machine's LAN address, for the other player to type.
    local_ip: String,
    pending_fight: Option<FightSetup>,
    /// A synced online match: VS screen, then the fight.
    pending_versus: Option<(FightSetup, [Portrait; 2])>,
    mx: Option<Mx>,
    /// The fighter pick screen should open (the shell parks this screen).
    want_select: bool,
    /// Card portraits (`mem:card_<fingerprint>`).
    mem: HashMap<String, MemImage>,
    auto: Option<OnlineAuto>,
    auto_joined: bool,
    /// Auto mode: leave the game (error, blocked) at this tick.
    auto_quit_at: Option<u32>,
    /// `--shot` pending: keep the matchup screen up once synced (it is the picture).
    pub hold_for_shot: bool,
    /// CONFIRM was visible last refresh (focus moves onto it when it appears).
    confirm_focused: bool,
    /// Server row to focus once the list is laid out (the default server).
    pending_focus: Option<usize>,
    /// PROFILE: this device's account per server address.
    accounts: HashMap<String, AcctView>,
    /// The server (row) the account page and form act on.
    acct_server: usize,
    acct_form: AcctForm,
    acct_user: String,
    /// Typed passwords: only sent, never stored; cleared with the form.
    acct_pass: String,
    acct_new: String,
    acct_show: bool,
    acct_error: String,
    /// PROFILE was opened from the home menu (BACK returns there).
    profile_from_title: bool,
    /// The account page opened because a server needs an account: go on
    /// into that server once logged in.
    connect_after: bool,
}

/// Account (id, key) for `server`; the legacy single key belongs to `[net] server`.
fn account_for(profile: &Profile, config: &Config, server: &str) -> (u64, String) {
    if let Some((id, key)) = profile.online_accounts.get(server) {
        return (*id, key.clone());
    }
    let legacy = if config.net.server.trim().is_empty() {
        official::server()
    } else {
        config.net.server.trim()
    };
    if official::same_server(server, legacy) && !profile.online_key.is_empty() {
        return (profile.online_id, profile.online_key.clone());
    }
    (0, String::new())
}

fn role_name(role: u8) -> &'static str {
    match role {
        0 => "HOST",
        1 => "CHALLENGER",
        _ => "SPECTATOR",
    }
}

fn rules_line(r: &Rules) -> String {
    format!(
        "{} ROUNDS  {}  {}  {}",
        r.rounds,
        if r.round_time == 0 {
            "NO TIMER".to_string()
        } else {
            format!("{} S", r.round_time)
        },
        r.mode.to_uppercase(),
        if r.chars == "mirror" {
            "MIRROR"
        } else {
            "FREE PICK"
        },
    )
}

fn ping_text(ms: Option<u32>) -> String {
    match ms {
        Some(ms) => format!("{ms} MS {}", "|".repeat(ping_bars(ms) as usize)),
        None => "-- MS".into(),
    }
}

impl OnlineScreen {
    pub fn load(motif: &Path, root: &Path, lib: &Library, config: &Config) -> anyhow::Result<Self> {
        let path = crate::uiscreen::motif_file(motif, "ui/online.toml")
            .unwrap_or_else(|| motif.join("ui/online.toml"));
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let def = ScreenDef::parse(&text).with_context(|| path.display().to_string())?;
        let profile = Profile::load(root);
        let mut s = Self {
            ui: Ui::new(def, Value::Null),
            motif: motif.to_path_buf(),
            textures: HashMap::new(),
            window: (1280, 720),
            mouse_px: None,
            client: OnlineClient::new("", None),
            presence: None,
            lib: lib.clone(),
            config: config.clone(),
            profile,
            root: root.to_path_buf(),
            state: State::Servers,
            status: String::new(),
            error: String::new(),
            servers: config
                .net
                .server_list(official::server())
                .into_iter()
                .map(|e| ServerRow {
                    addr: e.addr,
                    official: e.official,
                    ..Default::default()
                })
                .collect(),
            me: None,
            rooms: Vec::new(),
            room: None,
            used_ticket: None,
            typed: String::new(),
            password: String::new(),
            form: Form::default(),
            queue: None,
            ranked: true,
            board: None,
            jobs: Vec::new(),
            tick: 0,
            started: None,
            pending_fight: None,
            pending_versus: None,
            mx: None,
            want_select: false,
            mem: HashMap::new(),
            auto: None,
            auto_joined: false,
            auto_quit_at: None,
            hold_for_shot: false,
            confirm_focused: false,
            pending_focus: None,
            accounts: HashMap::new(),
            acct_server: 0,
            acct_form: AcctForm::Create,
            acct_user: String::new(),
            acct_pass: String::new(),
            acct_new: String::new(),
            acct_show: false,
            acct_error: String::new(),
            profile_from_title: false,
            connect_after: false,
            direct_side: 0,
            addr: String::new(),
            local_ip: rugen_net::socket::lan_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "YOUR-IP".into()),
        };
        s.status = "PICK A SERVER: OFFICIAL OR COMMUNITY".into();
        official::init(&lib.cache_dir());
        s.probe_servers();
        s.fetch_truth(None);
        s.poll_last_result();
        // back from a match: straight into the same server (and room)
        if let Some((server, room)) = RESUME.lock().ok().and_then(|mut r| r.take()) {
            s.connect(&server);
            if let Some(code) = room {
                s.state = State::Room;
                s.status = format!("ROOM {code}");
                let c = s.client.clone();
                s.jobs
                    .push(Job::Room(Pending::spawn(move || c.poll_room(&code))));
            }
        }
        s.refresh();
        if s.state == State::Servers {
            let default = s.config.net.default_index(official::server());
            s.pending_focus = (default > 0).then_some(default);
        }
        Ok(s)
    }

    /// Download the official truth feed (blocklist + ratings): from the
    /// official server, or the mirror of `via` (a community server).
    fn fetch_truth(&mut self, via: Option<&str>) {
        let c = OnlineClient::new(via.unwrap_or(official::server()), None);
        let mirror = via.is_some();
        self.jobs.push(Job::Truth(Pending::spawn(move || {
            if mirror {
                c.official_mirror()
            } else {
                c.truth()
            }
        })));
    }

    /// The last result is still waiting for the opponent's report: ask again.
    fn poll_last_result(&mut self) {
        let Some((server, id)) = LAST_RESULT.lock().ok().and_then(|l| {
            l.as_ref()
                .filter(|r| {
                    r.ack
                        .as_ref()
                        .is_none_or(|a| a.signed.is_none() && !a.disputed)
                })
                .map(|r| (r.server.clone(), r.match_id))
        }) else {
            return;
        };
        let (_, key) = account_for(&self.profile, &self.config, &server);
        if key.is_empty() {
            return;
        }
        let c = OnlineClient::new(&server, Some(key));
        self.jobs
            .push(Job::Result(Pending::spawn(move || c.match_result(id))));
    }

    /// What we know about a server's operator.
    fn trust_of(&self, addr: &str) -> Trust {
        self.servers
            .iter()
            .find(|r| official::same_server(&r.addr, addr))
            .map(|r| r.trust.clone())
            .unwrap_or_default()
    }

    /// Ranking scope of the connected server: `OFFICIAL` (global) or
    /// `COMMUNITY · NAME` (local to that server).
    fn scope(&self) -> String {
        match self.trust_of(&self.client.server) {
            Trust::Official => "OFFICIAL".into(),
            Trust::Impostor => format!("UNVERIFIED · {}", self.server_name().to_uppercase()),
            _ => format!("COMMUNITY · {}", self.server_name().to_uppercase()),
        }
    }

    /// The last result line (signature stamp included).
    fn last_result_line(&self) -> String {
        let Ok(l) = LAST_RESULT.lock() else {
            return String::new();
        };
        let Some(r) = l.as_ref() else {
            return String::new();
        };
        let what = if r.ranked { "RANKED" } else { "MATCH" };
        let outcome = match (r.won, r.ending) {
            (true, Some(rugen_proto::Ending::Disconnect)) => "WIN (OPPONENT DISCONNECTED)",
            (true, Some(_)) => "WIN (OPPONENT FLED)",
            (false, Some(_)) => "LOSS (YOU FLED)",
            (true, None) => "WIN",
            (false, None) => "LOSS",
        };
        match &r.ack {
            None => format!("LAST {what}: {outcome} - REPORTING..."),
            Some(a) if a.disputed => format!("LAST {what}: {outcome} - DISPUTED, NOT COUNTED"),
            Some(a) if a.signed.is_none() && a.delta.is_none() => {
                format!("LAST {what}: {outcome} - WAITING FOR THE OPPONENT'S REPORT")
            }
            Some(a) => {
                let stamp = official::result_stamp(a, r.match_id, r.me);
                let rating = match (r.ranked, a.rating, a.delta) {
                    (true, Some(rt), Some(d)) => format!(" {d:+} -> {rt}"),
                    _ => String::new(),
                };
                format!("LAST {what}: {outcome}{rating} · {}", stamp.label("RESULT"))
            }
        }
    }

    /// `rugen play --server`: connect straight to `auto.server` and drive the flow.
    pub fn load_auto(
        motif: &Path,
        root: &Path,
        lib: &Library,
        config: &Config,
        auto: OnlineAuto,
    ) -> anyhow::Result<Self> {
        if let Ok(mut r) = RESUME.lock() {
            *r = None;
        }
        let mut s = Self::load(motif, root, lib, config)?;
        s.jobs.clear();
        let server = auto.server.clone();
        // still learn who runs the server (OFFICIAL / COMMUNITY labels)
        if !s
            .servers
            .iter()
            .any(|r| official::same_server(&r.addr, &server))
        {
            s.servers.push(ServerRow {
                addr: server.clone(),
                official: official::is_official_addr(&server),
                ..Default::default()
            });
        }
        s.probe_servers();
        s.fetch_truth(None);
        s.auto = Some(auto);
        s.connect(&server);
        s.refresh();
        Ok(s)
    }

    /// PROFILE straight from the home menu.
    pub fn load_profile(
        motif: &Path,
        root: &Path,
        lib: &Library,
        config: &Config,
    ) -> anyhow::Result<Self> {
        if let Ok(mut r) = RESUME.lock() {
            *r = None;
        }
        let mut s = Self::load(motif, root, lib, config)?;
        s.profile_from_title = true;
        s.open_profile();
        s.refresh();
        Ok(s)
    }

    // ---- PROFILE: accounts

    /// The PROFILE page: every server's account, checked again.
    fn open_profile(&mut self) {
        self.state = State::Profile;
        self.ui.set_focus(None);
        self.connect_after = false;
        self.clear_form();
        self.error.clear();
        self.status = "PICK A SERVER TO CREATE AN ACCOUNT OR LOG IN".into();
        self.check_accounts();
    }

    /// Ask every server we hold a key for who that key is.
    fn check_accounts(&mut self) {
        for row in &self.servers {
            let (_, key) = account_for(&self.profile, &self.config, &row.addr);
            let view = self.accounts.entry(row.addr.clone()).or_default();
            if key.is_empty() {
                view.info = None;
                view.checking = false;
                continue;
            }
            view.checking = true;
            let c = OnlineClient::new(&row.addr, Some(key));
            self.jobs.push(Job::Account(
                row.addr.clone(),
                Pending::spawn(move || c.account()),
            ));
        }
    }

    /// Whether `addr` offers named accounts (unknown yet: assume it does).
    fn has_accounts(&self, addr: &str) -> bool {
        let row = self.servers.iter().find(|r| r.addr == addr);
        let unsupported = self.accounts.get(addr).is_some_and(|v| v.unsupported);
        !unsupported && row.and_then(|r| r.info.as_ref()).is_none_or(|i| i.accounts)
    }

    /// This device's account on server row `i`.
    fn acct_state(&self, i: usize) -> AcctState {
        let Some(row) = self.servers.get(i) else {
            return AcctState::None;
        };
        let (_, key) = account_for(&self.profile, &self.config, &row.addr);
        if key.is_empty() {
            return AcctState::None;
        }
        let view = self.accounts.get(&row.addr);
        match view.and_then(|v| v.info.as_ref()) {
            Some(info) if info.named() => AcctState::Named(info.username.clone()),
            Some(_) => AcctState::Anonymous,
            None if view.is_some_and(|v| v.checking) => AcctState::Checking,
            // offline: what we last knew
            None => match self.profile.online_names.get(&row.addr) {
                Some(n) => AcctState::Named(n.clone()),
                None => AcctState::Anonymous,
            },
        }
    }

    /// One server's account page.
    fn open_account(&mut self, i: usize) {
        if i >= self.servers.len() {
            return;
        }
        self.acct_server = i;
        self.state = State::Account;
        self.ui.set_focus(None);
        self.clear_form();
        self.error.clear();
        let addr = self.servers[i].addr.clone();
        self.status = match self.acct_state(i) {
            AcctState::Named(n) => format!("LOGGED IN AS {}", n.to_uppercase()),
            AcctState::Anonymous => "THIS ACCOUNT HAS NO USERNAME YET".into(),
            _ if !self.has_accounts(&addr) => "THIS SERVER HAS NO USERNAMES".into(),
            _ => "NOT LOGGED IN".into(),
        };
    }

    fn clear_form(&mut self) {
        self.acct_user.clear();
        self.acct_pass.clear();
        self.acct_new.clear();
        self.acct_show = false;
        self.acct_error.clear();
        for id in ["acct_user", "acct_pass", "acct_new"] {
            self.ui.set_input(id, "");
        }
    }

    fn open_form(&mut self, kind: AcctForm) {
        self.clear_form();
        self.acct_form = kind;
        self.state = State::AccountForm;
        self.error.clear();
        self.status = kind.title().into();
        self.refresh();
        self.ui.edit(if kind == AcctForm::Password {
            "acct_pass"
        } else {
            "acct_user"
        });
    }

    fn acct_addr(&self) -> String {
        self.servers
            .get(self.acct_server)
            .map(|r| r.addr.clone())
            .unwrap_or_default()
    }

    fn submit_form(&mut self) {
        let kind = self.acct_form;
        let (user, pass, new) = (
            self.acct_user.trim().to_string(),
            self.acct_pass.clone(),
            self.acct_new.clone(),
        );
        if let Some(e) = form_error(kind, &user, &pass, &new) {
            self.acct_error = e.into();
            return;
        }
        if self.jobs.iter().any(|j| matches!(j, Job::Session(..))) {
            return;
        }
        let addr = self.acct_addr();
        let (_, key) = account_for(&self.profile, &self.config, &addr);
        let mut c = OnlineClient::new(&addr, (!key.is_empty()).then_some(key));
        self.acct_error.clear();
        self.status = "SENDING...".into();
        let job = Pending::spawn(move || match kind {
            AcctForm::Create => c.create_account(&user, &pass),
            AcctForm::Login => c.login(&user, &pass),
            AcctForm::Claim => c.claim(&user, &pass),
            AcctForm::Password => c.change_password(&pass, &new),
        });
        self.jobs.push(Job::Session(addr, kind, job));
    }

    fn logout(&mut self, everywhere: bool) {
        let addr = self.acct_addr();
        let (_, key) = account_for(&self.profile, &self.config, &addr);
        if key.is_empty() {
            return;
        }
        let c = OnlineClient::new(&addr, Some(key));
        self.status = "LOGGING OUT...".into();
        self.jobs.push(Job::Logout(
            addr,
            everywhere,
            Pending::spawn(move || c.logout(everywhere)),
        ));
    }

    /// A server's account changed (`key`: the device's key now, empty when
    /// logged out): the connected client follows.
    fn account_changed(&mut self, addr: &str, key: &str) {
        if !official::same_server(&self.client.server, addr) {
            return;
        }
        self.presence = None;
        if key.is_empty() {
            self.client = OnlineClient::new("", None);
            self.me = None;
        } else {
            self.client.key = Some(key.to_string());
            self.presence = Presence::spawn(&self.client).ok();
            let (c, id) = (self.client.clone(), self.my_id());
            self.jobs
                .push(Job::Me(Pending::spawn(move || c.profile(id))));
        }
    }

    /// Auto mode: once registered, create or join the room.
    fn auto_join(&mut self) {
        let Some(a) = self.auto.clone() else { return };
        if self.auto_joined || self.client.key.is_none() {
            return;
        }
        self.auto_joined = true;
        if a.room.eq_ignore_ascii_case("new") {
            let opts = RoomOptions {
                name: format!("{} CLI", self.profile.name.to_uppercase()),
                public: true,
                rules: Rules {
                    stage: a.stage.clone().unwrap_or_default(),
                    ..Rules::default()
                },
                max_spectators: 2,
                ..Default::default()
            };
            let (c, ch) = (self.client.clone(), a.char_id.clone());
            self.state = State::Room;
            self.status = "CREATING ROOM".into();
            self.jobs
                .push(Job::Room(Pending::spawn(move || c.create_room(&ch, &opts))));
        } else {
            self.password.clear();
            self.join(a.room.to_ascii_uppercase(), a.spectate);
        }
    }

    /// Auto mode: print, then leave the game shortly (time to read / screenshot).
    fn auto_exit(&mut self, msg: &str) {
        if self.auto.is_some() && self.auto_quit_at.is_none() {
            eprintln!("{msg}");
            self.auto_quit_at = Some(self.tick + 180);
        }
    }

    /// The matchup screen is settled enough for a `--shot` (verdict shown,
    /// transfers finished or a moment passed).
    pub fn is_auto(&self) -> bool {
        self.auto.is_some()
    }

    pub fn shot_ready(&self) -> bool {
        // the UI draws the data of the previous tick: wait a few settled ticks
        self.state == State::Matchup && self.mx.as_ref().is_some_and(|m| m.settled_ticks >= 20)
    }

    fn settled(mx: &Mx) -> bool {
        let Some(at) = mx.verdict_at else {
            return false;
        };
        let blocked = mx.snap.verdict() == Some(VerdictKind::Blocked);
        let synced = mx.snap.ready.is_some()
            || (mx.snap.uploads.iter().all(Xfer::finished)
                && mx.snap.downloads.iter().all(Xfer::finished)
                && !mx.snap.downloads.is_empty());
        let t = at.elapsed().as_secs_f32();
        // harness: the confirm step is the picture
        let hold = std::env::var_os("RUGEN_HARNESS_NO_CONFIRM").is_some() && t >= 2.0;
        t >= 1.0 && (blocked || synced || hold || t >= 20.0)
    }

    /// Open the matchup phase of match `id`.
    fn start_matchup(&mut self, id: u64, players: [u64; 2], ranked: bool, room: Option<String>) {
        if let Some(old) = self.mx.take() {
            old.driver.leave();
        }
        log::info!(
            "online: matchup {id} ({})",
            if ranked { "ranked" } else { "casual" }
        );
        let driver = Driver::start(
            self.client.clone(),
            self.lib.clone(),
            id,
            self.config.net.stage.clone(),
        );
        // auto mode and rematches pick straight away (CHANGE FIGHTER stays available)
        let preset = self
            .auto
            .as_ref()
            .map(|a| a.char_id.clone())
            .or_else(|| LAST_PICK.lock().ok().and_then(|l| l.clone()))
            .filter(|c| self.lib.find_char(c).is_some());
        let stage = self.auto.as_ref().and_then(|a| a.stage.clone());
        if let Some(c) = &preset {
            driver.pick(c, stage);
        }
        self.mx = Some(Mx {
            driver,
            players,
            ranked,
            room,
            snap: Shared::default(),
            need_pick: preset.is_none(),
            verdict_ticks: 0,
            verdict_at: None,
            settled_ticks: 0,
            auto_confirm_at: 0,
        });
        self.error.clear();
        self.state = State::Matchup;
        self.status = match (&self.mx.as_ref().and_then(|m| m.room.clone()), ranked) {
            (Some(code), _) => format!("ROOM {code}  -  CASUAL: UNBALANCED FIGHTS NEED BOTH OK"),
            (None, true) => "RANKED: ONLY FAIR MATCHUPS".into(),
            (None, false) => "QUICK MATCH: UNBALANCED FIGHTS NEED BOTH OK".into(),
        };
        if preset.is_none() {
            self.want_select = true;
        }
    }

    /// Watching a running room match: fetch its files, then the fight.
    fn start_watching(&mut self, r: &RoomInfo) {
        if self.mx.as_ref().is_some_and(|m| m.driver.id == r.match_id) {
            return;
        }
        let host = r
            .members
            .iter()
            .find(|m| m.role == 0)
            .map_or(0, |m| m.profile.id);
        let chal = r
            .members
            .iter()
            .find(|m| m.role == 1)
            .map_or(0, |m| m.profile.id);
        let driver = Driver::start(
            self.client.clone(),
            self.lib.clone(),
            r.match_id,
            String::new(),
        );
        self.mx = Some(Mx {
            driver,
            players: [host, chal],
            ranked: false,
            room: Some(r.code.clone()),
            snap: Shared::default(),
            need_pick: false,
            verdict_ticks: 0,
            verdict_at: None,
            settled_ticks: 0,
            auto_confirm_at: 0,
        });
        self.state = State::Matchup;
        self.status = format!("ROOM {} - FETCHING THE FIGHTERS", r.code);
    }

    /// The shell's fighter pick (select screen in online mode) came back.
    pub fn picked(&mut self, char_id: &str, stage: Option<String>) {
        self.want_select = false;
        if let Some(mx) = self.mx.as_mut() {
            mx.need_pick = false;
            mx.driver.pick(char_id, stage);
            if let Ok(mut l) = LAST_PICK.lock() {
                *l = Some(char_id.to_string());
            }
        }
    }

    /// The pick screen was left without a pick: keep the previous fighter,
    /// or leave the matchup when there is none.
    pub fn pick_cancelled(&mut self) {
        self.want_select = false;
        if self.mx.as_ref().is_some_and(|m| m.need_pick) {
            self.leave_matchup("YOU LEFT THE MATCHUP");
        }
    }

    /// The pick screen could not open (theme without one): bring the main fighter.
    pub fn pick_default(&mut self) {
        let c = self.my_char();
        self.picked(&c, None);
    }

    fn leave_matchup(&mut self, why: &str) {
        if let Some(mx) = self.mx.take() {
            mx.driver.leave();
            self.status = why.into();
            self.state = if mx.room.is_some() && self.room.is_some() {
                State::Room
            } else {
                State::Menu
            };
        }
    }

    /// Follow the matchup worker: errors back to the room, VS once synced.
    fn poll_matchup(&mut self) {
        let Some(mx) = self.mx.as_mut() else { return };
        mx.snap = mx.driver.snapshot();
        if mx.snap.verdict().is_some()
            && mx
                .snap
                .view
                .as_ref()
                .is_some_and(|v| v.fighters.iter().all(Option::is_some))
        {
            mx.verdict_ticks += 1;
            mx.verdict_at.get_or_insert_with(std::time::Instant::now);
        } else {
            mx.verdict_ticks = 0;
            mx.verdict_at = None;
        }
        mx.settled_ticks = if Self::settled(mx) {
            mx.settled_ticks + 1
        } else {
            0
        };
        if let Some(e) = mx.snap.error.clone() {
            let mx = self.mx.take();
            let room = mx.as_ref().and_then(|m| m.room.clone());
            self.error = format!("MATCHUP: {e}").to_uppercase();
            self.status = room
                .as_ref()
                .map(|c| format!("ROOM {c} - ASK FOR A REMATCH TO TRY AGAIN"))
                .unwrap_or_default();
            self.state = if room.is_some() && self.room.is_some() {
                State::Room
            } else if self.client.server.is_empty() {
                State::Servers
            } else {
                State::Menu
            };
            self.auto_exit(&format!("matchup failed: {e}"));
            return;
        }
        if let Some(ready) = mx.snap.ready.clone()
            && !self.hold_for_shot
        {
            let mx = self.mx.take().expect("matchup");
            self.start_synced(ready, &mx);
            return;
        }
        // auto mode: confirm unless BLOCKED; leave when our own fighter is blocked
        if self.auto.is_some() {
            let tick = self.tick;
            let mut exit = None;
            if let Some(v) = &mx.snap.view {
                let blocked = mx.snap.verdict() == Some(VerdictKind::Blocked);
                let mine_blocked = mx
                    .snap
                    .mine
                    .as_ref()
                    .and_then(|m| m.profile.as_ref())
                    .is_some_and(|p| p.blocked());
                // harness: RUGEN_HARNESS_NO_CONFIRM keeps the confirm step on screen
                if v.phase == "confirm"
                    && !mx.snap.accepted
                    && !blocked
                    && tick >= mx.auto_confirm_at + 120
                    && std::env::var_os("RUGEN_HARNESS_NO_CONFIRM").is_none()
                {
                    mx.auto_confirm_at = tick;
                    mx.driver.accept();
                }
                if blocked && mine_blocked && mx.verdict_ticks == 1 {
                    let reasons = v
                        .verdict
                        .as_ref()
                        .map(|v| v.reasons.join("; "))
                        .unwrap_or_default();
                    exit = Some(format!("BLOCKED: {reasons}"));
                }
                if mx.verdict_ticks == 1
                    && let Some(verdict) = &v.verdict
                {
                    let names: Vec<String> = v
                        .fighters
                        .iter()
                        .flatten()
                        .map(|c| c.decl.name.clone())
                        .collect();
                    println!(
                        "MATCHUP: {}  →  {}",
                        names.join(" vs "),
                        verdict.kind.label()
                    );
                    for r in &verdict.reasons {
                        println!("  - {r}");
                    }
                }
            }
            if let Some(m) = exit {
                self.auto_exit(&m);
            }
        }
    }

    /// Every file is here and the relay ticket arrived: VS screen, then the fight.
    fn start_synced(&mut self, ready: crate::matchup::Ready, mx: &Mx) {
        let v = &ready.view;
        if let Ok(mut s) = STARTED.lock() {
            s.push(v.id);
        }
        if let Ok(mut r) = RESUME.lock() {
            *r = Some((self.client.server.clone(), mx.room.clone()));
        }
        let [p1, p2, stage] = ready.defs.clone();
        let m = OnlineMatch {
            server: self.client.server.clone(),
            match_id: v.id,
            players: mx.players,
            my_side: usize::from(v.side == 2),
            ranked: v.ranked,
            spectator: v.side > 2,
            rules: v.rules.clone(),
        };
        self.status = "MATCH STARTING".into();
        let side = |def: &PathBuf| {
            let (name, img) = crate::game::char_card(def);
            Portrait::new(name, img.map(|(w, h, rgba)| (w, h, rgba, [0, 0])))
        };
        let portraits = [side(&p1), side(&p2)];
        let setup = FightSetup {
            p1,
            p2,
            stage: Some(stage),
            ai: [0, 0],
            net: Some(NetMode::Relay {
                server: base_url(&self.client.server),
                ticket: ready.ticket.clone(),
                websocket_only: self.config.net.websocket_only,
            }),
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
        };
        self.used_ticket = Some(ready.ticket.session);
        self.pending_versus = Some((setup, portraits));
        self.started = Some(m);
    }

    /// A fighter card for the matchup screen.
    fn card_json(
        &mut self,
        card: Option<&FighterCard>,
        local: Option<&PathBuf>,
        status: &str,
    ) -> Value {
        let Some(c) = card else {
            return json!({ "present": false, "status": status, "has_portrait": false });
        };
        let d = &c.decl;
        let key = format!("card_{}", crate::matchup::short(&d.fingerprint));
        if !self.mem.contains_key(&key) {
            let img = local
                .and_then(|p| crate::uiscreen::load_image(p).ok())
                .or_else(|| {
                    d.portrait
                        .as_deref()
                        .and_then(rugen_net::online::b64_decode)
                        .and_then(|b| crate::uiscreen::decode_webp(&b).ok())
                });
            self.mem
                .insert(key.clone(), img.unwrap_or((0, 0, Vec::new())));
        }
        let has_portrait = self.mem.get(&key).is_some_and(|(w, _, _)| *w > 0);
        let p = d.profile.clone().unwrap_or_default();
        let here = if self.trust_of(&self.client.server).is_official() {
            "OFFICIAL"
        } else {
            "THIS SERVER"
        };
        let mut rating = match &c.rating {
            Some(r) if r.games > 0 => format!("{here} {}  ({} GAMES)", r.rating, r.games),
            _ => format!("{here}: UNRATED"),
        };
        if here != "OFFICIAL"
            && let Some((r, g)) = official::rating(&d.fingerprint)
        {
            rating = format!("{rating}  ·  OFFICIAL {r} ({g})");
        }
        let official_block = official::blocked(&d.fingerprint);
        let name = d.name.to_uppercase();
        let len = name.chars().count();
        json!({
            "present": true,
            "name": name,
            "long": len > 14,
            // matchup header: three sizes so any name fits its column
            "name_big": len <= 10,
            "name_mid": (11..=17).contains(&len),
            "name_small": len > 17,
            "author": if d.author.is_empty() { "BY UNKNOWN".to_string() } else { format!("BY {}", d.author.to_uppercase()) },
            "portrait": format!("mem:{key}"),
            "has_portrait": has_portrait,
            "stats1": format!("LIFE {}   ATTACK {}   DEFENCE {}", p.life, p.attack, p.defence),
            "stats2": format!("SUPERS {}   PROJECTILES {}   THROWS {}", p.supers, p.projectiles, p.throws),
            "stats3": format!("BEST HIT {}   COMBO ~{}", p.max_hit_damage, p.est_combo_damage),
            "power": format!("POWER {:.0}", p.power_score),
            "rating": rating,
            "verified": if official_block.is_some() { "BLOCKED BY THE OFFICIAL SERVER" } else if p.blocked() { "BLOCKED: NEVER SHARED" } else if c.verified { "FILES CHECKED BY THE SERVER" } else { "WAITING FOR THE SERVER CHECK" },
            "official_block": official_block.as_ref().map(|w| format!("OFFICIAL BLOCK: {w}")).unwrap_or_default(),
            "has_official_block": official_block.is_some(),
            "size": format!("{:.1} MB", d.size as f64 / 1048576.0),
            "status": status,
            "flags": p.flags.iter().map(|f| f.code.to_uppercase()).collect::<Vec<_>>().join(" "),
        })
    }

    /// One side's transfer / server check on the matchup panel: a short
    /// line and a 0-100 bar. `xfer` is this side's transfer (our upload,
    /// their download), `sending` names it.
    fn side_xfer(card: Option<&FighterCard>, xfer: &[Xfer], sending: &str) -> Value {
        let Some(c) = card else {
            return json!({ "text": "", "pct": 0 });
        };
        let blocked = c.decl.profile.as_ref().is_some_and(|p| p.blocked())
            || official::blocked(&c.decl.fingerprint).is_some();
        let failed = xfer.iter().any(|x| matches!(x.state, XferState::Failed(_)));
        let moving = !xfer.is_empty() && !xfer.iter().all(Xfer::finished);
        let frac = if xfer.is_empty() {
            0.0
        } else {
            xfer.iter().map(Xfer::fraction).sum::<f32>() / xfer.len() as f32
        };
        let (text, pct) = if blocked {
            ("BLOCKED".to_string(), 0)
        } else if failed {
            (format!("{sending} FAILED"), (frac * 100.0) as u32)
        } else if moving {
            let pct = (frac * 100.0) as u32;
            (format!("{sending} {pct}%"), pct)
        } else if c.verified {
            ("FILES CHECKED".to_string(), 100)
        } else {
            (
                "WAITING FOR SERVER CHECK".to_string(),
                (frac * 100.0) as u32,
            )
        };
        json!({ "text": text, "pct": pct })
    }

    /// The `mx` block of the data context (matchup screen).
    fn matchup_json(&mut self) -> Value {
        let Some(snap) = self.mx.as_ref().map(|m| m.snap.clone()) else {
            return json!({});
        };
        let ranked = self.mx.as_ref().is_some_and(|m| m.ranked);
        let need_pick = self.mx.as_ref().is_some_and(|m| m.need_pick);
        let v = snap.view.clone().unwrap_or_default();
        let spectator = v.side > 2;
        let me = usize::from(v.side == 2);
        let opp = 1 - me;
        let phase = v.phase.as_str();
        let status_of = |i: usize| -> String {
            let Some(card) = &v.fighters[i] else {
                return "PICKING...".into();
            };
            let blocked = card.decl.profile.as_ref().is_some_and(|p| p.blocked())
                || official::blocked(&card.decl.fingerprint).is_some();
            if blocked {
                "BLOCKED".into()
            } else if v.ready[i] {
                "READY".into()
            } else if v.accepted[i] {
                "CONFIRMED".into()
            } else if phase == "pick" {
                "WAITING".into()
            } else {
                "DECIDING...".into()
            }
        };
        let my_status = if snap.preparing {
            "PREPARING YOUR FIGHTER...".to_string()
        } else if need_pick {
            "PICKING...".to_string()
        } else {
            status_of(me)
        };
        let local = snap.mine_card.clone();
        let (left, right) = if spectator {
            (
                self.card_json(v.fighters[0].as_ref(), None, "P1"),
                self.card_json(v.fighters[1].as_ref(), None, "P2"),
            )
        } else {
            (
                self.card_json(v.fighters[me].as_ref(), local.as_ref(), &my_status),
                self.card_json(v.fighters[opp].as_ref(), None, &status_of(opp)),
            )
        };
        let verdict = v.verdict.clone();
        let kind = verdict.as_ref().map(|v| v.kind);
        let both = v.fighters.iter().all(Option::is_some);
        let mine_blocked = snap
            .mine
            .as_ref()
            .and_then(|m| m.profile.as_ref())
            .is_some_and(|p| p.blocked());
        let stamp = if both && kind.is_some() {
            official::verdict_stamp(&v)
        } else {
            Stamp::Unsigned
        };
        if let Stamp::Forged(e) = &stamp {
            log::warn!("online: verdict signature rejected: {e}");
        }
        let can_confirm = !spectator
            && phase == "confirm"
            && !snap.accepted
            && !(ranked && kind == Some(VerdictKind::Unbalanced));
        // the stat face-off: left = us (P1 when watching)
        let (li, ri) = if spectator { (0, 1) } else { (me, opp) };
        let stats = stat_rows(
            v.fighters[li]
                .as_ref()
                .and_then(|c| c.decl.profile.as_ref()),
            v.fighters[ri]
                .as_ref()
                .and_then(|c| c.decl.profile.as_ref()),
        );
        let (left_xfer, right_xfer) = if spectator {
            (
                Self::side_xfer(v.fighters[0].as_ref(), &[], ""),
                Self::side_xfer(v.fighters[1].as_ref(), &[], ""),
            )
        } else {
            (
                Self::side_xfer(v.fighters[me].as_ref(), &snap.uploads, "SENDING FIGHTER"),
                Self::side_xfer(
                    v.fighters[opp].as_ref(),
                    &snap.downloads,
                    "RECEIVING FIGHTER",
                ),
            )
        };
        let state_of = |s: String| s.trim_end_matches('.').to_string();
        let (left_state, right_state) = if spectator {
            (status_of(0), status_of(1))
        } else {
            (my_status.clone(), status_of(opp))
        };
        // a short rule only when this verdict needs something from someone
        let both_ok = v.accepted.iter().all(|a| *a);
        let rule = match kind {
            _ if spectator || !both => "",
            Some(VerdictKind::Blocked) if mine_blocked => "YOUR FIGHTER IS BLOCKED",
            Some(VerdictKind::Blocked) => "THEIR FIGHTER IS BLOCKED",
            Some(VerdictKind::Unbalanced) if ranked => "RANKED NEEDS A FAIR FIGHT",
            Some(VerdictKind::Unbalanced) if !both_ok => "UNBALANCED: BOTH MUST ACCEPT",
            _ => "",
        };
        let note = if snap.verdict_changed {
            "THE VERDICT CHANGED: CONFIRM AGAIN".to_string()
        } else if !snap.notice.is_empty() {
            snap.notice.to_uppercase()
        } else {
            rule.to_string()
        };
        let room = self.mx.as_ref().and_then(|m| m.room.clone());
        json!({
            "title": if spectator { "WATCHING".to_string() } else if ranked { format!("RANKED MATCHUP · {}", if self.trust_of(&self.client.server).is_official() { "OFFICIAL" } else { "COMMUNITY" }) } else { "MATCHUP".to_string() },
            "left": left,
            "right": right,
            "left_tag": if spectator { "P1" } else { "YOU" },
            "right_tag": if spectator { "P2" } else { "OPPONENT" },
            "has_verdict": both && kind.is_some(),
            "pending": !both || kind.is_none(),
            "verdict": kind.map(|k| k.label()).unwrap_or(""),
            "fair": both && kind == Some(VerdictKind::Fair),
            "unbalanced": both && kind == Some(VerdictKind::Unbalanced),
            "blocked": both && kind == Some(VerdictKind::Blocked),
            "stamp": if both && kind.is_some() { stamp.label("VERDICT") } else { String::new() },
            "stamp_official": stamp.is_official(),
            "stamp_other": both && kind.is_some() && !stamp.is_official(),
                        "can_confirm": can_confirm,
            // the stamp already says UNBALANCED; the label must fit its button
            "confirm_label": if kind == Some(VerdictKind::Unbalanced) { "ACCEPT" } else { "CONFIRM" },
            "can_change": !spectator && matches!(phase, "pick" | "confirm" | "") && !snap.preparing,
            "changed": snap.verdict_changed,
            "notice": if snap.verdict_changed { "THE VERDICT CHANGED: CONFIRM AGAIN".to_string() } else { snap.notice.to_uppercase() },
            "has_notice": !snap.notice.is_empty() || snap.verdict_changed,
            "st": stats,
            "left_xfer": left_xfer,
            "right_xfer": right_xfer,
            "left_state": state_of(left_state),
            "right_state": state_of(right_state),
            "note": note,
            "has_note": !note.is_empty(),
            "pending_label": if both { "JUDGING" } else { "WAITING FOR PICKS" },
            "conn": if spectator { json!({ "line": "WATCHING", "high_ping": false, "sub": "" }) } else { self.conn_json(ranked, room.as_deref()) },
        })
    }

    /// Ask every listed server for its info, measure the round trip and
    /// have it sign a fresh nonce (who holds which key).
    fn probe_servers(&mut self) {
        for (i, row) in self.servers.iter_mut().enumerate() {
            let c = OnlineClient::new(&row.addr, None);
            row.nonce = official::nonce();
            let nonce = row.nonce.clone();
            self.jobs.push(Job::Info(
                i,
                Pending::spawn(move || {
                    let info = c.info()?;
                    let ping = c.ping()?;
                    let id = if info.key.is_some() {
                        c.identity(&nonce).ok()
                    } else {
                        None
                    };
                    Ok((info, ping, id))
                }),
            ));
        }
    }

    /// Pick a server: account (register on first use), presence socket, menu.
    fn connect(&mut self, server: &str) {
        // fighters travel as .rgc files with rugen-fp-2 fingerprints since API 5
        let outdated = self
            .servers
            .iter()
            .find(|r| r.addr == server)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| outdated_error(server, i));
        if let Some(e) = outdated {
            self.error = e;
            return;
        }
        let (id, key) = account_for(&self.profile, &self.config, server);
        // a server with named accounts: create one or log in first (the
        // `rugen play --server` harness keeps registering anonymously)
        if key.is_empty()
            && self.auto.is_none()
            && self.has_accounts(server)
            && let Some(i) = self.servers.iter().position(|r| r.addr == server)
        {
            self.presence = None;
            self.client = OnlineClient::new("", None);
            self.open_account(i);
            self.connect_after = true;
            self.status = "THIS SERVER NEEDS AN ACCOUNT: CREATE ONE OR LOG IN".into();
            return;
        }
        self.client = OnlineClient::new(server, (!key.is_empty()).then_some(key));
        self.me = None;
        self.state = State::Menu;
        self.error.clear();
        self.status = format!("CONNECTING TO {}", server.to_uppercase());
        // no official feed yet (official server unreachable): its mirror here
        if official::feed().is_none() && !official::is_official_addr(server) {
            self.fetch_truth(Some(server));
        }
        if self.client.key.is_none() {
            let mut c = self.client.clone();
            self.jobs
                .push(Job::Register(Pending::spawn(move || c.register())));
        } else {
            self.presence = Presence::spawn(&self.client).ok();
            let c = self.client.clone();
            self.jobs
                .push(Job::Me(Pending::spawn(move || c.profile(id))));
        }
    }

    fn server_name(&self) -> String {
        self.servers
            .iter()
            .find(|r| r.addr == self.client.server)
            .and_then(|r| r.info.as_ref())
            .map(|i| i.name.clone())
            .unwrap_or_else(|| self.client.server.clone())
    }

    fn server_ping(&self) -> Option<u32> {
        self.servers
            .iter()
            .find(|r| r.addr == self.client.server)
            .and_then(|r| r.ping)
    }

    /// Character this player brings online: `profile.main_char`, else `[net] p1`, else the first installed.
    fn my_char(&self) -> String {
        let want = if self.profile.main_char.trim().is_empty() {
            self.config.net.p1.clone()
        } else {
            self.profile.main_char.clone()
        };
        if self.lib.find_char(&want).is_some() {
            return want;
        }
        self.lib
            .chars()
            .first()
            .map(|e| e.id.clone())
            .unwrap_or(want)
    }

    fn my_id(&self) -> u64 {
        self.me.as_ref().map_or_else(
            || account_for(&self.profile, &self.config, &self.client.server).0,
            |m| m.id,
        )
    }

    /// The server's estimate of the round trip to the opponent.
    fn est_ping(&self) -> Option<u32> {
        self.room.as_ref().and_then(|r| r.est_ping_ms).or_else(|| {
            self.queue
                .as_ref()
                .and_then(|q| q.found.as_ref())
                .and_then(|m| m.est_ping_ms)
        })
    }

    /// Matchup screen, bottom left: how the match will connect, on one
    /// line (`DIRECT 492 MS  DELAY 7F`), whether the ping is high, and the
    /// small line under it (room, mode, what the path reveals).
    fn conn_json(&self, ranked: bool, room: Option<&str>) -> Value {
        let direct = self.config.net.direct;
        let how = if direct { "DIRECT" } else { "RELAYED" };
        let ping = self.est_ping();
        let line = match ping {
            Some(ms) => {
                let d = self
                    .config
                    .net
                    .input_delay
                    .frames(Some(ms), &self.config.net.delay_policy());
                format!("{how} {ms} MS DELAY {d}F")
            }
            None => how.to_string(),
        };
        let mut sub = Vec::new();
        match room {
            Some(code) => sub.push(format!("ROOM {code}")),
            None => sub.push("QUICK MATCH".into()),
        }
        sub.push(if ranked { "RANKED" } else { "CASUAL" }.into());
        sub.push(if direct {
            "PEER-TO-PEER, OPPONENT SEES YOUR IP".into()
        } else {
            "RELAYED THROUGH THE SERVER".into()
        });
        json!({
            "line": line,
            "high_ping": ping.is_some_and(|ms| ms > HIGH_PING_MS),
            "sub": sub.join(" · "),
        })
    }

    /// Display name of server row `i`.
    fn row_name(&self, i: usize) -> String {
        self.servers
            .get(i)
            .map(|r| {
                r.info
                    .as_ref()
                    .map_or(official::host_label(&r.addr).to_string(), |i| {
                        i.name.clone()
                    })
            })
            .unwrap_or_default()
            .to_uppercase()
    }

    /// The account card (account page and form).
    fn acct_json(&self) -> Value {
        let i = self.acct_server;
        let addr = self.acct_addr();
        let official = self
            .servers
            .get(i)
            .is_some_and(|r| r.official && r.trust != Trust::Impostor);
        let view = self.accounts.get(&addr);
        let profile = view.and_then(|v| v.info.as_ref()).map(|a| &a.profile);
        let state = self.acct_state(i);
        let supported = self.has_accounts(&addr);
        let title = match &state {
            AcctState::Named(n) => format!("{} ✓", n.to_uppercase()),
            AcctState::Anonymous => "NO USERNAME YET".into(),
            AcctState::Checking => "CHECKING...".into(),
            AcctState::None => "NOT LOGGED IN".into(),
        };
        let rating = profile
            .map(|p| format!("{} {}   BEST {}", p.rank, p.rating, p.best_rating))
            .unwrap_or_default();
        let record = profile
            .map(|p| {
                let devices = view.and_then(|v| v.info.as_ref()).map_or(0, |a| a.sessions);
                let mut line = format!("{}W {}L", p.wins, p.losses);
                if matches!(state, AcctState::Named(_)) && devices > 0 {
                    line.push_str(&format!(
                        "   LOGGED IN ON {devices} DEVICE{}",
                        if devices == 1 { "" } else { "S" }
                    ));
                }
                line
            })
            .unwrap_or_default();
        let hint = match &state {
            _ if !supported => "THIS SERVER HAS NO USERNAMES YET: YOU PLAY THERE ANONYMOUSLY.",
            AcctState::Named(_) => {
                "LOG IN WITH THIS USERNAME AND PASSWORD ON YOUR OTHER DEVICES. CHANGING THE PASSWORD LOGS THEM OUT."
            }
            AcctState::Anonymous => {
                "THIS DEVICE PLAYS ON AN ACCOUNT WITHOUT A USERNAME. CLAIM IT TO KEEP ITS RATING AND PLAY FROM OTHER DEVICES."
            }
            AcctState::Checking => "",
            AcctState::None => {
                "ONLINE ROOMS, RANKED AND THE LEADERBOARD NEED AN ACCOUNT ON THIS SERVER. OFFLINE PLAY NEEDS NONE."
            }
        };
        let kind = self.acct_form;
        let error = if self.acct_error.is_empty() {
            view.map(|v| v.error.clone()).unwrap_or_default()
        } else {
            self.acct_error.clone()
        };
        json!({
            "server": self.row_name(i),
            "scope": if official { "OFFICIAL SERVER · GLOBAL RANKING" } else { "COMMUNITY SERVER" },
            "official": official,
            "title": title,
            "rating": rating,
            "record": record,
            "hint": hint,
            "note": "NO EMAIL: KEEP YOUR PASSWORD SAFE",
            "form_title": format!("{} · {}", kind.title(), self.row_name(i).chars().take(18).collect::<String>()),
            "user": self.acct_user,
            "pass": self.acct_pass,
            "new": self.acct_new,
            "masked": !self.acct_show,
            "show_label": if self.acct_show { "HIDE" } else { "SHOW" },
            "has_user": kind != AcctForm::Password,
            "has_new": kind == AcctForm::Password,
            "pass_label": if kind == AcctForm::Password { "CURRENT PASSWORD" } else { "PASSWORD" },
            "rules": match kind {
                AcctForm::Create | AcctForm::Claim => "3-16 LETTERS, DIGITS, _ OR -   PASSWORD: 6+ CHARACTERS",
                AcctForm::Login => "USERNAMES IGNORE CASE",
                AcctForm::Password => "6+ CHARACTERS. YOUR OTHER DEVICES GET LOGGED OUT.",
            },
            "submit": kind.submit(),
            "error": error,
            "has_error": !error.is_empty(),
        })
    }

    /// PROFILE overview: one line per server.
    fn accts_json(&self) -> Vec<Value> {
        (0..self.servers.len())
            .map(|i| {
                let addr = &self.servers[i].addr;
                let profile = self
                    .accounts
                    .get(addr)
                    .and_then(|v| v.info.as_ref())
                    .map(|a| &a.profile);
                let status = match self.acct_state(i) {
                    AcctState::Named(n) => match profile {
                        Some(p) => format!("{} ✓  {} {}", n.to_uppercase(), p.rank, p.rating),
                        None => format!("{} ✓", n.to_uppercase()),
                    },
                    AcctState::Anonymous => "NO USERNAME (CLAIM IT)".into(),
                    AcctState::Checking => "CHECKING...".into(),
                    AcctState::None if self.has_accounts(addr) => "NOT LOGGED IN".into(),
                    AcctState::None => "ANONYMOUS ONLY".into(),
                };
                json!({
                    "server": self.row_name(i).chars().take(18).collect::<String>(),
                    "status": status,
                    "named": matches!(self.acct_state(i), AcctState::Named(_)),
                })
            })
            .collect()
    }

    fn refresh(&mut self) {
        let mx = if self.state == State::Matchup {
            self.matchup_json()
        } else {
            json!({})
        };
        let me = self.me.clone().unwrap_or_else(|| ProfileInfo {
            id: self.my_id(),
            name: self.profile.name.clone(),
            rating: 1200,
            best_rating: 1200,
            rank: rank_name(1200).into(),
            ..Default::default()
        });
        let room = self.room.as_ref();
        let opp = room
            .and_then(|r| r.opponent.clone())
            .or_else(|| {
                self.queue
                    .as_ref()
                    .and_then(|q| q.found.as_ref())
                    .map(|m| m.opponent.clone())
            })
            .unwrap_or_default();
        let opp_ping = self.est_ping();
        let scope = self.scope();
        let ranked_official = scope == "OFFICIAL";
        let scope_short = if ranked_official {
            "OFFICIAL"
        } else {
            "COMMUNITY"
        }
        .to_string();
        let board_rows: Vec<Value> = self
            .board
            .as_ref()
            .map(|b| {
                b.rows
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        json!({
                            "scope": scope_short.clone(),
                            "pos": i + 1,
                            "name": badge_name(p),
                            "rating": p.rating,
                            "rank": p.rank,
                            "record": format!("{}W {}L", p.wins, p.losses),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let my_rank = self
            .board
            .as_ref()
            .and_then(|b| b.me.as_ref())
            .map(|(pos, _)| format!("YOUR {scope_short} RANK #{pos}"))
            .unwrap_or_else(|| "UNRANKED - PLAY A RANKED MATCH".into());
        let has_server = self.client.key.is_some() || self.state != State::Servers;
        let menu: Vec<Value> = match self.state {
            State::Servers => {
                let mut m: Vec<Value> = self
                    .servers
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let name = |s: &str, n: usize| s.to_uppercase().chars().take(n).collect::<String>();
                        let label = match (&r.info, r.error) {
                            (Some(info), _) => format!("{}  {}", name(&info.name, 15), r.ping.map_or("--".into(), |p| format!("{p} MS"))),
                            (None, true) => format!("{} OFFLINE", name(official::host_label(&r.addr), 12)),
                            (None, false) => format!("{} ...", name(official::host_label(&r.addr), 17)),
                        };
                        let badge = match (r.official, &r.trust) {
                            (_, Trust::Impostor) => "UNVERIFIED",
                            (true, _) => "OFFICIAL",
                            (false, _) => "COMMUNITY",
                        };
                        json!({
                            "id": format!("srv{i}"), "label": label, "action": format!("server:{i}"),
                            "badge": badge, "has_badge": true,
                            "official": r.official && r.trust != Trust::Impostor,
                            "community": !r.official || r.trust == Trust::Impostor,
                        })
                    })
                    .collect();
                m.push(json!({ "id": "profile", "label": "PROFILE / ACCOUNTS", "action": "online:profile" }));
                m.push(json!({ "id": "add", "label": "ADD SERVER", "action": "online:add" }));
                if self.servers.iter().any(|r| !r.official) {
                    m.push(json!({ "id": "remove", "label": "REMOVE SERVER", "action": "online:remove" }));
                }
                m.extend([
                    json!({ "id": "host", "label": "HOST DIRECT (LAN)", "action": "online:host" }),
                    json!({ "id": "direct", "label": "JOIN DIRECT (LAN)", "action": "online:direct" }),
                ]);
                if !self.config.net.remote.trim().is_empty() {
                    m.push(json!({ "id": "netplay", "label": "NETPLAY (RUGEN.TOML)", "action": "netplay" }));
                }
                m.push(json!({ "id": "back", "label": "BACK", "action": "back" }));
                m
            }
            State::RemoveServer => {
                let mut m: Vec<Value> = self
                    .servers
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.official)
                    .map(|(i, r)| {
                        let name = r.info.as_ref().map_or(official::host_label(&r.addr).to_string(), |i| i.name.clone());
                        json!({
                            "id": format!("rm{i}"),
                            "label": format!("REMOVE {}", name.to_uppercase().chars().take(16).collect::<String>()),
                            "action": format!("server_remove:{i}"),
                            "badge": "COMMUNITY", "has_badge": true, "community": true, "official": false,
                        })
                    })
                    .collect();
                m.push(json!({ "id": "back", "label": "CANCEL", "action": "back" }));
                m
            }
            State::Profile => {
                let mut m: Vec<Value> = self
                    .servers
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let name = r
                            .info
                            .as_ref()
                            .map_or(official::host_label(&r.addr).to_string(), |i| {
                                i.name.clone()
                            });
                        let badge = match (r.official, &r.trust) {
                            (_, Trust::Impostor) => "UNVERIFIED",
                            (true, _) => "OFFICIAL",
                            (false, _) => "COMMUNITY",
                        };
                        json!({
                            "id": format!("acct{i}"),
                            "label": name.to_uppercase().chars().take(18).collect::<String>(),
                            "action": format!("acct:open:{i}"),
                            "badge": badge, "has_badge": true,
                            "official": badge == "OFFICIAL", "community": badge != "OFFICIAL",
                        })
                    })
                    .collect();
                m.push(json!({ "id": "back", "label": "BACK", "action": "back" }));
                m
            }
            State::Account => {
                let i = self.acct_server;
                let addr = self.acct_addr();
                let item = |id: &str, label: &str| json!({ "id": id, "label": label, "action": format!("acct:{id}") });
                let mut m = match self.acct_state(i) {
                    AcctState::Named(_) => vec![
                        item("password", "CHANGE PASSWORD"),
                        item("logout", "LOG OUT"),
                        item("logout_all", "LOG OUT EVERYWHERE"),
                    ],
                    AcctState::Anonymous if self.has_accounts(&addr) => vec![
                        item("claim", "CLAIM THIS ACCOUNT"),
                        item("login", "LOG IN INSTEAD"),
                    ],
                    AcctState::None if self.has_accounts(&addr) => {
                        vec![item("create", "CREATE ACCOUNT"), item("login", "LOG IN")]
                    }
                    _ => Vec::new(),
                };
                if self.connect_after && !matches!(self.acct_state(i), AcctState::None) {
                    m.insert(0, item("enter", "ENTER SERVER"));
                }
                m.push(json!({ "id": "back", "label": "BACK", "action": "back" }));
                m
            }
            _ => vec![
                json!({ "id": "lobby", "label": "LOBBY", "action": "online:lobby" }),
                json!({ "id": "room", "label": "CREATE ROOM", "action": "online:create" }),
                json!({ "id": "join", "label": "JOIN BY CODE", "action": "online:join" }),
                json!({ "id": "quick", "label": "QUICK MATCH", "action": "online:quick" }),
                json!({ "id": "ranked", "label": "RANKED", "action": "online:ranked" }),
                json!({ "id": "board", "label": "LEADERBOARD", "action": "online:board" }),
                json!({ "id": "servers", "label": "CHANGE SERVER", "action": "online:servers" }),
                json!({ "id": "back", "label": "BACK", "action": "back" }),
            ],
        };
        let acct = self.acct_json();
        let accts = self.accts_json();
        let lobby: Vec<Value> = self
            .rooms
            .iter()
            .map(|r| {
                json!({
                    "name": format!(
                        "{}{}{}",
                        if r.joinable_by(rugen_proto::API_VERSION) { String::new() } else { format!("[{}] ", rugen_proto::release_of(r.api)) },
                        if r.locked { "[LOCKED] " } else { "" },
                        r.name.to_uppercase()
                    ),
                    "host": if r.host_verified { format!("{} ✓", r.host.to_uppercase()) } else { r.host.to_uppercase() },
                    "rules": rules_line(&r.rules),
                    "count": format!(
                        "{}/2  WATCHING {}/{}{}",
                        r.players, r.spectators, r.max_spectators,
                        if r.in_match { "  LIVE" } else { "" }
                    ),
                    "join": format!("lobby:join:{}", r.code),
                    "watch": format!("lobby:watch:{}", r.code),
                })
            })
            .collect();
        let my_role = room.map_or(2, |r| r.side.saturating_sub(1));
        let members: Vec<Value> = room
            .map(|r| {
                r.members
                    .iter()
                    .map(|m| {
                        json!({
                            "name": badge_name(&m.profile),
                            "role": role_name(m.role),
                            "ping": ping_text(m.rtt_ms),
                            "kick": format!("room:kick:{}", m.profile.id),
                            "can_kick": my_role == 0 && m.role != 0,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let accounts_page = matches!(
            self.state,
            State::Profile | State::Account | State::AccountForm
        );
        let server_line = if accounts_page {
            "ACCOUNTS · ONE PER SERVER".to_string()
        } else if matches!(self.state, State::Servers | State::RemoveServer) {
            if official::feed().is_some() {
                "SERVERS  ·  OFFICIAL BLOCKLIST UP TO DATE".to_string()
            } else {
                "SERVERS".to_string()
            }
        } else {
            let info = self
                .servers
                .iter()
                .find(|r| r.addr == self.client.server)
                .and_then(|r| r.info.clone());
            format!(
                "{}   {} ONLINE   {} ROOMS   {}",
                scope,
                info.as_ref().map_or(0, |i| i.online),
                info.as_ref().map_or(0, |i| i.rooms),
                ping_text(self.server_ping()),
            )
        };
        let motd = self
            .servers
            .iter()
            .find(|r| r.addr == self.client.server)
            .and_then(|r| r.info.as_ref())
            .map(|i| i.motd.clone())
            .unwrap_or_default();
        let port = self.config.net.port;
        let f = &self.form;
        let data = json!({
            "menu": menu,
            "is_menu": matches!(self.state, State::Menu | State::Servers | State::RemoveServer | State::Profile | State::Account),
            "is_profile": self.state == State::Profile,
            "is_account": matches!(self.state, State::Account | State::AccountForm),
            "is_acct_form": self.state == State::AccountForm,
            "acct": acct,
            "accts": accts,
            "is_servers": matches!(self.state, State::Servers | State::RemoveServer),
            "is_add": self.state == State::AddServer,
            "is_lobby": self.state == State::Lobby,
            "is_create": self.state == State::Create,
            "is_room": self.state == State::Room,
            "is_join": self.state == State::Join,
            "is_queue": self.state == State::Queue,
            "is_board": self.state == State::Board,
            "is_direct": self.state == State::Direct,
            "is_matchup": self.state == State::Matchup,
            "not_matchup": self.state != State::Matchup,
            "mx": mx,
            "has_server": has_server && !matches!(self.state, State::Servers | State::RemoveServer | State::Matchup | State::Profile | State::Account | State::AccountForm),
            "no_server": matches!(self.state, State::Servers | State::RemoveServer),
            "scope": scope,
            "ranked_official": ranked_official,
            "ranked_community": !ranked_official,
            "last_result": self.last_result_line(),
            "has_last_result": !self.last_result_line().is_empty() && !matches!(self.state, State::Matchup) && !accounts_page,
            "title": if accounts_page { "PROFILE" } else { "ONLINE" },
            "server_line": server_line,
            "motd": motd,
            "lobby": lobby,
            "lobby_empty": self.rooms.is_empty(),
            "create": {
                "name": f.name,
                "password": f.password,
                "public": if f.public { "PUBLIC (LISTED)" } else { "PRIVATE (CODE ONLY)" },
                "rounds": format!("ROUNDS: {}", f.rules.rounds),
                "time": if f.rules.round_time == 0 { "TIMER: OFF".to_string() } else { format!("TIMER: {} S", f.rules.round_time) },
                "mode": format!("MODE: {}", f.rules.mode.to_uppercase()),
                "chars": if f.rules.chars == "mirror" { "FIGHTERS: MIRROR" } else { "FIGHTERS: FREE PICK" },
                "spect": format!("SPECTATORS: {}", f.spectators),
            },
            "direct": {
                "title": if self.direct_side == 0 { "HOST  -  YOU PLAY 1P" } else { "JOIN  -  YOU PLAY 2P" },
                "addr": self.addr,
                "mine": format!("{}:{port}", self.local_ip),
                "fighters": format!(
                    "1P {}  VS  2P {}   STAGE {}",
                    self.config.net.p1.to_uppercase(),
                    self.config.net.p2.to_uppercase(),
                    if self.config.net.stage.trim().is_empty() { "AUTO".to_string() } else { self.config.net.stage.to_uppercase() },
                ),
            },
            "status": self.status,
            "error": self.error,
            "has_error": !self.error.is_empty(),
            "server": {
                "name": self.server_name(),
                "ping": ping_text(self.server_ping()),
            },
            "me": {
                "scope": if ranked_official {
                    "OFFICIAL RANKING · GLOBAL".to_string()
                } else {
                    format!("COMMUNITY RANKING · {}", self.server_name().to_uppercase().chars().take(14).collect::<String>())
                },
                "name": badge_name(&me),
                "rating": me.rating,
                "best": me.best_rating,
                "rank": me.rank,
                "wins": me.wins,
                "losses": me.losses,
                "char": self.my_char().to_uppercase(),
            },
            "room": {
                "code": room.map(|r| r.code.clone()).unwrap_or_default(),
                "name": room.map(|r| r.name.to_uppercase()).unwrap_or_default(),
                "rules": room.map(|r| rules_line(&r.rules)).unwrap_or_default(),
                "ready": room.is_some_and(|r| r.ready),
                "waiting": room.is_some_and(|r| r.opponent.is_none() && r.side <= 2),
                "rematch": room.is_some_and(|r| r.opponent.is_some() && r.side <= 2),
                "members": members,
                "role": role_name(my_role),
            },
            "typed": format!("{:<5}", self.typed).replace(' ', "_"),
            "password": self.password,
            "opponent": {
                "scope": scope_short,
                "name": badge_name(&opp),
                "rating": opp.rating,
                "rank": opp.rank,
                "record": format!("{}W {}L", opp.wins, opp.losses),
                "ping": ping_text(opp_ping),
            },
            "queue": {
                "title": if self.ranked { format!("RANKED · {scope_short}") } else { "QUICK MATCH".to_string() },
                "secs": self.queue.as_ref().map_or(0, |q| q.waiting_secs),
                "count": self.queue.as_ref().map_or(0, |q| q.players_in_queue),
                "found": self.queue.as_ref().is_some_and(|q| q.found.is_some()),
            },
            "board": board_rows,
            "season": self.board.as_ref().map(|b| format!("{} · {}", scope, b.season.name.to_uppercase())).unwrap_or_default(),
            "my_rank": my_rank,
            "profile": serde_json::to_value(&self.profile).map(|mut v| {
                // never hand account keys to the UI layer
                if let Some(o) = v.as_object_mut() {
                    o.remove("online_key");
                    o.remove("online_accounts");
                }
                v
            }).unwrap_or(Value::Null),
        });
        // CONFIRM appearing takes the focus (it is what the player wants next)
        let can_confirm = data["mx"]["can_confirm"].as_bool() == Some(true);
        if can_confirm && !self.confirm_focused {
            self.ui.set_focus(Some("mx_confirm"));
        }
        self.confirm_focused = can_confirm;
        self.ui.set_data(data);
    }

    /// Text inputs take raw keys while they are being edited.
    pub fn capturing(&self) -> bool {
        self.ui.text_focused()
    }

    pub fn raw_key(&mut self, key: &Key) -> bool {
        // up / down leave the text box and move on (the text stays)
        if matches!(
            key,
            Key::Named(winit::keyboard::NamedKey::ArrowUp | winit::keyboard::NamedKey::ArrowDown)
        ) && self.ui.text_focused()
        {
            self.ui.stop_edit();
            return false;
        }
        let Some(k) = crate::uiscreen::text_key(key) else {
            return self.ui.text_focused();
        };
        if k == rugen_ui::TextKey::Escape
            && matches!(
                self.state,
                State::Join | State::Direct | State::AddServer | State::Create | State::AccountForm
            )
        {
            self.ui.text_key(k);
            self.cancel();
            self.refresh();
            return true;
        }
        self.ui.text_key(k)
    }

    pub fn mouse(&mut self, ev: MouseEvent) {
        match ev {
            MouseEvent::Move(x, y) => self.mouse_px = Some((x, y)),
            MouseEvent::Left => {
                self.mouse_px = None;
                self.ui.mouse_move(None);
            }
            MouseEvent::Button(down) => self.ui.mouse_button(down),
            MouseEvent::Wheel(lines) => self.ui.mouse_wheel(lines),
        }
    }

    fn join(&mut self, code: String, spectate: bool) {
        let (c, ch, pw) = (self.client.clone(), self.my_char(), self.password.clone());
        self.status = format!("JOINING {code}");
        self.error.clear();
        self.room = None;
        self.jobs.push(Job::Room(Pending::spawn(move || {
            c.join_room(&code, &ch, &pw, spectate)
        })));
        self.state = State::Room;
    }

    fn enter_queue(&mut self, ranked: bool) {
        let (c, ch) = (self.client.clone(), self.my_char());
        let max = self
            .config
            .net
            .good_connections_only
            .then_some(GOOD_PING_MS);
        let rtt = self.server_ping();
        self.ranked = ranked;
        self.queue = None;
        self.error.clear();
        self.status = "ENTERING QUEUE".into();
        self.state = State::Queue;
        self.jobs.push(Job::Queue(Pending::spawn(move || {
            c.enter_queue(&ch, ranked, max, rtt)
        })));
    }

    fn save_config(&self) {
        let mut c = crate::config::Config::load(&self.root);
        c.net.servers = self.config.net.servers.clone();
        c.net.server = self.config.net.server.clone();
        if let Err(e) = c.save(&self.root) {
            log::warn!("could not save rugen.toml: {e}");
        }
    }

    fn action(&mut self, a: &str) -> Option<Transition> {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match (head, arg) {
            ("back", _) | ("title", _) => {
                if self.leaves_screen() {
                    return Some(Transition::Title);
                }
                self.cancel();
            }
            ("online", "profile") => self.open_profile(),
            ("acct", rest) => {
                let (what, arg) = rest.split_once(':').unwrap_or((rest, ""));
                match what {
                    "open" => {
                        if let Ok(i) = arg.parse::<usize>() {
                            self.open_account(i);
                        }
                    }
                    "create" => self.open_form(AcctForm::Create),
                    "login" => self.open_form(AcctForm::Login),
                    "claim" => self.open_form(AcctForm::Claim),
                    "password" => self.open_form(AcctForm::Password),
                    "logout" => self.logout(false),
                    "logout_all" => self.logout(true),
                    "show" => self.acct_show = !self.acct_show,
                    "submit" => self.submit_form(),
                    "next" => {
                        // ENTER in a field: on to the next one, or send
                        let next = match (self.acct_form, arg) {
                            (AcctForm::Password, "pass") => Some("acct_new"),
                            (AcctForm::Password, _) | (_, "pass") => None,
                            _ => Some("acct_pass"),
                        };
                        match next {
                            Some(id) => self.ui.edit(id),
                            None => self.submit_form(),
                        }
                    }
                    "enter" => {
                        self.connect_after = false;
                        let addr = self.acct_addr();
                        self.connect(&addr);
                    }
                    _ => {}
                }
            }
            ("acct_user", v) => {
                self.acct_user = v
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                    .take(rugen_proto::account::USERNAME_MAX)
                    .collect();
                self.ui.set_input("acct_user", &self.acct_user);
            }
            ("acct_pass", v) => {
                self.acct_pass = v.chars().take(rugen_proto::account::PASSWORD_MAX).collect();
            }
            ("acct_new", v) => {
                self.acct_new = v.chars().take(rugen_proto::account::PASSWORD_MAX).collect();
            }
            ("server", i) => {
                if let Some(row) = i.parse::<usize>().ok().and_then(|i| self.servers.get(i)) {
                    let addr = row.addr.clone();
                    self.connect(&addr);
                }
            }
            ("online", "servers") => {
                self.presence = None;
                self.client = OnlineClient::new("", None);
                self.state = State::Servers;
                self.status = "PICK A SERVER".into();
                self.probe_servers();
            }
            ("online", "remove") => {
                self.state = State::RemoveServer;
                self.status = "PICK THE COMMUNITY SERVER TO REMOVE (THE OFFICIAL ONE STAYS)".into();
            }
            ("server_remove", i) => {
                if let Some(row) = i.parse::<usize>().ok().and_then(|i| self.servers.get(i))
                    && !row.official
                {
                    let addr = row.addr.clone();
                    self.config.net.remove_server(&addr, official::server());
                    self.servers.retain(|r| r.addr != addr);
                    self.save_config();
                    self.status = format!("REMOVED {}", addr.to_uppercase());
                }
                self.state = State::Servers;
                self.probe_servers();
            }
            ("online", "add") => {
                self.addr.clear();
                self.error.clear();
                self.status = "TYPE A SERVER: HTTPS://HOST OR IP:PORT".into();
                self.state = State::AddServer;
                self.ui.set_input("server_addr", "");
                self.refresh();
                self.ui.edit("server_addr");
            }
            ("server_addr", v) => {
                self.addr = v
                    .chars()
                    .filter(|c| c.is_ascii_graphic())
                    .take(80)
                    .collect();
                self.ui.set_input("server_addr", &self.addr);
            }
            ("server_add", _) => {
                let addr = self.addr.trim().to_string();
                if addr.is_empty() {
                    self.status = "TYPE AN ADDRESS".into();
                    self.ui.edit("server_addr");
                    return None;
                }
                if official::is_official_addr(&addr) {
                    self.state = State::Servers;
                    self.status = "THE OFFICIAL SERVER IS ALWAYS LISTED".into();
                    return None;
                }
                if !self
                    .servers
                    .iter()
                    .any(|r| official::same_server(&r.addr, &addr))
                {
                    self.servers.push(ServerRow {
                        addr: addr.clone(),
                        official: false,
                        ..Default::default()
                    });
                    self.config.net.servers.push(addr.clone());
                    self.save_config();
                }
                self.state = State::Servers;
                self.status = format!("SAVED {}", addr.to_uppercase());
                self.probe_servers();
            }
            ("online", "lobby") => {
                self.state = State::Lobby;
                self.status = "LOADING ROOMS".into();
                let c = self.client.clone();
                self.jobs
                    .push(Job::Rooms(Pending::spawn(move || c.rooms())));
            }
            ("lobby", rest) => {
                let (kind, code) = rest.split_once(':').unwrap_or(("join", rest));
                // a host of another version: say so instead of trying
                if let Some(e) = self
                    .rooms
                    .iter()
                    .find(|r| r.code == code)
                    .and_then(room_error)
                {
                    self.error = e;
                    return None;
                }
                let locked = self.rooms.iter().any(|r| r.code == code && r.locked);
                if locked && self.password.is_empty() {
                    // ask for the password first, then join
                    self.typed = code.to_string();
                    self.state = State::Join;
                    self.status = "THIS ROOM HAS A PASSWORD".into();
                    self.ui.set_input("code", &self.typed);
                    self.ui.set_input("password", "");
                    self.refresh();
                    self.ui.edit("password");
                    return None;
                }
                self.join(code.to_string(), kind == "watch");
            }
            ("online", "create") => {
                self.form = Form {
                    name: format!("{}'S ROOM", self.profile.name.to_uppercase()),
                    ..Form::default()
                };
                self.state = State::Create;
                self.status = "SET UP YOUR ROOM".into();
                self.ui.set_input("room_name", &self.form.name);
                self.ui.set_input("room_password", "");
            }
            ("room_name", v) => {
                self.form.name = v.chars().take(24).collect();
                self.ui.set_input("room_name", &self.form.name);
            }
            ("room_password", v) => {
                self.form.password = v.chars().take(32).collect();
                self.ui.set_input("room_password", &self.form.password);
            }
            ("form", what) => {
                let f = &mut self.form;
                match what {
                    "public" => f.public = !f.public,
                    "rounds" => f.rules.rounds = f.rules.rounds % 3 + 1,
                    "time" => {
                        f.rules.round_time = match f.rules.round_time {
                            99 => 60,
                            60 => 0,
                            _ => 99,
                        }
                    }
                    "mode" => {
                        f.rules.mode = if f.rules.mode == "arena" {
                            "classic".into()
                        } else {
                            "arena".into()
                        }
                    }
                    "chars" => {
                        f.rules.chars = if f.rules.chars == "mirror" {
                            "free".into()
                        } else {
                            "mirror".into()
                        }
                    }
                    "spect" => f.spectators = (f.spectators + 1) % 5,
                    "go" => {
                        let opts = RoomOptions {
                            name: f.name.clone(),
                            public: f.public,
                            password: f.password.clone(),
                            rules: Rules {
                                stage: self.config.net.stage.clone(),
                                ..f.rules.clone()
                            },
                            max_spectators: f.spectators,
                        };
                        let (c, ch) = (self.client.clone(), self.my_char());
                        self.room = None;
                        self.used_ticket = None;
                        self.status = "CREATING ROOM".into();
                        self.state = State::Room;
                        self.jobs
                            .push(Job::Room(Pending::spawn(move || c.create_room(&ch, &opts))));
                    }
                    _ => {}
                }
            }
            ("typed", v) => {
                self.typed = v
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .map(|c| c.to_ascii_uppercase())
                    .take(5)
                    .collect();
                self.ui.set_input("code", &self.typed);
            }
            ("pw", v) => {
                self.password = v.chars().take(32).collect();
                self.ui.set_input("password", &self.password);
            }
            ("join", _) => {
                if self.typed.len() == 5 {
                    self.join(self.typed.clone(), false);
                } else {
                    self.status = "THE CODE HAS 5 LETTERS".into();
                    self.ui.edit("code");
                }
            }
            ("online", "join") => {
                self.typed.clear();
                self.password.clear();
                self.error.clear();
                self.status = "TYPE THE 5-LETTER ROOM CODE".into();
                self.state = State::Join;
                self.ui.set_input("code", "");
                self.ui.set_input("password", "");
                self.refresh();
                self.ui.edit("code");
                // `[net] room` in rugen.toml pre-fills the code
                let preset = self.config.net.room.trim().to_ascii_uppercase();
                if preset.len() == 5 {
                    self.typed = preset;
                    self.ui.set_input("code", &self.typed);
                }
            }
            ("room", rest) => {
                let code = self.room.as_ref().map(|r| r.code.clone())?;
                let c = self.client.clone();
                if let Some(id) = rest
                    .strip_prefix("kick:")
                    .and_then(|i| i.parse::<u64>().ok())
                {
                    self.jobs
                        .push(Job::Room(Pending::spawn(move || c.kick(&code, id))));
                } else if rest == "rematch" {
                    self.status = "WAITING FOR YOUR OPPONENT TO ACCEPT".into();
                    self.jobs
                        .push(Job::Room(Pending::spawn(move || c.rematch(&code))));
                }
            }
            ("online", "quick") => self.enter_queue(false),
            ("online", "ranked") => {
                // rankings belong to named accounts (servers that have them)
                let named = self.me.as_ref().is_none_or(|m| m.verified);
                let server = self.client.server.clone();
                if !named && self.auto.is_none() && self.has_accounts(&server) {
                    if let Some(i) = self.servers.iter().position(|r| r.addr == server) {
                        self.open_account(i);
                        self.check_accounts();
                        self.status = "RANKED NEEDS A USERNAME: CLAIM THIS ACCOUNT".into();
                    }
                    return None;
                }
                self.enter_queue(true);
            }
            ("online", "board") => {
                let c = self.client.clone();
                self.board = None;
                self.error.clear();
                self.status = "LOADING".into();
                self.state = State::Board;
                self.jobs
                    .push(Job::Board(Pending::spawn(move || c.leaderboard())));
            }
            ("online", "cancel") => self.cancel(),
            ("online", "host") | ("online", "direct") => {
                self.direct_side = usize::from(arg == "direct");
                self.error.clear();
                self.status = "TYPE THE OTHER PLAYER'S ADDRESS AND PRESS ENTER".into();
                self.state = State::Direct;
                self.ui.set_input("addr", &self.addr);
                self.refresh();
                self.ui.edit("addr");
            }
            ("addr", v) => {
                self.addr = v
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-'))
                    .take(40)
                    .collect();
                self.ui.set_input("addr", &self.addr);
            }
            ("direct", "go") => self.direct_go(),
            ("mx", "confirm") => {
                if let Some(mx) = &self.mx {
                    mx.driver.accept();
                }
            }
            ("mx", "change") => {
                if self.mx.is_some() {
                    self.want_select = true;
                }
            }
            ("mx", "leave") => self.leave_matchup("YOU LEFT THE MATCHUP"),
            ("netplay", _) => return Some(Transition::Netplay),
            _ => log::warn!("online: unknown action {a:?}"),
        }
        None
    }

    /// Direct match: resolve the typed address and both `[net]` fighters, then fight.
    fn direct_go(&mut self) {
        use std::net::ToSocketAddrs as _;
        let typed = self.addr.trim();
        if typed.is_empty() {
            self.status = "TYPE AN ADDRESS, E.G. 192.168.1.20".into();
            self.ui.edit("addr");
            return;
        }
        let with_port = if typed.contains(':') {
            typed.to_string()
        } else {
            format!("{typed}:{}", self.config.net.port)
        };
        let remote = match with_port.to_socket_addrs().map(|mut a| a.next()) {
            Ok(Some(a)) => a,
            Ok(None) | Err(_) => {
                self.status = format!("CANNOT REACH {}", with_port.to_uppercase());
                self.ui.edit("addr");
                return;
            }
        };
        let resolve = |lib: &Library, id: &str| -> Option<PathBuf> {
            let e = lib.find_char(id)?;
            lib.prepare_char(&e).ok().map(|(d, man)| man.def_path(&d))
        };
        let n = self.config.net.clone();
        let (Some(p1), Some(p2)) = (resolve(&self.lib, &n.p1), resolve(&self.lib, &n.p2)) else {
            self.fail(
                "cannot start",
                format!("[net] p1 {:?} / p2 {:?} must be installed", n.p1, n.p2),
            );
            return;
        };
        let stage = self
            .lib
            .stages()
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(&n.stage))
            .and_then(|e| self.lib.prepare_stage(&e).ok().map(|(d, m)| m.def_path(&d)))
            .or_else(|| self.lib.default_stage());
        log::info!("online: direct match as side {}", self.direct_side + 1);
        self.pending_fight = Some(FightSetup {
            p1,
            p2,
            stage,
            ai: [0, 0],
            net: Some(NetMode::P2P {
                port: n.port,
                remote,
                side: self.direct_side,
            }),
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
        });
    }

    /// BACK leaves the ONLINE screen (to the home menu) from here.
    fn leaves_screen(&self) -> bool {
        self.state == State::Servers || (self.state == State::Profile && self.profile_from_title)
    }

    fn cancel(&mut self) {
        if self.state == State::Matchup {
            self.leave_matchup("YOU LEFT THE MATCHUP");
            return;
        }
        match self.state {
            State::AccountForm => {
                let i = self.acct_server;
                let after = self.connect_after;
                self.open_account(i);
                self.connect_after = after;
                return;
            }
            State::Account if !self.connect_after => {
                self.open_profile();
                return;
            }
            State::Account | State::Profile => {
                self.clear_form();
                self.connect_after = false;
                self.state = State::Servers;
                self.status = "PICK A SERVER".into();
                self.probe_servers();
                return;
            }
            _ => {}
        }
        match self.state {
            State::Queue => {
                let c = self.client.clone();
                self.jobs
                    .push(Job::Done(Pending::spawn(move || c.leave_queue())));
            }
            State::Room => {
                if let Some(code) = self.room.as_ref().map(|r| r.code.clone()) {
                    let c = self.client.clone();
                    self.jobs
                        .push(Job::Done(Pending::spawn(move || c.leave_room(&code))));
                }
            }
            _ => {}
        }
        self.room = None;
        self.queue = None;
        self.typed.clear();
        self.password.clear();
        self.status.clear();
        self.state = if matches!(
            self.state,
            State::Servers | State::AddServer | State::RemoveServer | State::Direct
        ) || self.client.server.is_empty()
        {
            State::Servers
        } else {
            State::Menu
        };
    }

    fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        log::warn!("online: {what}: {e}");
        if let Some(mx) = self.mx.take() {
            mx.driver.leave();
        }
        self.auto_exit(&format!("{what}: {e}"));
        self.error = format!("{what}: {e}").to_uppercase();
        self.status.clear();
        if !matches!(
            self.state,
            State::Board | State::Servers | State::RemoveServer | State::Lobby
        ) {
            self.state = if self.client.server.is_empty() {
                State::Servers
            } else {
                State::Menu
            };
        }
    }

    fn room_update(&mut self, r: RoomInfo) {
        let host = r
            .members
            .iter()
            .find(|m| m.role == 0)
            .map_or(0, |m| m.profile.id);
        let chal = r
            .members
            .iter()
            .find(|m| m.role == 1)
            .map_or(0, |m| m.profile.id);
        // a new match in this room: its matchup phase (pick, verdict, sync)
        if r.side <= 2
            && let Some(mv) = &r.matchup
            && mv.phase != "failed"
            && mv.phase != "started"
            && !started_before(mv.id)
            && self.mx.as_ref().is_none_or(|m| m.driver.id != mv.id)
            && !self.error.starts_with("MATCHUP")
        {
            log::info!("online: room {} match {} - matchup", r.code, mv.id);
            let (id, code) = (mv.id, r.code.clone());
            self.room = Some(r);
            self.start_matchup(id, [host, chal], false, Some(code));
            return;
        }
        // watching: the match runs, fetch its fighters and join the relay
        if r.side > 2 && r.ready && r.ticket.is_some() && !started_before(r.match_id) {
            self.start_watching(&r);
            self.room = Some(r);
            return;
        }
        if self.state == State::Matchup {
            self.room = Some(r);
            return;
        }
        if self.room.as_ref().is_none_or(|old| old.code != r.code) {
            log::info!("online: room code {}", r.code);
            if self
                .auto
                .as_ref()
                .is_some_and(|a| a.room.eq_ignore_ascii_case("new"))
            {
                println!("ROOM CODE: {}", r.code);
            }
        }
        if self.error.is_empty() || !self.error.starts_with("MATCHUP") {
            self.status = if r.opponent.is_none() && r.side <= 2 {
                format!("ROOM {} - WAITING FOR A CHALLENGER", r.code)
            } else if r.side > 2 {
                format!("ROOM {} - WATCHING, WAITING FOR THE NEXT MATCH", r.code)
            } else {
                format!("ROOM {} - REMATCH?", r.code)
            };
        }
        self.room = Some(r);
    }

    fn match_found(&mut self, mi: MatchInfo) {
        let my_id = self.my_id();
        let players = if mi.side == 1 {
            [my_id, mi.opponent.id]
        } else {
            [mi.opponent.id, my_id]
        };
        let Some(mv) = mi.matchup.clone() else {
            self.fail(
                "match",
                "this server is too old for bring-your-own-character",
            );
            return;
        };
        if self.mx.as_ref().is_some_and(|m| m.driver.id == mv.id) || started_before(mv.id) {
            return;
        }
        self.start_matchup(mv.id, players, mi.ranked, None);
        self.status = format!(
            "MATCH FOUND: {} ({})  {}",
            mi.opponent.name,
            mi.opponent.rank,
            ping_text(mi.est_ping_ms)
        );
    }

    fn poll_presence(&mut self) {
        let Some(p) = self.presence.as_mut() else {
            return;
        };
        for m in p.drain() {
            match m {
                ServerMsg::Welcome { id, .. } => log::debug!("online: presence as {id}"),
                ServerMsg::Room { room } => {
                    if matches!(self.state, State::Room | State::Matchup)
                        && self.room.as_ref().is_none_or(|r| r.code == room.code)
                    {
                        self.room_update(room);
                    }
                }
                ServerMsg::Found { found } => {
                    if self.state == State::Queue {
                        self.match_found(found);
                    }
                }
                ServerMsg::Kicked { code } => {
                    if self.room.as_ref().is_some_and(|r| r.code == code) {
                        self.room = None;
                        self.fail("room", format!("{code} closed or you were removed"));
                    }
                }
                ServerMsg::Pong | ServerMsg::Error { .. } | ServerMsg::Matchup { .. } => {}
            }
        }
    }

    fn poll_jobs(&mut self) {
        let mut jobs = std::mem::take(&mut self.jobs);
        let mut keep = Vec::new();
        let mut outdated = None;
        for mut j in jobs.drain(..) {
            match &mut j {
                Job::Register(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        if !self.auto.as_ref().is_some_and(|a| a.guest) {
                            self.profile
                                .online_accounts
                                .insert(self.client.server.clone(), (r.id, r.key.clone()));
                            self.profile.save(&self.root);
                        }
                        self.client.key = Some(r.key);
                        self.presence = Presence::spawn(&self.client).ok();
                        let (c, name) = (self.client.clone(), self.profile.name.clone());
                        self.jobs
                            .push(Job::Me(Pending::spawn(move || c.set_profile(&name, 0))));
                    }
                    Some(Err(e)) => self.fail("register", e),
                },
                Job::Account(addr, p) => match p.take() {
                    None => keep.push(j),
                    Some(r) => {
                        let addr = addr.clone();
                        self.account_status(&addr, r);
                    }
                },
                Job::Session(addr, kind, p) => match p.take() {
                    None => keep.push(j),
                    Some(r) => {
                        let (addr, kind) = (addr.clone(), *kind);
                        self.session_done(&addr, kind, r);
                    }
                },
                Job::Logout(addr, everywhere, p) => match p.take() {
                    None => keep.push(j),
                    Some(r) => {
                        let (addr, everywhere) = (addr.clone(), *everywhere);
                        match r {
                            Ok(()) | Err(rugen_net::online::OnlineError::Server(_)) => {
                                forget_session(&mut self.profile, &self.config, &addr);
                                self.profile.save(&self.root);
                                self.accounts.remove(&addr);
                                self.account_changed(&addr, "");
                                self.ui.set_focus(None);
                                self.status = if everywhere {
                                    "LOGGED OUT ON EVERY DEVICE".into()
                                } else {
                                    "LOGGED OUT".into()
                                };
                            }
                            Err(e) => {
                                log::warn!("online: logout: {e}");
                                self.status.clear();
                                self.acct_error = "CANNOT REACH THE SERVER".into();
                            }
                        }
                    }
                },
                Job::Info(i, p) => match p.take() {
                    None => keep.push(j),
                    Some(r) => {
                        let i = *i;
                        if let Some(row) = self.servers.get_mut(i) {
                            match r {
                                Ok((info, ping, id)) => {
                                    let claims = row.official || info.official;
                                    row.trust =
                                        official::identity_trust(id.as_ref(), &row.nonce, claims);
                                    if row.trust == Trust::Impostor {
                                        log::warn!(
                                            "online: {} could not prove it is the official server",
                                            row.addr
                                        );
                                    }
                                    // connected before the probe answered (resume, auto,
                                    // a quick pick): leave a server too old for .rgc
                                    if row.addr == self.client.server
                                        && let Some(e) = outdated_error(&row.addr, &info)
                                    {
                                        outdated = Some(e);
                                    }
                                    row.info = Some(info);
                                    row.ping = Some(ping);
                                    row.error = false;
                                }
                                Err(e) => {
                                    log::info!("online: {}: {e}", row.addr);
                                    row.error = true;
                                }
                            }
                        }
                    }
                },
                Job::Truth(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(blob)) => match official::store_feed(&self.lib.cache_dir(), &blob) {
                        Ok(f) => log::info!(
                            "online: official feed #{}: {} blocked, {} rated",
                            f.sequence,
                            f.blocked.len(),
                            f.ratings.len()
                        ),
                        Err(e) => log::warn!("online: official feed rejected: {e}"),
                    },
                    Some(Err(e)) => log::info!("online: official feed: {e}"),
                },
                Job::Result(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(ack)) => {
                        if let Ok(mut l) = LAST_RESULT.lock()
                            && let Some(r) = l.as_mut()
                        {
                            r.ack = Some(ack);
                        }
                    }
                    Some(Err(e)) => log::info!("online: last result: {e}"),
                },
                Job::Me(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(me)) => {
                        self.remember_rank(&me);
                        if me.name != self.profile.name && self.me.is_none() && !me.verified {
                            // first contact: push the local name to the server
                            let (c, name) = (self.client.clone(), self.profile.name.clone());
                            self.jobs
                                .push(Job::Me(Pending::spawn(move || c.set_profile(&name, 0))));
                        }
                        if self.status.starts_with("CONNECTING") {
                            self.status = "CONNECTED".into();
                        }
                        self.me = Some(me);
                    }
                    Some(Err(e)) => {
                        log::warn!("online: profile: {e}");
                        if self.status.starts_with("CONNECTING") {
                            self.status = "CONNECTED".into();
                        }
                    }
                },
                Job::Rooms(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        self.status = if r.is_empty() {
                            "NO PUBLIC ROOMS - CREATE ONE!".into()
                        } else {
                            format!("{} PUBLIC ROOMS", r.len())
                        };
                        self.rooms = r;
                    }
                    Some(Err(e)) => self.fail("lobby", e),
                },
                Job::Room(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        if matches!(self.state, State::Room | State::Matchup) {
                            self.room_update(r);
                        }
                    }
                    Some(Err(e)) => self.fail("room", e),
                },
                Job::Queue(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(q)) => {
                        if let Some(m) = q.found.clone() {
                            self.queue = Some(q);
                            if self.state == State::Queue {
                                self.match_found(m);
                            }
                        } else if self.state == State::Queue {
                            self.status = if q.queued {
                                format!("SEARCHING - {} IN QUEUE", q.players_in_queue)
                            } else {
                                "NOT QUEUED".into()
                            };
                            self.queue = Some(q);
                        }
                    }
                    Some(Err(e)) => self.fail("queue", e),
                },
                Job::Board(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(b)) => {
                        self.status.clear();
                        self.board = Some(b);
                    }
                    Some(Err(e)) => self.fail("leaderboard", e),
                },
                Job::Done(p) => {
                    if p.take().is_none() {
                        keep.push(j);
                    }
                }
            }
        }
        keep.append(&mut self.jobs);
        self.jobs = keep;
        if let Some(e) = outdated {
            self.presence = None;
            self.client = OnlineClient::new("", None);
            self.state = State::Servers;
            self.error = e;
        }
    }

    /// `GET /v1/account` answered for server `addr`.
    fn account_status(&mut self, addr: &str, r: OnlineResult<AccountInfo>) {
        use rugen_net::online::OnlineError;
        let mut forget = false;
        {
            let view = self.accounts.entry(addr.to_string()).or_default();
            view.checking = false;
            match r {
                Ok(info) => {
                    view.error.clear();
                    view.unsupported = false;
                    view.info = Some(info);
                }
                Err(OnlineError::Server(e)) if e == "bad key" => {
                    // revoked (password changed / logged out elsewhere)
                    view.info = None;
                    view.error = "LOGGED OUT: THE ACCOUNT WAS LOGGED OUT ON THIS DEVICE".into();
                    forget = true;
                }
                Err(OnlineError::Server(e)) => {
                    // older server: no account routes
                    log::info!("online: {addr}: accounts: {e}");
                    view.unsupported = true;
                    view.error.clear();
                }
                Err(e) => {
                    log::info!("online: {addr}: accounts: {e}");
                    view.error = "SERVER OFFLINE".into();
                }
            }
        }
        let named = self
            .accounts
            .get(addr)
            .and_then(|v| v.info.as_ref())
            .map(|i| i.username.clone());
        if forget {
            forget_session(&mut self.profile, &self.config, addr);
            self.profile.save(&self.root);
            self.account_changed(addr, "");
        } else if let Some(name) = named
            && self.profile.online_names.get(addr).map(String::as_str) != Some(name.as_str())
        {
            if name.is_empty() {
                self.profile.online_names.remove(addr);
            } else {
                self.profile.online_names.insert(addr.to_string(), name);
            }
            self.profile.save(&self.root);
        }
    }

    /// The account form came back from server `addr`.
    fn session_done(&mut self, addr: &str, kind: AcctForm, r: OnlineResult<Session>) {
        use rugen_net::online::OnlineError;
        match r {
            Ok(s) => {
                remember_session(&mut self.profile, addr, &s);
                self.profile.save(&self.root);
                let view = self.accounts.entry(addr.to_string()).or_default();
                view.unsupported = false;
                view.error.clear();
                let sessions = view.info.as_ref().map_or(1, |i| i.sessions);
                view.info = Some(AccountInfo {
                    id: s.id,
                    username: s.username.clone(),
                    sessions: if kind == AcctForm::Password {
                        1
                    } else {
                        sessions
                    },
                    profile: s.profile.clone(),
                });
                self.account_changed(addr, &s.key);
                let after = self.connect_after;
                if let Some(i) = self.servers.iter().position(|r| r.addr == addr) {
                    self.open_account(i);
                }
                self.check_accounts();
                let name = s.username.to_uppercase();
                self.status = match kind {
                    AcctForm::Create => format!("WELCOME, {name}! ACCOUNT CREATED"),
                    AcctForm::Login => format!("LOGGED IN AS {name}"),
                    AcctForm::Claim => format!("ACCOUNT CLAIMED: YOU ARE {name}"),
                    AcctForm::Password => "PASSWORD CHANGED: OTHER DEVICES WERE LOGGED OUT".into(),
                };
                // the server asked for an account: straight in
                if after && kind != AcctForm::Password {
                    let status = self.status.clone();
                    self.connect(addr);
                    self.status = status;
                }
            }
            Err(OnlineError::Server(e)) => {
                self.status = kind.title().into();
                self.acct_error = if e == "no such route" {
                    // an older server: anonymous accounts only (entering it registers one)
                    self.accounts
                        .entry(addr.to_string())
                        .or_default()
                        .unsupported = true;
                    "THIS SERVER HAS NO USERNAMES YET: GO BACK AND ENTER IT".into()
                } else {
                    e.to_uppercase()
                };
            }
            Err(e) => {
                log::warn!("online: account: {e}");
                self.status = kind.title().into();
                self.acct_error = "CANNOT REACH THE SERVER".into();
            }
        }
        // typed passwords go as soon as they were sent
        self.acct_pass.clear();
        self.acct_new.clear();
        self.ui.set_input("acct_pass", "");
        self.ui.set_input("acct_new", "");
    }

    /// Keep the rating of this server in the profile (title screen rank card).
    fn remember_rank(&mut self, me: &ProfileInfo) {
        if self.auto.as_ref().is_some_and(|a| a.guest) {
            return;
        }
        let snap = crate::uiscreen::RankSnap {
            server: self.server_name(),
            official: self.trust_of(&self.client.server).is_official(),
            rating: me.rating,
            rank: me.rank.clone(),
            wins: me.wins,
            losses: me.losses,
        };
        if self.profile.online_ranks.get(&self.client.server) != Some(&snap) {
            self.profile
                .online_ranks
                .insert(self.client.server.clone(), snap);
            self.profile.save(&self.root);
        }
    }

    /// Periodic polling (rooms are also a heartbeat; pushes arrive via presence).
    fn poll_server(&mut self) {
        if self.client.server.is_empty() || self.tick % 60 != 0 {
            return;
        }
        let busy_room = self.jobs.iter().any(|j| matches!(j, Job::Room(_)));
        let busy_queue = self.jobs.iter().any(|j| matches!(j, Job::Queue(_)));
        match self.state {
            State::Room | State::Matchup if !busy_room => {
                if let Some(code) = self.room.as_ref().map(|r| r.code.clone()) {
                    let c = self.client.clone();
                    self.jobs
                        .push(Job::Room(Pending::spawn(move || c.poll_room(&code))));
                }
            }
            State::Queue if !busy_queue => {
                let c = self.client.clone();
                self.jobs
                    .push(Job::Queue(Pending::spawn(move || c.poll_queue())));
            }
            State::Menu if self.tick % 600 == 0 => self.poll_last_result(),
            State::Lobby if self.tick % 300 == 0 => {
                let c = self.client.clone();
                self.jobs
                    .push(Job::Rooms(Pending::spawn(move || c.rooms())));
            }
            _ => {}
        }
    }

    pub fn tick(
        &mut self,
        pressed: [Buttons; 2],
        sys: &mut SystemData,
        r: &mut Renderer,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        self.tick += 1;
        self.poll_jobs();
        self.poll_presence();
        self.poll_server();
        self.poll_matchup();
        self.auto_join();
        if self.auto_quit_at.is_some_and(|t| self.tick >= t) {
            return Some(Transition::Quit);
        }
        if let Some(f) = self.pending_fight.take() {
            return Some(Transition::Fight(f));
        }
        if let Some((setup, sides)) = self.pending_versus.take() {
            return Some(Transition::Versus(setup, sides));
        }
        if self.want_select {
            return Some(Transition::OnlineSelect);
        }
        self.window = r.size();
        let (vw, vh) = viewport(self.window);
        self.ui.set_size(vw, vh);
        if let Some((px, py)) = self.mouse_px {
            self.ui
                .mouse_move(Some(rugen_ui::window_to_virtual(self.window, px, py)));
        }
        let nav = if self.capturing() {
            Nav::default()
        } else {
            let any = |f: fn(&Buttons) -> bool| pressed.iter().any(f);
            Nav {
                up: any(|b| b.u),
                down: any(|b| b.d),
                left: any(|b| b.l),
                right: any(|b| b.r),
                accept: any(|b| b.a || b.c || b.x || b.y || b.z || b.s),
                back: any(|b| b.b),
            }
        };
        let events = {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
                mem: Some(&self.mem),
                rot: None,
            };
            self.ui.tick(nav, &mut host)
        };
        // list items are keyed `<list>[i].<id>`: move from row 0 to the default row
        if self.tick > 1
            && let Some(i) = self.pending_focus.take()
            && let Some(f) = self.ui.focused().filter(|f| f.contains("[0]."))
        {
            let key = f.replacen("[0].", &format!("[{i}]."), 1);
            self.ui.set_focus(Some(&key));
        }
        let mut out = None;
        for e in events {
            match e {
                Event::Moved => sfx(sys.title.move_snd),
                Event::Back => {
                    if self.leaves_screen() {
                        out = Some(Transition::Title);
                    } else {
                        self.cancel();
                    }
                }
                Event::Action(a) => {
                    let typing = [
                        "typed:",
                        "pw:",
                        "addr:",
                        "server_addr:",
                        "room_name:",
                        "room_password:",
                        "acct_user:",
                        "acct_pass:",
                        "acct_new:",
                    ];
                    if !typing.iter().any(|t| a.starts_with(t)) {
                        sfx(sys.title.done_snd);
                    }
                    if let Some(t) = self.action(&a) {
                        out = Some(t);
                    }
                }
                Event::Action2(_) | Event::Moved2 | Event::Input(..) => {}
            }
        }
        self.refresh();
        out
    }

    /// Esc from the shell: leave sub-states first, the screen second.
    pub fn back(&mut self) -> bool {
        if self.leaves_screen() {
            return true;
        }
        self.cancel();
        false
    }

    pub fn render(&mut self, r: &mut Renderer, sys: &mut SystemData) -> (Camera, [f64; 4]) {
        self.window = r.size();
        let (vw, vh) = viewport(self.window);
        self.ui.set_size(vw, vh);
        {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
                mem: Some(&self.mem),
                rot: None,
            };
            self.ui.draw(&mut host);
        }
        let bg = self.ui.background();
        (
            Camera {
                screen: [vw, vh],
                center: [vw / 2.0, vh / 2.0],
                zoom: 1.0,
            },
            [bg[0] as f64, bg[1] as f64, bg[2] as f64, 1.0],
        )
    }
}

/// A ping above this is flagged on the matchup screen.
const HIGH_PING_MS: u32 = 200;
/// Matchup stat panel (1280-wide layout, panel-relative): the centre line,
/// the half-width of the label column and the longest bar.
const STAT_CX: f32 = 600.0;
const STAT_GAP: f32 = 100.0;
const STAT_BAR: f32 = 300.0;

/// One mirrored stat row: values, bar lengths scaled to the larger value
/// (the left bar grows leftward from the label column), and which side is
/// higher (red), lower (ink) or equal (grey).
fn stat_row(a: Option<f64>, b: Option<f64>, fmt: &dyn Fn(f64) -> String) -> Value {
    let (Some(a), Some(b)) = (a, b) else {
        let txt = |v: Option<f64>| v.map_or_else(|| "-".to_string(), fmt);
        return json!({
            "l": txt(a), "r": txt(b), "lx": STAT_CX - STAT_GAP, "lw": 0, "rw": 0,
            "l_hi": false, "l_lo": false, "l_eq": false, "l_plain": true,
            "r_hi": false, "r_lo": false, "r_eq": false, "r_plain": true,
        });
    };
    let (a0, b0) = (a.max(0.0), b.max(0.0));
    let m = a0.max(b0);
    let len = |v: f64| {
        if m > 0.0 {
            (STAT_BAR as f64 * v / m).round() as f32
        } else {
            0.0
        }
    };
    let (lw, rw) = (len(a0), len(b0));
    let eq = (a - b).abs() < 1e-6;
    json!({
        "l": fmt(a), "r": fmt(b),
        "lx": STAT_CX - STAT_GAP - lw, "lw": lw, "rw": rw,
        "l_hi": a > b && !eq, "l_lo": a < b && !eq, "l_eq": eq, "l_plain": a <= b || eq,
        "r_hi": b > a && !eq, "r_lo": b < a && !eq, "r_eq": eq, "r_plain": b <= a || eq,
    })
}

/// The matchup's stat face-off, power first.
fn stat_rows(
    a: Option<&rugen_proto::FairnessProfile>,
    b: Option<&rugen_proto::FairnessProfile>,
) -> Value {
    let int = |v: f64| format!("{v:.0}");
    let approx = |v: f64| format!("~{v:.0}");
    let row = |f: &dyn Fn(&rugen_proto::FairnessProfile) -> f64, fmt: &dyn Fn(f64) -> String| {
        stat_row(a.map(f), b.map(f), fmt)
    };
    json!({
        "power": row(&|p| f64::from(p.power_score), &int),
        "life": row(&|p| f64::from(p.life), &int),
        "attack": row(&|p| f64::from(p.attack), &int),
        "defence": row(&|p| f64::from(p.defence), &int),
        "best_hit": row(&|p| f64::from(p.max_hit_damage), &int),
        "combo": row(&|p| f64::from(p.est_combo_damage), &approx),
        "supers": row(&|p| f64::from(p.supers), &int),
        "projectiles": row(&|p| f64::from(p.projectiles), &int),
        "throws": row(&|p| f64::from(p.throws), &int),
    })
}

/// Game settings both sides of an online match must share (determinism):
/// defaults, plus the room's round time and mode.
pub fn match_config(base: &Config, rules: &Rules) -> Config {
    let mut c = base.clone();
    c.game = crate::config::GameCfg {
        ai_level: base.game.ai_level,
        ..Default::default()
    };
    c.game.round_time = if rules.round_time == 0 {
        -1
    } else {
        rules.round_time as i32
    };
    if rules.mode == "arena" {
        c.game.air_jumps = 1;
        c.game.air_dash = true;
        c.game.fast_fall = true;
    }
    c
}

/// Report a finished online match on a worker thread (spectators report nothing).
pub fn report_result(
    root: &Path,
    config: &Config,
    m: &OnlineMatch,
    winner_side: Option<usize>,
    evidence: Evidence,
) -> Option<Pending<ResultAck>> {
    if m.spectator {
        return None;
    }
    let winner_side = winner_side?;
    let profile = Profile::load(root);
    let (_, key) = account_for(&profile, config, &m.server);
    let c = OnlineClient::new(&m.server, Some(key));
    let (id, winner) = (m.match_id, m.players[winner_side]);
    if let Ok(mut l) = LAST_RESULT.lock() {
        *l = Some(LastResult {
            server: m.server.clone(),
            match_id: id,
            me: m.players[m.my_side],
            ranked: m.ranked,
            won: winner_side == m.my_side,
            ending: evidence.ending,
            ack: None,
        });
    }
    log::info!(
        "online: reporting {} match {id}, winner {winner}, {} frames",
        if m.ranked { "ranked" } else { "unranked" },
        evidence.frames
    );
    Some(Pending::spawn(move || c.report(id, winner, &evidence)))
}

/// Apply an accepted result to the local profile.
pub fn apply_result(root: &Path, ack: &OnlineResult<ResultAck>, won: bool) {
    match ack {
        Ok(a) => {
            if let Ok(mut l) = LAST_RESULT.lock()
                && let Some(r) = l.as_mut()
            {
                if let Stamp::Forged(e) = official::result_stamp(a, r.match_id, r.me) {
                    log::warn!("online: result signature rejected: {e}");
                }
                r.ack = Some(a.clone());
            }
            if a.disputed {
                log::warn!("online: the two reports disagree; the result was not counted");
                return;
            }
            if let Some(d) = a.delta {
                log::info!("online: rating {} ({d:+})", a.rating.unwrap_or(0));
            }
            let mut p = Profile::load(root);
            if won {
                p.wins += 1;
            } else {
                p.losses += 1;
            }
            p.save(root);
        }
        Err(e) => log::warn!("online: result not accepted: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_forms_check_what_the_server_will() {
        use AcctForm::*;
        assert_eq!(form_error(Create, "axel", "simple", ""), None);
        assert_eq!(
            form_error(Create, "ab", "simple", ""),
            Some("USERNAME: 3 TO 16 LETTERS, DIGITS, _ OR -")
        );
        assert_eq!(
            form_error(Claim, "admin", "simple", ""),
            Some("THIS USERNAME IS RESERVED")
        );
        assert_eq!(
            form_error(Create, "axel", "12345", ""),
            Some("PASSWORD: 6 TO 128 CHARACTERS")
        );
        // logins only need something typed (the server answers generically)
        assert_eq!(form_error(Login, "x", "y", ""), None);
        assert_eq!(
            form_error(Login, "", "y", ""),
            Some("TYPE YOUR USERNAME AND PASSWORD")
        );
        assert_eq!(
            form_error(Password, "", "", "simple"),
            Some("TYPE YOUR CURRENT PASSWORD")
        );
        assert_eq!(
            form_error(Password, "", "old", "new"),
            Some("PASSWORD: 6 TO 128 CHARACTERS")
        );
        assert_eq!(form_error(Password, "", "old", "newer!"), None);
    }

    #[test]
    fn the_profile_keeps_keys_never_passwords() {
        let password = "correct-horse-battery";
        let mut p = Profile::default();
        let server = "https://rugen.example.com";
        let s = Session {
            id: 42,
            key: "k".repeat(64),
            username: "Axel".into(),
            profile: ProfileInfo {
                id: 42,
                name: "Axel".into(),
                verified: true,
                ..Default::default()
            },
        };
        remember_session(&mut p, server, &s);
        assert_eq!(p.online_accounts.get(server), Some(&(42, "k".repeat(64))));
        assert_eq!(p.online_names.get(server).map(String::as_str), Some("Axel"));
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains(password));
        assert!(
            !json.to_lowercase().contains("password"),
            "no password field at all"
        );
        // the Session a form returns carries no password either
        assert!(!serde_json::to_string(&s).unwrap().contains("password"));
        // logging out forgets the key (and the legacy single key of that server)
        let mut cfg = Config::default();
        cfg.net.server = server.into();
        p.online_key = "l".repeat(64);
        p.online_id = 7;
        forget_session(&mut p, &cfg, "https://rugen.example.com/");
        assert!(p.online_accounts.is_empty() && p.online_names.is_empty());
        assert!(p.online_key.is_empty() && p.online_id == 0);
        assert_eq!(account_for(&p, &cfg, server), (0, String::new()));
        // an anonymous session (claim of an older client) keeps no name
        let anon = Session {
            username: String::new(),
            ..s
        };
        remember_session(&mut p, server, &anon);
        assert!(p.online_names.is_empty());
    }

    #[test]
    fn usernames_wear_a_badge() {
        let mut p = ProfileInfo {
            name: "axel".into(),
            ..Default::default()
        };
        assert_eq!(badge_name(&p), "AXEL");
        p.verified = true;
        assert_eq!(badge_name(&p), "AXEL ✓");
    }

    fn info(api: u32, api_min: u32) -> ServerInfo {
        ServerInfo {
            api,
            api_min,
            ..Default::default()
        }
    }

    #[test]
    fn stat_rows_scale_to_the_larger_side() {
        let int = |v: f64| format!("{v:.0}");
        // 134 vs 109: the right bar is full length and red, the left shorter, ink
        let r = stat_row(Some(109.0), Some(134.0), &int);
        assert_eq!(r["l"], "109");
        assert_eq!(r["rw"], STAT_BAR as f64);
        let lw = r["lw"].as_f64().unwrap();
        assert!((lw - (STAT_BAR as f64 * 109.0 / 134.0).round()).abs() < 0.5);
        // the left bar ends at the label column
        assert_eq!(
            r["lx"].as_f64().unwrap() + lw,
            f64::from(STAT_CX - STAT_GAP)
        );
        assert_eq!(
            (r["r_hi"].as_bool(), r["l_lo"].as_bool()),
            (Some(true), Some(true))
        );
        assert_eq!(r["l_plain"], true);
        assert_eq!(r["r_plain"], false);
        // equal: both grey, nobody bold
        let e = stat_row(Some(100.0), Some(100.0), &int);
        assert_eq!(
            (e["l_eq"].as_bool(), e["r_eq"].as_bool()),
            (Some(true), Some(true))
        );
        assert_eq!(
            (e["l_hi"].as_bool(), e["r_hi"].as_bool()),
            (Some(false), Some(false))
        );
        // both zero: no bars, no division by zero
        let z = stat_row(Some(0.0), Some(0.0), &int);
        assert_eq!((z["lw"].as_f64(), z["rw"].as_f64()), (Some(0.0), Some(0.0)));
        // one fighter not picked yet: values only
        let m = stat_row(Some(5.0), None, &int);
        assert_eq!((m["l"].as_str(), m["r"].as_str()), (Some("5"), Some("-")));
        assert_eq!(m["lw"], 0);
    }

    #[test]
    fn servers_of_other_versions_get_a_reason() {
        // the 0.5-era server: too old for .rgc
        let e = outdated_error("play.example", &info(4, 0)).unwrap();
        assert!(e.contains("OLDER VERSION (API 4"), "{e}");
        // a transition server serving 0.5 and 0.6 players: fine
        assert!(outdated_error("play.example", &info(5, 4)).is_none());
        assert!(outdated_error("play.example", &info(rugen_proto::API_VERSION, 0)).is_none());
        // a server that dropped this API: update
        let e = outdated_error("play.example", &info(7, 6)).unwrap();
        assert!(e.contains("UPDATE YOUR GAME"), "{e}");
    }

    #[test]
    fn rooms_of_other_versions_say_who_must_update() {
        let room = |api| RoomSummary {
            api,
            ..Default::default()
        };
        assert!(room_error(&room(0)).is_none());
        assert!(room_error(&room(rugen_proto::API_VERSION)).is_none());
        assert_eq!(
            room_error(&room(4)).unwrap(),
            "THIS ROOM'S HOST RUNS AN OLDER RUGEN (0.5). BOTH PLAYERS NEED THE SAME VERSION."
        );
        assert!(room_error(&room(9)).unwrap().contains("UPDATE YOUR GAME"));
    }
}
