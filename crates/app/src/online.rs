//! ONLINE screens on the UI engine (`motif/ui/online.toml`): custom rooms by
//! code, ranked queue with Elo, leaderboard. Talks to `rugen-server` through
//! [`rugen_net::online::OnlineClient`] on worker threads so frames never wait.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rugen_content::Library;
use rugen_net::online::{
    Leaderboard, MatchInfo, OnlineClient, OnlineResult, Pending, ProfileInfo, QueueStatus,
    Registered, ResultAck, RoomInfo, ServerInfo, rank_name,
};
use rugen_render::{Camera, Renderer};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, ScreenDef, Ui};
use serde_json::{Value, json};
use winit::keyboard::Key;

use crate::config::Config;
use crate::game::Transition;
use crate::play::{FightSetup, NetMode};
use crate::system::SystemData;
use crate::uiscreen::{Cached, Host, MouseEvent, Profile, viewport};

/// Which online match a fight belongs to, for result reporting.
#[derive(Clone, Debug)]
pub struct OnlineMatch {
    pub match_id: u64,
    /// Account ids by side (index 0 = P1).
    pub players: [u64; 2],
    pub my_side: usize,
    pub ranked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Menu,
    Room,
    Join,
    Queue,
    Board,
}

enum Job {
    Register(Pending<Registered>),
    Info(Pending<ServerInfo>),
    Me(Pending<ProfileInfo>),
    Room(Pending<RoomInfo>),
    Queue(Pending<QueueStatus>),
    Board(Pending<Leaderboard>),
    Leave(Pending<()>),
}

pub struct OnlineScreen {
    ui: Ui,
    motif: PathBuf,
    textures: HashMap<String, Cached>,
    window: (u32, u32),
    mouse_px: Option<(f32, f32)>,
    client: OnlineClient,
    lib: Library,
    config: Config,
    profile: Profile,
    root: PathBuf,
    state: State,
    status: String,
    error: String,
    info: Option<ServerInfo>,
    me: Option<ProfileInfo>,
    room: Option<RoomInfo>,
    typed: String,
    queue: Option<QueueStatus>,
    queued_for: u32,
    board: Option<Leaderboard>,
    jobs: Vec<Job>,
    tick: u32,
    /// Set when a match is about to start; the shell takes it.
    pub started: Option<OnlineMatch>,
    pending_fight: Option<FightSetup>,
}

impl OnlineScreen {
    pub fn load(motif: &Path, root: &Path, lib: &Library, config: &Config) -> anyhow::Result<Self> {
        let path = crate::uiscreen::motif_file(motif, "ui/online.toml")
            .unwrap_or_else(|| motif.join("ui/online.toml"));
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let def = ScreenDef::parse(&text).with_context(|| path.display().to_string())?;
        let profile = Profile::load(root);
        let server = config.net.server.trim().to_string();
        anyhow::ensure!(!server.is_empty(), "[net] server is not set in rugen.toml");
        let client = OnlineClient::new(&server, Some(profile.online_key.clone()));
        let mut s = Self {
            ui: Ui::new(def, Value::Null),
            motif: motif.to_path_buf(),
            textures: HashMap::new(),
            window: (1280, 720),
            mouse_px: None,
            client,
            lib: lib.clone(),
            config: config.clone(),
            profile,
            root: root.to_path_buf(),
            state: State::Menu,
            status: "CONNECTING".into(),
            error: String::new(),
            info: None,
            me: None,
            room: None,
            typed: String::new(),
            queue: None,
            queued_for: 0,
            board: None,
            jobs: Vec::new(),
            tick: 0,
            started: None,
            pending_fight: None,
        };
        let c = s.client.clone();
        s.jobs.push(Job::Info(Pending::spawn(move || c.info())));
        if s.client.key.is_none() {
            let mut c = s.client.clone();
            s.jobs
                .push(Job::Register(Pending::spawn(move || c.register())));
        } else {
            s.fetch_me();
        }
        s.refresh();
        Ok(s)
    }

    fn fetch_me(&mut self) {
        let id = self.profile.online_id;
        if id == 0 {
            return;
        }
        let c = self.client.clone();
        self.jobs
            .push(Job::Me(Pending::spawn(move || c.profile(id))));
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

    fn refresh(&mut self) {
        let me = self.me.clone().unwrap_or_else(|| ProfileInfo {
            id: self.profile.online_id,
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
        let menu = json!([
            { "id": "room", "label": "CUSTOM ROOM", "action": "online:room" },
            { "id": "join", "label": "JOIN A ROOM", "action": "online:join" },
            { "id": "ranked", "label": "RANKED", "action": "online:ranked" },
            { "id": "board", "label": "LEADERBOARD", "action": "online:board" },
            { "id": "back", "label": "BACK", "action": "back" },
        ]);
        let data = json!({
            "menu": menu,
            "is_menu": self.state == State::Menu,
            "is_room": self.state == State::Room,
            "is_join": self.state == State::Join,
            "is_queue": self.state == State::Queue,
            "is_board": self.state == State::Board,
            "status": self.status,
            "error": self.error,
            "has_error": !self.error.is_empty(),
            "server": {
                "name": self.info.as_ref().map(|i| i.name.clone()).unwrap_or_else(|| self.config.net.server.clone()),
                "online": self.info.as_ref().map_or(0, |i| i.online),
                "queued": self.info.as_ref().map_or(0, |i| i.queued),
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
                "ready": room.is_some_and(|r| r.ready),
                "waiting": room.is_some_and(|r| !r.ready),
            },
            "typed": format!("{:<5}", self.typed).replace(' ', "_"),
            "opponent": {
                "name": opp.name,
                "rating": opp.rating,
                "rank": opp.rank,
                "record": format!("{}W {}L", opp.wins, opp.losses),
            },
            "queue": {
                "secs": self.queue.as_ref().map_or(0, |q| q.waiting_secs),
                "count": self.queue.as_ref().map_or(0, |q| q.players_in_queue),
                "found": self.queue.as_ref().is_some_and(|q| q.found.is_some()),
            },
            "board": board_rows,
            "season": self.board.as_ref().map(|b| b.season.name.clone()).unwrap_or_default(),
            "my_rank": my_rank,
            "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
        });
        self.ui.set_data(data);
    }

    /// The room-code input takes raw keys while it is being edited.
    pub fn capturing(&self) -> bool {
        self.ui.text_focused()
    }

    pub fn raw_key(&mut self, key: &Key) -> bool {
        let Some(k) = crate::uiscreen::text_key(key) else {
            return self.ui.text_focused();
        };
        if k == rugen_ui::TextKey::Escape && self.state == State::Join {
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

    fn join(&mut self) {
        let (c, code, ch) = (self.client.clone(), self.typed.clone(), self.my_char());
        self.status = format!("JOINING {code}");
        self.error.clear();
        self.jobs
            .push(Job::Room(Pending::spawn(move || c.join_room(&code, &ch))));
        self.state = State::Room;
    }

    fn action(&mut self, a: &str) -> Option<Transition> {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match (head, arg) {
            ("back", _) | ("title", _) => {
                if self.state == State::Menu {
                    return Some(Transition::Title);
                }
                self.cancel();
            }
            ("online", "room") => {
                let (c, ch, st) = (
                    self.client.clone(),
                    self.my_char(),
                    self.config.net.stage.clone(),
                );
                self.room = None;
                self.error.clear();
                self.status = "CREATING ROOM".into();
                self.state = State::Room;
                self.jobs
                    .push(Job::Room(Pending::spawn(move || c.create_room(&ch, &st))));
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
            ("join", _) => {
                if self.typed.len() == 5 {
                    self.join();
                } else {
                    self.status = "THE CODE HAS 5 LETTERS".into();
                }
            }
            ("online", "join") => {
                self.typed.clear();
                self.error.clear();
                self.status = "TYPE THE 5-LETTER ROOM CODE".into();
                self.state = State::Join;
                self.ui.set_input("code", "");
                self.refresh();
                self.ui.edit("code");
                // `[net] room` in rugen.toml pre-fills and joins straight away
                let preset = self.config.net.room.trim().to_ascii_uppercase();
                if preset.len() == 5 {
                    self.typed = preset;
                    self.join();
                }
            }
            ("online", "ranked") => {
                let (c, ch) = (self.client.clone(), self.my_char());
                self.queue = None;
                self.queued_for = 0;
                self.error.clear();
                self.status = "ENTERING QUEUE".into();
                self.state = State::Queue;
                self.jobs
                    .push(Job::Queue(Pending::spawn(move || c.enter_queue(&ch))));
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
            _ => log::warn!("online: unknown action {a:?}"),
        }
        None
    }

    fn cancel(&mut self) {
        if self.state == State::Queue {
            let c = self.client.clone();
            self.jobs
                .push(Job::Leave(Pending::spawn(move || c.leave_queue())));
        }
        self.room = None;
        self.queue = None;
        self.typed.clear();
        self.status.clear();
        self.state = State::Menu;
    }

    fn fail(&mut self, what: &str, e: impl std::fmt::Display) {
        log::warn!("online: {what}: {e}");
        self.error = format!("{what}: {e}").to_uppercase();
        self.status.clear();
        if self.state != State::Board {
            self.state = State::Menu;
        }
    }

    /// Resolve both characters and the stage, hand the shell a fight.
    fn start_fight(
        &mut self,
        code: &str,
        side: u8,
        p1_char: &str,
        p2_char: &str,
        stage: &str,
        m: OnlineMatch,
    ) {
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
        let udp_port = self.info.as_ref().map_or(7777, |i| i.udp_port);
        let server: SocketAddr = match self.client.udp_addr(udp_port) {
            Ok(a) => a,
            Err(e) => {
                self.fail("server address", e);
                return;
            }
        };
        let want = if stage.is_empty() {
            self.config.net.stage.clone()
        } else {
            stage.to_string()
        };
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
        self.pending_fight = Some(FightSetup {
            p1,
            p2,
            stage: stage_path,
            ai: [0, 0],
            net: Some(NetMode::Rendezvous {
                port: self.config.net.port,
                server,
                room: code.to_string(),
                side: usize::from(side == 2),
            }),
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
        });
        self.started = Some(m);
    }

    fn room_ready(&mut self, r: RoomInfo) {
        let my_id = self.profile.online_id;
        let opp = r.opponent.as_ref().map_or(0, |o| o.id);
        let players = if r.side == 1 {
            [my_id, opp]
        } else {
            [opp, my_id]
        };
        let m = OnlineMatch {
            match_id: r.match_id,
            players,
            my_side: usize::from(r.side == 2),
            ranked: false,
        };
        let (code, side, p1, p2, stage) = (
            r.code.clone(),
            r.side,
            r.p1_char.clone(),
            r.p2_char.clone(),
            r.stage.clone(),
        );
        self.start_fight(&code, side, &p1, &p2, &stage, m);
    }

    fn match_found(&mut self, mi: MatchInfo) {
        let my_id = self.profile.online_id;
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
            let c = self.client.clone();
            self.jobs
                .push(Job::Leave(Pending::spawn(move || c.leave_queue())));
            return;
        }
        let m = OnlineMatch {
            match_id: mi.match_id,
            players,
            my_side: usize::from(mi.side == 2),
            ranked: true,
        };
        self.status = format!("MATCH FOUND: {} ({})", mi.opponent.name, mi.opponent.rank);
        self.start_fight(&mi.code, mi.side, &p1c, &p2c, &mi.stage, m);
    }

    fn poll_jobs(&mut self) {
        let mut jobs = std::mem::take(&mut self.jobs);
        let mut keep = Vec::new();
        for mut j in jobs.drain(..) {
            match &mut j {
                Job::Register(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        self.profile.online_key = r.key.clone();
                        self.profile.online_id = r.id;
                        self.profile.save(&self.root);
                        self.client.key = Some(r.key);
                        let (c, name) = (self.client.clone(), self.profile.name.clone());
                        self.jobs
                            .push(Job::Me(Pending::spawn(move || c.set_profile(&name, 0))));
                    }
                    Some(Err(e)) => self.fail("register", e),
                },
                Job::Info(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(i)) => {
                        self.info = Some(i);
                        if self.status == "CONNECTING" {
                            self.status.clear();
                        }
                    }
                    Some(Err(e)) => self.fail("server", e),
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
                        self.me = Some(me);
                    }
                    Some(Err(e)) => log::warn!("online: profile: {e}"),
                },
                Job::Room(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(r)) => {
                        if r.ready {
                            self.room = Some(r.clone());
                            self.room_ready(r);
                        } else {
                            if self.room.as_ref().is_none_or(|old| old.code != r.code) {
                                log::info!("online: room code {}", r.code);
                            }
                            self.status = format!("ROOM {} - WAITING FOR A PLAYER", r.code);
                            self.room = Some(r);
                        }
                    }
                    Some(Err(e)) => self.fail("room", e),
                },
                Job::Queue(p) => match p.take() {
                    None => keep.push(j),
                    Some(Ok(q)) => {
                        if let Some(m) = q.found.clone() {
                            self.queue = Some(q);
                            self.match_found(m);
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
                Job::Leave(p) => {
                    if p.take().is_none() {
                        keep.push(j);
                    }
                }
            }
        }
        keep.append(&mut self.jobs);
        self.jobs = keep;
    }

    /// Periodic polling while waiting in a room or the queue.
    fn poll_server(&mut self) {
        if self.tick % 60 != 0 {
            return;
        }
        let busy_room = self.jobs.iter().any(|j| matches!(j, Job::Room(_)));
        let busy_queue = self.jobs.iter().any(|j| matches!(j, Job::Queue(_)));
        match self.state {
            State::Room if !busy_room => {
                if let Some(r) = &self.room
                    && !r.ready
                {
                    let (c, code) = (self.client.clone(), r.code.clone());
                    self.jobs
                        .push(Job::Room(Pending::spawn(move || c.poll_room(&code))));
                }
            }
            State::Queue if !busy_queue => {
                self.queued_for += 1;
                let c = self.client.clone();
                self.jobs
                    .push(Job::Queue(Pending::spawn(move || c.poll_queue())));
            }
            _ => {}
        }
        if self.tick % 600 == 0 {
            let c = self.client.clone();
            self.jobs.push(Job::Info(Pending::spawn(move || c.info())));
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
                accept: any(|b| b.a || b.b || b.c || b.x || b.y || b.z || b.s),
                back: false,
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
                    if self.state == State::Menu {
                        out = Some(Transition::Title);
                    } else {
                        self.cancel();
                    }
                }
                Event::Action(a) => {
                    if !a.starts_with("typed:") {
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
        if self.state == State::Menu {
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

/// Report a finished online match on a worker thread.
pub fn report_result(
    config: &Config,
    root: &Path,
    m: &OnlineMatch,
    winner_side: Option<usize>,
) -> Option<Pending<ResultAck>> {
    let winner_side = winner_side?;
    let profile = Profile::load(root);
    let c = OnlineClient::new(config.net.server.trim(), Some(profile.online_key));
    let (id, winner) = (m.match_id, m.players[winner_side]);
    log::info!(
        "online: reporting {} match {id}, winner {winner}",
        if m.ranked { "ranked" } else { "room" }
    );
    Some(Pending::spawn(move || c.report(id, winner)))
}

/// Apply an accepted result to the local profile.
pub fn apply_result(root: &Path, ack: &OnlineResult<ResultAck>, won: bool) {
    match ack {
        Ok(a) => {
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
