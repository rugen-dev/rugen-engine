//! ONLINE screens on the UI engine (`motif/ui/online.toml`).
//!
//! 1. **Servers**: `[net] server` + `[net] servers` favourites with name,
//!    MOTD, players online and ping; ADD SERVER saves a new favourite.
//! 2. **Lobby** of the chosen server: public rooms (join / watch), create a
//!    room (name, public / private, password, rules, spectator seats), join
//!    by code (+ password), quick match, ranked, leaderboard.
//! 3. Matches are **relayed by the server** (UDP fast path, WebSocket
//!    fallback): no player ever learns another's address.
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
    Evidence, Leaderboard, MatchInfo, OnlineClient, OnlineResult, Pending, Presence, ProfileInfo,
    QueueStatus, Registered, ResultAck, RoomInfo, RoomOptions, RoomSummary, Rules, ServerInfo,
    ServerMsg, SessionTicket, base_url, ping_bars, rank_name,
};
use rugen_render::{Camera, Renderer};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, ScreenDef, Ui};
use serde_json::{Value, json};
use winit::keyboard::Key;

use crate::config::{Config, GOOD_PING_MS};
use crate::game::Transition;
use crate::play::{FightSetup, NetMode};
use crate::system::SystemData;
use crate::uiscreen::{Cached, Host, MouseEvent, Profile, viewport};

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
    /// Room to return to (rematch), if any.
    pub room: Option<String>,
}

/// Where the ONLINE screen resumes after a match: (server, room code).
static RESUME: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Servers,
    AddServer,
    Menu,
    Lobby,
    Create,
    Join,
    Room,
    Queue,
    Board,
    /// Typing the peer's address for a direct (LAN / IP) match.
    Direct,
}

enum Job {
    Register(Pending<Registered>),
    Info(usize, Pending<(ServerInfo, u32)>),
    Me(Pending<ProfileInfo>),
    Rooms(Pending<Vec<RoomSummary>>),
    Room(Pending<RoomInfo>),
    Queue(Pending<QueueStatus>),
    Board(Pending<Leaderboard>),
    Done(Pending<()>),
}

/// One row of the server list.
#[derive(Clone, Debug, Default)]
struct ServerRow {
    addr: String,
    info: Option<ServerInfo>,
    ping: Option<u32>,
    error: bool,
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
}

/// Account (id, key) for `server`; the legacy single key belongs to `[net] server`.
fn account_for(profile: &Profile, config: &Config, server: &str) -> (u64, String) {
    if let Some((id, key)) = profile.online_accounts.get(server) {
        return (*id, key.clone());
    }
    if server.eq_ignore_ascii_case(config.net.server.trim()) && !profile.online_key.is_empty() {
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
                .server_list()
                .into_iter()
                .map(|addr| ServerRow {
                    addr,
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
            direct_side: 0,
            addr: String::new(),
            local_ip: rugen_net::socket::lan_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "YOUR-IP".into()),
        };
        s.status = if s.servers.is_empty() {
            "NO SERVER YET: ADD ONE, OR PLAY A DIRECT (LAN) MATCH".into()
        } else {
            "PICK A SERVER".into()
        };
        s.probe_servers();
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
        Ok(s)
    }

    /// Ask every listed server for its info and measure the round trip.
    fn probe_servers(&mut self) {
        for (i, row) in self.servers.iter().enumerate() {
            let c = OnlineClient::new(&row.addr, None);
            self.jobs.push(Job::Info(
                i,
                Pending::spawn(move || {
                    let info = c.info()?;
                    let ping = c.ping()?;
                    Ok((info, ping))
                }),
            ));
        }
    }

    /// Pick a server: account (register on first use), presence socket, menu.
    fn connect(&mut self, server: &str) {
        let (id, key) = account_for(&self.profile, &self.config, server);
        self.client = OnlineClient::new(server, (!key.is_empty()).then_some(key));
        self.me = None;
        self.state = State::Menu;
        self.error.clear();
        self.status = format!("CONNECTING TO {}", server.to_uppercase());
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

    fn refresh(&mut self) {
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
        let opp_ping = room.and_then(|r| r.est_ping_ms).or_else(|| {
            self.queue
                .as_ref()
                .and_then(|q| q.found.as_ref())
                .and_then(|m| m.est_ping_ms)
        });
        let board_rows: Vec<Value> = self
            .board
            .as_ref()
            .map(|b| {
                b.rows
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        json!({
                            "pos": i + 1,
                            "name": p.name,
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
            .map(|(pos, _)| format!("YOUR RANK #{pos}"))
            .unwrap_or_else(|| "UNRANKED - PLAY A RANKED MATCH".into());
        let has_server = self.client.key.is_some() || self.state != State::Servers;
        let menu: Vec<Value> = match self.state {
            State::Servers => {
                let mut m: Vec<Value> = self
                    .servers
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let label = match (&r.info, r.error) {
                            (Some(info), _) => format!(
                                "{}  {}",
                                info.name.to_uppercase().chars().take(14).collect::<String>(),
                                ping_text(r.ping)
                            ),
                            (None, true) => format!("{} OFFLINE", r.addr.to_uppercase().chars().take(14).collect::<String>()),
                            (None, false) => format!("{} ...", r.addr.to_uppercase().chars().take(16).collect::<String>()),
                        };
                        json!({ "id": format!("srv{i}"), "label": label, "action": format!("server:{i}") })
                    })
                    .collect();
                m.extend([
                    json!({ "id": "add", "label": "ADD SERVER", "action": "online:add" }),
                    json!({ "id": "host", "label": "HOST DIRECT (LAN)", "action": "online:host" }),
                    json!({ "id": "direct", "label": "JOIN DIRECT (LAN)", "action": "online:direct" }),
                ]);
                if !self.config.net.remote.trim().is_empty() {
                    m.push(json!({ "id": "netplay", "label": "NETPLAY (RUGEN.TOML)", "action": "netplay" }));
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
        let lobby: Vec<Value> = self
            .rooms
            .iter()
            .map(|r| {
                json!({
                    "name": format!("{}{}", if r.locked { "[LOCKED] " } else { "" }, r.name.to_uppercase()),
                    "host": r.host.to_uppercase(),
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
                            "name": m.profile.name.to_uppercase(),
                            "role": role_name(m.role),
                            "ping": ping_text(m.rtt_ms),
                            "kick": format!("room:kick:{}", m.profile.id),
                            "can_kick": my_role == 0 && m.role != 0,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let server_line = if self.state == State::Servers {
            "SERVERS".to_string()
        } else {
            let info = self
                .servers
                .iter()
                .find(|r| r.addr == self.client.server)
                .and_then(|r| r.info.clone());
            format!(
                "{}   {} ONLINE   {} ROOMS   {}",
                self.server_name().to_uppercase(),
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
            "is_menu": matches!(self.state, State::Menu | State::Servers),
            "is_servers": self.state == State::Servers,
            "is_add": self.state == State::AddServer,
            "is_lobby": self.state == State::Lobby,
            "is_create": self.state == State::Create,
            "is_room": self.state == State::Room,
            "is_join": self.state == State::Join,
            "is_queue": self.state == State::Queue,
            "is_board": self.state == State::Board,
            "is_direct": self.state == State::Direct,
            "has_server": has_server && self.state != State::Servers,
            "no_server": self.state == State::Servers,
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
                "name": me.name,
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
                "name": opp.name,
                "rating": opp.rating,
                "rank": opp.rank,
                "record": format!("{}W {}L", opp.wins, opp.losses),
                "ping": ping_text(opp_ping),
            },
            "queue": {
                "title": if self.ranked { "RANKED" } else { "QUICK MATCH" },
                "secs": self.queue.as_ref().map_or(0, |q| q.waiting_secs),
                "count": self.queue.as_ref().map_or(0, |q| q.players_in_queue),
                "found": self.queue.as_ref().is_some_and(|q| q.found.is_some()),
            },
            "board": board_rows,
            "season": self.board.as_ref().map(|b| b.season.name.clone()).unwrap_or_default(),
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
                State::Join | State::Direct | State::AddServer | State::Create
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
        if let Err(e) = c.save(&self.root) {
            log::warn!("could not save rugen.toml: {e}");
        }
    }

    fn action(&mut self, a: &str) -> Option<Transition> {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match (head, arg) {
            ("back", _) | ("title", _) => {
                if self.state == State::Servers {
                    return Some(Transition::Title);
                }
                self.cancel();
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
                if !self
                    .servers
                    .iter()
                    .any(|r| r.addr.eq_ignore_ascii_case(&addr))
                {
                    self.servers.push(ServerRow {
                        addr: addr.clone(),
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
            ("online", "ranked") => self.enter_queue(true),
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
            lib.prepare_char(&e).ok().map(|(d, man)| d.join(man.def))
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
            .and_then(|e| self.lib.prepare_stage(&e).ok().map(|(d, m)| d.join(m.def)))
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

    fn cancel(&mut self) {
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
            State::Servers | State::AddServer | State::Direct
        ) || self.client.server.is_empty()
        {
            State::Servers
        } else {
            State::Menu
        };
    }

    fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        log::warn!("online: {what}: {e}");
        self.error = format!("{what}: {e}").to_uppercase();
        self.status.clear();
        if !matches!(self.state, State::Board | State::Servers | State::Lobby) {
            self.state = if self.client.server.is_empty() {
                State::Servers
            } else {
                State::Menu
            };
        }
    }

    /// Resolve both characters and the stage, hand the shell a relayed fight.
    fn start_fight(
        &mut self,
        p1_char: &str,
        p2_char: &str,
        stage: &str,
        ticket: SessionTicket,
        m: OnlineMatch,
    ) {
        if self.used_ticket == Some(ticket.session) {
            return;
        }
        let resolve = |lib: &Library, id: &str| -> Option<PathBuf> {
            let e = lib.find_char(id)?;
            lib.prepare_char(&e).ok().map(|(d, man)| d.join(man.def))
        };
        let (Some(p1), Some(p2)) = (resolve(&self.lib, p1_char), resolve(&self.lib, p2_char))
        else {
            let missing = if resolve(&self.lib, p1_char).is_none() {
                p1_char
            } else {
                p2_char
            };
            self.fail(
                "cannot start",
                format!("character {missing:?} is not installed here"),
            );
            return;
        };
        let want = if stage.is_empty() {
            self.config.net.stage.clone()
        } else {
            stage.to_string()
        };
        // both sides must pick the same stage: by id, else the first one sorted
        let stage_path = self
            .lib
            .stages()
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(&want))
            .or_else(|| self.lib.stages().into_iter().next())
            .and_then(|e| {
                self.lib
                    .prepare_stage(&e)
                    .ok()
                    .map(|(d, man)| d.join(man.def))
            })
            .or_else(|| self.lib.default_stage());
        self.used_ticket = Some(ticket.session);
        if let Ok(mut r) = RESUME.lock() {
            *r = Some((self.client.server.clone(), m.room.clone()));
        }
        self.pending_fight = Some(FightSetup {
            p1,
            p2,
            stage: stage_path,
            ai: [0, 0],
            net: Some(NetMode::Relay {
                server: base_url(&self.client.server),
                ticket,
                websocket_only: self.config.net.websocket_only,
            }),
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
        });
        self.started = Some(m);
    }

    fn room_update(&mut self, r: RoomInfo) {
        if r.ready
            && let Some(ticket) = r.ticket.clone()
            && self.used_ticket != Some(ticket.session)
        {
            let my_id = self.my_id();
            let opp = r.opponent.as_ref().map_or(0, |o| o.id);
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
            let players = match r.side {
                1 => [my_id, opp],
                2 => [opp, my_id],
                _ => [host, chal],
            };
            let m = OnlineMatch {
                server: self.client.server.clone(),
                match_id: r.match_id,
                players,
                my_side: usize::from(r.side == 2),
                ranked: false,
                spectator: r.side > 2,
                rules: r.rules.clone(),
                room: Some(r.code.clone()),
            };
            self.status = format!("ROOM {} - MATCH STARTING", r.code);
            let (p1, p2, stage) = (r.p1_char.clone(), r.p2_char.clone(), r.stage.clone());
            self.room = Some(r);
            self.start_fight(&p1, &p2, &stage, ticket, m);
            return;
        }
        if self.room.as_ref().is_none_or(|old| old.code != r.code) {
            log::info!("online: room code {}", r.code);
        }
        self.status = if r.opponent.is_none() && r.side <= 2 {
            format!("ROOM {} - WAITING FOR A CHALLENGER", r.code)
        } else if r.side > 2 {
            format!("ROOM {} - WATCHING, WAITING FOR THE NEXT MATCH", r.code)
        } else {
            format!("ROOM {} - REMATCH?", r.code)
        };
        self.room = Some(r);
    }

    fn match_found(&mut self, mi: MatchInfo) {
        let my_id = self.my_id();
        let Some(ticket) = mi.ticket.clone() else {
            self.fail("match", "the server sent no relay ticket");
            return;
        };
        let (p1c, p2c, players) = if mi.side == 1 {
            (
                mi.my_char.clone(),
                mi.opponent_char.clone(),
                [my_id, mi.opponent.id],
            )
        } else {
            (
                mi.opponent_char.clone(),
                mi.my_char.clone(),
                [mi.opponent.id, my_id],
            )
        };
        if self.lib.find_char(&mi.opponent_char).is_none() {
            self.fail(
                "match declined",
                format!("opponent plays {:?}, not installed here", mi.opponent_char),
            );
            return;
        }
        let m = OnlineMatch {
            server: self.client.server.clone(),
            match_id: mi.match_id,
            players,
            my_side: usize::from(mi.side == 2),
            ranked: mi.ranked,
            spectator: false,
            rules: Rules::default(),
            room: None,
        };
        self.status = format!(
            "MATCH FOUND: {} ({})  {}",
            mi.opponent.name,
            mi.opponent.rank,
            ping_text(mi.est_ping_ms)
        );
        self.start_fight(&p1c, &p2c, &mi.stage, ticket, m);
    }

    fn poll_presence(&mut self) {
        let Some(p) = self.presence.as_mut() else {
            return;
        };
        for m in p.drain() {
            match m {
                ServerMsg::Welcome { id, .. } => log::debug!("online: presence as {id}"),
                ServerMsg::Room { room } => {
                    if self.state == State::Room
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
                ServerMsg::Pong | ServerMsg::Error { .. } => {}
            }
        }
    }

    fn poll_jobs(&mut self) {
        let mut jobs = std::mem::take(&mut self.jobs);
        let mut keep = Vec::new();
        for mut j in jobs.drain(..) {
            match &mut j {
                Job::Register(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        self.profile
                            .online_accounts
                            .insert(self.client.server.clone(), (r.id, r.key.clone()));
                        self.profile.save(&self.root);
                        self.client.key = Some(r.key);
                        self.presence = Presence::spawn(&self.client).ok();
                        let (c, name) = (self.client.clone(), self.profile.name.clone());
                        self.jobs
                            .push(Job::Me(Pending::spawn(move || c.set_profile(&name, 0))));
                    }
                    Some(Err(e)) => self.fail("register", e),
                },
                Job::Info(i, p) => match p.take() {
                    None => keep.push(j),
                    Some(r) => {
                        let i = *i;
                        if let Some(row) = self.servers.get_mut(i) {
                            match r {
                                Ok((info, ping)) => {
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
                Job::Me(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(me)) => {
                        if me.name != self.profile.name && self.me.is_none() {
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
                        if self.state == State::Room {
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
    }

    /// Periodic polling (rooms are also a heartbeat; pushes arrive via presence).
    fn poll_server(&mut self) {
        if self.client.server.is_empty() || self.tick % 60 != 0 {
            return;
        }
        let busy_room = self.jobs.iter().any(|j| matches!(j, Job::Room(_)));
        let busy_queue = self.jobs.iter().any(|j| matches!(j, Job::Queue(_)));
        match self.state {
            State::Room if !busy_room => {
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
        if let Some(f) = self.pending_fight.take() {
            return Some(Transition::Fight(f));
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
                mem: None,
                rot: None,
            };
            self.ui.tick(nav, &mut host)
        };
        let mut out = None;
        for e in events {
            match e {
                Event::Moved => sfx(sys.title.move_snd),
                Event::Back => {
                    if self.state == State::Servers {
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
        if self.state == State::Servers {
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
                mem: None,
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

/// `rugen play` online from the command line: join (or create with
/// `new`) a room on `server` and block until the match starts. Returns the
/// relay fight mode, both character ids and the match bookkeeping.
pub fn cli_session(
    root: &Path,
    config: &Config,
    server: &str,
    room: &str,
    my_char: &str,
    spectate: bool,
    guest: bool,
) -> anyhow::Result<(NetMode, String, String, OnlineMatch)> {
    let mut profile = Profile::load(root);
    let (_, key) = if guest {
        (0, String::new())
    } else {
        account_for(&profile, config, server)
    };
    let mut client = OnlineClient::new(server, (!key.is_empty()).then_some(key));
    if client.key.is_none() {
        let r = client.register().context("registering on the server")?;
        if !guest {
            profile
                .online_accounts
                .insert(server.to_string(), (r.id, r.key));
            profile.save(root);
        }
        let _ = client.set_profile(&profile.name, 0);
    }
    let mut info = if room.eq_ignore_ascii_case("new") {
        let r = client.create_room(
            my_char,
            &RoomOptions {
                name: format!("{} CLI", profile.name),
                public: true,
                rules: Rules::default(),
                max_spectators: 2,
                ..Default::default()
            },
        )?;
        println!("ROOM CODE: {}", r.code);
        log::info!("online: room {} created, waiting for a challenger", r.code);
        r
    } else {
        client.join_room(room, my_char, "", spectate)?
    };
    let start = std::time::Instant::now();
    while info.ticket.is_none() {
        if start.elapsed() > std::time::Duration::from_secs(300) {
            anyhow::bail!("nobody joined room {} within 5 minutes", info.code);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        info = client.poll_room(&info.code)?;
    }
    let ticket = info.ticket.clone().unwrap_or_default();
    let host = info
        .members
        .iter()
        .find(|m| m.role == 0)
        .map_or(0, |m| m.profile.id);
    let chal = info
        .members
        .iter()
        .find(|m| m.role == 1)
        .map_or(0, |m| m.profile.id);
    let m = OnlineMatch {
        server: server.to_string(),
        match_id: info.match_id,
        players: [host, chal],
        my_side: usize::from(info.side == 2),
        ranked: false,
        spectator: info.side > 2,
        rules: info.rules.clone(),
        room: Some(info.code.clone()),
    };
    log::info!(
        "online: room {} match {} as {}",
        info.code,
        info.match_id,
        role_name(info.side.saturating_sub(1))
    );
    let mode = NetMode::Relay {
        server: base_url(server),
        ticket,
        websocket_only: config.net.websocket_only,
    };
    Ok((mode, info.p1_char.clone(), info.p2_char.clone(), m))
}
