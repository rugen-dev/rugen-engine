//! `rugen.toml`: video, input and net settings. Written with defaults on first run.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub video: Video,
    pub camera: CameraCfg,
    pub audio: AudioCfg,
    pub input: Input,
    pub net: Net,
    pub game: GameCfg,
    pub content: ContentCfg,
}

/// `[content]`: how `chars/` and `stages/` are imported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentCfg {
    /// After an archive (.zip / .rar) is converted to `.rgc` / `.rgs` and the
    /// new file reads back with the same fingerprint, move the archive to
    /// `chars/originals/` (`stages/originals/`). `false` deletes it instead.
    /// Folders are never moved or deleted.
    pub keep_originals: bool,
}

impl Default for ContentCfg {
    fn default() -> Self {
        Self {
            keep_originals: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameCfg {
    /// Computer opponent level 1..=8.
    pub ai_level: i32,
    /// Extra air jumps for every character (stages may override in `[Arena]`).
    pub air_jumps: i32,
    /// Double-tap forward/back in the air to dash.
    pub air_dash: bool,
    /// Hold down while falling to drop faster.
    pub fast_fall: bool,
    /// Arena AI: computer fighters on arena stages avoid the pit, recover
    /// and use platforms (their own AI still picks the attacks).
    pub arena_ai: bool,
    /// Seconds on the round clock in offline matches; 0 = no timer.
    pub round_time: i32,
    /// FIGHTER SIZE: fighters already inside the band keep their size, the
    /// rest get a round x1.1..x3.0 or /1.1../3.0 step (0.1 apart) into it (320x240 units: `normal`
    /// 58-82, `small` 62-78, `chibi` 44-56); `none` never scales.
    pub fighter_size: rugen_sim::scale::FighterSize,
}

impl Default for GameCfg {
    fn default() -> Self {
        Self {
            ai_level: 4,
            air_jumps: 0,
            air_dash: false,
            fast_fall: false,
            arena_ai: true,
            round_time: 99,
            fighter_size: rugen_sim::scale::FighterSize::Normal,
        }
    }
}

/// Camera limits applied on top of every stage: players can separate until the
/// view has pulled back to `zoom_out`, and the view closes in up to `zoom_in`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraCfg {
    /// Smallest scale (1 = never pull back, 0.5 = twice the width visible).
    pub zoom_out: f32,
    /// Largest scale when players are close (1 = the stage's own screen,
    /// below 1 = always a wider view).
    pub zoom_in: f32,
}

/// `zoom_in` default before v0.3.1; configs still holding it get the new one.
const OLD_DEFAULT_ZOOM_IN: f32 = 1.25;

impl Default for CameraCfg {
    fn default() -> Self {
        Self {
            zoom_out: 0.6,
            zoom_in: 0.85,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioCfg {
    /// 0..1
    pub master: f32,
    pub music: f32,
}

impl Default for AudioCfg {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 0.8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    /// UI theme: a folder under `motif/themes/` (`ink`, `dojo`, ...) or `classic` for the flat `motif/`.
    pub theme: String,
    /// Lowest-latency presentation: mailbox (or FIFO with one queued
    /// frame), a frame drawn right after each tick. Off: the driver's
    /// default vsync with frames drawn continuously (up to two queued).
    pub low_latency: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub p1: PlayerKeys,
    pub p2: PlayerKeys,
    /// Gamepad slots (connection order) for each side; -1 = none.
    pub gamepad_p1: i32,
    pub gamepad_p2: i32,
    /// Analog stick deadzone 0..1.
    pub deadzone: f32,
    /// Let the mouse hover and click menus (off: keyboard / pad only, cursor hidden).
    pub mouse: bool,
}

/// Keyboard mapping. Names are winit logical keys: single characters
/// (`"a"`, `","`) or named keys (`"ArrowUp"`, `"Enter"`, `"Space"`, `"Shift"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerKeys {
    pub up: String,
    pub down: String,
    pub left: String,
    pub right: String,
    pub a: String,
    pub b: String,
    pub c: String,
    pub x: String,
    pub y: String,
    pub z: String,
    /// MUGEN's seventh button `s`: a fighter button (power charge, taunts).
    /// Missing in configs older than v0.6.1, where `start` was this button
    /// (see [`PlayerKeys::migrate`]).
    #[serde(default)]
    pub s: String,
    /// START: pause, menus, confirm. Never a fighter input (not sent online,
    /// not recorded in replays).
    pub start: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Net {
    /// Local UDP port for `--host` / `--join`.
    pub port: u16,
    /// Frames of local input delay: `"auto"` (picked from the measured
    /// round trip of the chosen path when the match starts, see
    /// `rugen_net::DelayPolicy`) or a number of frames.
    pub input_delay: InputDelay,
    /// Smallest automatic input delay (frames).
    pub min_delay: usize,
    /// Rollback window: frames the game may predict the opponent before it
    /// has to wait. Automatic input delay only covers latency beyond it.
    pub prediction_window: usize,
    /// Old name of the rollback window (read, never written: the old
    /// default of 8 frames is replaced by `prediction_window`).
    #[serde(skip_serializing)]
    pub max_prediction: Option<usize>,
    /// Online matches connect the two players directly (peer to peer, the
    /// fastest path; the opponent's game sees your IP address). Off: always
    /// relayed (TURN / the server), private. Off on either side = relayed.
    pub direct: bool,
    /// Quick match / ranked: only accept opponents with an estimated ping
    /// up to [`GOOD_PING_MS`] (the window never widens past it).
    pub good_connections_only: bool,
    /// LAN / explicit-IP peer `ip:port` used by the NETPLAY menu entry
    /// (empty = entry hidden). The address is typed by the players
    /// themselves; online play never exchanges addresses.
    pub remote: String,
    /// Which side this machine plays in the NETPLAY menu entry: 1 or 2.
    pub side: u8,
    /// Characters and stage for the NETPLAY menu entry (ids from chars/ and stages/).
    pub p1: String,
    pub p2: String,
    pub stage: String,
    /// Room code pre-filled by JOIN A ROOM.
    pub room: String,
    /// Server selected by default in the ONLINE list: empty = the official
    /// server (built in, always listed first), else one of `servers`.
    pub server: String,
    /// Community servers (`https://host` or `host:port`) listed below the
    /// official one. The official server is never stored here.
    pub servers: Vec<String>,
    /// Skip the UDP fast path and relay over the WebSocket only.
    pub websocket_only: bool,
}

/// One row of the ONLINE server list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEntry {
    pub addr: String,
    /// The built-in official server (pinned first, cannot be removed).
    pub official: bool,
}

impl Net {
    /// The automatic input delay policy of these settings.
    pub fn delay_policy(&self) -> rugen_net::DelayPolicy {
        let d = rugen_net::DelayPolicy::default();
        rugen_net::DelayPolicy {
            min_delay: self.min_delay.min(InputDelay::MAX),
            prediction_window: self.prediction_window.clamp(4, 16),
            headroom: d.headroom,
        }
    }

    /// Bring `server` / `servers` to the official + community layout:
    /// the official address leaves `servers` (and `server` becomes empty
    /// when it named it); a custom default server (older configs) joins the
    /// community list; duplicates go. True when anything changed.
    pub fn migrate_servers(&mut self, official: &str) -> bool {
        let same = crate::official::same_server;
        let before = (self.server.clone(), self.servers.clone());
        let mut community: Vec<String> = Vec::new();
        let default = self.server.trim().to_string();
        let extra = (!default.is_empty()).then_some(&default);
        for s in extra.into_iter().chain(&self.servers) {
            let s = s.trim();
            if s.is_empty() || same(s, official) || community.iter().any(|c| same(c, s)) {
                continue;
            }
            community.push(s.to_string());
        }
        self.servers = community;
        if same(&default, official) {
            self.server.clear();
        } else {
            self.server = default;
        }
        (self.server.clone(), self.servers.clone()) != before
    }

    /// The server list: the official server first, then the community ones.
    pub fn server_list(&self, official: &str) -> Vec<ServerEntry> {
        let mut v = vec![ServerEntry {
            addr: official.to_string(),
            official: true,
        }];
        v.extend(
            self.servers
                .iter()
                .filter(|s| !s.trim().is_empty() && !crate::official::same_server(s, official))
                .map(|s| ServerEntry {
                    addr: s.trim().to_string(),
                    official: false,
                }),
        );
        v
    }

    /// Index in [`Net::server_list`] of the default selection.
    pub fn default_index(&self, official: &str) -> usize {
        if self.server.trim().is_empty() {
            return 0;
        }
        self.server_list(official)
            .iter()
            .position(|e| crate::official::same_server(&e.addr, &self.server))
            .unwrap_or(0)
    }

    /// Remove a community server (never the official one). True when removed.
    pub fn remove_server(&mut self, addr: &str, official: &str) -> bool {
        if crate::official::same_server(addr, official) {
            return false;
        }
        let n = self.servers.len();
        self.servers
            .retain(|s| !crate::official::same_server(s, addr));
        if crate::official::same_server(&self.server, addr) {
            self.server.clear();
        }
        self.servers.len() != n
    }
}

/// Estimated ping limit for `good_connections_only`.
pub const GOOD_PING_MS: u32 = 80;

/// `input_delay = "auto"` or `input_delay = 2` in rugen.toml.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InputDelay(pub Option<usize>);

impl InputDelay {
    pub const MAX: usize = 8;

    /// Frames for a measured p95 round trip (auto, with `policy`) or the
    /// fixed setting.
    pub fn frames(self, rtt_p95_ms: Option<u32>, policy: &rugen_net::DelayPolicy) -> usize {
        match (self.0, rtt_p95_ms) {
            (Some(n), _) => n,
            (None, Some(rtt)) => policy.delay(rtt),
            // auto without a measurement: a safe middle
            (None, None) => 2usize.max(policy.min_delay),
        }
    }

    /// Cycle AUTO, 0 .. MAX (menus): `dir` +1 / -1.
    pub fn step(self, dir: i32, wrap: bool) -> Self {
        // AUTO is position 0, n frames is n + 1
        let pos = self.0.map_or(0, |n| n as i32 + 1);
        let last = Self::MAX as i32 + 1;
        let next = if wrap && pos + dir > last {
            0
        } else {
            (pos + dir).clamp(0, last)
        };
        Self((next > 0).then(|| next as usize - 1))
    }

    /// Menu position (0 = AUTO) and labels.
    pub fn choice(self) -> (Vec<String>, usize) {
        let mut v = vec!["AUTO".to_string()];
        v.extend((0..=Self::MAX).map(|n| format!("{n} F")));
        (v, self.0.map_or(0, |n| n + 1))
    }
}

impl std::fmt::Display for InputDelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(n) => write!(f, "{n} F"),
            None => f.write_str("AUTO"),
        }
    }
}

impl Serialize for InputDelay {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Some(n) => s.serialize_u64(n as u64),
            None => s.serialize_str("auto"),
        }
    }
}

impl<'de> Deserialize<'de> for InputDelay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            N(i64),
            S(String),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::N(n) => InputDelay(Some(n.clamp(0, Self::MAX as i64) as usize)),
            Raw::S(s) => match s.trim().parse::<usize>() {
                Ok(n) => InputDelay(Some(n.min(Self::MAX))),
                Err(_) => InputDelay(None),
            },
        })
    }
}

impl Default for Video {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fullscreen: false,
            theme: "ink".into(),
            low_latency: true,
        }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self {
            p1: PlayerKeys::p1(),
            p2: PlayerKeys::p2(),
            gamepad_p1: 0,
            gamepad_p2: 1,
            deadzone: 0.5,
            mouse: false,
        }
    }
}

impl Default for PlayerKeys {
    fn default() -> Self {
        Self::p1()
    }
}

impl PlayerKeys {
    /// AZERTY ZQSD layout (player 1 default).
    pub fn p1() -> Self {
        Self {
            up: "z".into(),
            down: "s".into(),
            left: "q".into(),
            right: "d".into(),
            a: "j".into(),
            b: "k".into(),
            c: "l".into(),
            x: "u".into(),
            y: "i".into(),
            z: "o".into(),
            s: "m".into(),
            start: "p".into(),
        }
    }
    /// Arrows + number keys (player 2 default).
    pub fn p2() -> Self {
        Self {
            up: "ArrowUp".into(),
            down: "ArrowDown".into(),
            left: "ArrowLeft".into(),
            right: "ArrowRight".into(),
            a: "1".into(),
            b: "2".into(),
            c: "3".into(),
            x: "4".into(),
            y: "5".into(),
            z: "6".into(),
            s: "7".into(),
            start: "Enter".into(),
        }
    }
    /// Set the key for the button at `i` (order: u d l r a b c x y z s start).
    pub fn set(&mut self, i: usize, name: &str) {
        let slot = match i {
            0 => &mut self.up,
            1 => &mut self.down,
            2 => &mut self.left,
            3 => &mut self.right,
            4 => &mut self.a,
            5 => &mut self.b,
            6 => &mut self.c,
            7 => &mut self.x,
            8 => &mut self.y,
            9 => &mut self.z,
            10 => &mut self.s,
            _ => &mut self.start,
        };
        *slot = name.to_string();
    }

    /// Mapping in order: u d l r a b c x y z s start (Buttons order + START).
    pub fn names(&self) -> [&str; 12] {
        [
            &self.up,
            &self.down,
            &self.left,
            &self.right,
            &self.a,
            &self.b,
            &self.c,
            &self.x,
            &self.y,
            &self.z,
            &self.s,
            &self.start,
        ]
    }

    /// START keys tried, in order, for a config from before START had its
    /// own binding: the new default first.
    fn start_candidates(player: usize) -> &'static [&'static str] {
        if player == 0 {
            &["p", "m", "Space", "Tab"]
        } else {
            &["Enter", "0", "8", "9", "Backspace"]
        }
    }

    /// The old default START key, which was also MUGEN's `s` button.
    fn old_default_start(player: usize) -> &'static str {
        if player == 0 { "m" } else { "Enter" }
    }
}

impl Input {
    /// Configs older than v0.6.1 had one `start` key for both MUGEN's `s`
    /// button and the menus. Split it: a player still on the old default
    /// gets the new default pair; a custom key stays the `s` button and START
    /// takes the new default, or the next free candidate when that key is
    /// already bound. True when anything changed.
    pub fn migrate_start(&mut self) -> bool {
        let mut changed = false;
        for p in 0..2 {
            let keys = if p == 0 { &self.p1 } else { &self.p2 };
            if !keys.s.is_empty() {
                continue;
            }
            let old = keys.start.clone();
            let fresh = if p == 0 {
                PlayerKeys::p1()
            } else {
                PlayerKeys::p2()
            };
            let (s, start) = if old.is_empty() || old == PlayerKeys::old_default_start(p) {
                (fresh.s, fresh.start)
            } else {
                let taken = |k: &str| {
                    [&self.p1, &self.p2]
                        .iter()
                        .any(|pk| pk.names().iter().any(|n| n.eq_ignore_ascii_case(k)))
                };
                let start = PlayerKeys::start_candidates(p)
                    .iter()
                    .find(|c| !c.eq_ignore_ascii_case(&old) && !taken(c))
                    .map_or(String::new(), |c| c.to_string());
                (old, start)
            };
            let keys = if p == 0 { &mut self.p1 } else { &mut self.p2 };
            keys.s = s;
            keys.start = start;
            changed = true;
        }
        changed
    }
}

impl Default for Net {
    fn default() -> Self {
        Self {
            port: 7000,
            input_delay: InputDelay(None),
            min_delay: 1,
            prediction_window: 12,
            max_prediction: None,
            direct: true,
            good_connections_only: false,
            remote: String::new(),
            side: 1,
            p1: "kfm".into(),
            p2: "kfm".into(),
            stage: String::new(),
            room: String::new(),
            server: String::new(),
            servers: Vec::new(),
            websocket_only: false,
        }
    }
}

impl Config {
    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(root.join("rugen.toml"), text)?;
        Ok(())
    }

    /// Load `<root>/rugen.toml`, creating it with defaults when missing.
    pub fn load(root: &Path) -> Self {
        let path = root.join("rugen.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    if c.input.migrate_start() {
                        log::info!(
                            "rugen.toml: START split from the S button (P1 S {:?} START {:?}, P2 S {:?} START {:?})",
                            c.input.p1.s,
                            c.input.p1.start,
                            c.input.p2.s,
                            c.input.p2.start
                        );
                    }
                    if c.camera.zoom_in == OLD_DEFAULT_ZOOM_IN {
                        c.camera.zoom_in = CameraCfg::default().zoom_in;
                    }
                    // official server built in: [net] servers = community only
                    if c.net.migrate_servers(crate::official::server()) {
                        log::info!(
                            "rugen.toml: [net] servers migrated (official server is built in)"
                        );
                    }
                    c
                }
                Err(e) => {
                    log::warn!("{}: {e}; using defaults", path.display());
                    Config::default()
                }
            },
            Err(_) => {
                let c = Config::default();
                if let Ok(text) = toml::to_string_pretty(&c)
                    && let Err(e) = std::fs::write(&path, text)
                {
                    log::warn!("could not write {}: {e}", path.display());
                }
                c
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.input.p1.up, "z");
        assert_eq!(back.net.port, 7000);
    }

    #[test]
    fn old_start_binding_becomes_s_and_start_gets_its_own_key() {
        let old = |p1: &str, p2: &str| {
            let text = format!(
                "[input.p1]\nup = \"z\"\ndown = \"s\"\nleft = \"q\"\nright = \"d\"\na = \"j\"\nb = \"k\"\nc = \"l\"\nx = \"u\"\ny = \"i\"\nz = \"o\"\nstart = \"{p1}\"\n\
                 [input.p2]\nup = \"ArrowUp\"\ndown = \"ArrowDown\"\nleft = \"ArrowLeft\"\nright = \"ArrowRight\"\na = \"1\"\nb = \"2\"\nc = \"3\"\nx = \"4\"\ny = \"5\"\nz = \"6\"\nstart = \"{p2}\"\n"
            );
            toml::from_str::<Config>(&text).unwrap().input
        };
        // untouched old defaults: the new default pair
        let mut i = old("m", "Enter");
        assert!(i.migrate_start());
        assert_eq!((i.p1.s.as_str(), i.p1.start.as_str()), ("m", "p"));
        assert_eq!((i.p2.s.as_str(), i.p2.start.as_str()), ("7", "Enter"));
        assert!(!i.migrate_start(), "idempotent");
        // a custom key stays the S button; START takes the new default...
        let mut i = old("h", "0");
        i.migrate_start();
        assert_eq!((i.p1.s.as_str(), i.p1.start.as_str()), ("h", "p"));
        assert_eq!((i.p2.s.as_str(), i.p2.start.as_str()), ("0", "Enter"));
        // ...or the next free key when the default is the old key itself
        let mut i = old("p", "Enter");
        i.migrate_start();
        assert_eq!((i.p1.s.as_str(), i.p1.start.as_str()), ("p", "m"));
        // a fresh config is left alone
        let mut d = Input::default();
        assert!(!d.migrate_start());
        let names = |k: &PlayerKeys| k.names().map(str::to_lowercase);
        let all: Vec<String> = names(&d.p1).into_iter().chain(names(&d.p2)).collect();
        let mut uniq = all.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), all.len(), "default keys never overlap: {all:?}");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let back: Config = toml::from_str("[net]\nport = 9000\n").unwrap();
        assert_eq!(back.net.port, 9000);
        assert_eq!(back.video.width, 1280);
    }

    const OFFICIAL: &str = "https://play.rugen.dev";

    #[test]
    fn server_config_migrates_to_official_plus_community() {
        // an old config: custom default, favourites incl. the official one
        let mut n: Net = toml::from_str::<Config>(
            "[net]\nserver = \"https://mine.example\"\nservers = [\"https://play.rugen.dev/\", \"10.0.0.2:8080\", \"https://MINE.example\"]\n",
        )
        .unwrap()
        .net;
        assert!(n.migrate_servers(OFFICIAL));
        assert_eq!(n.servers, vec!["https://mine.example", "10.0.0.2:8080"]);
        assert_eq!(
            n.server, "https://mine.example",
            "still the default selection"
        );
        assert!(!n.migrate_servers(OFFICIAL), "idempotent");
        // the official server as the old default: becomes the built-in default
        let mut o = Net {
            server: "HTTPS://play.rugen.dev".into(),
            servers: vec![],
            ..Net::default()
        };
        assert!(o.migrate_servers(OFFICIAL));
        assert!(o.server.is_empty() && o.servers.is_empty());
        let mut fresh = Net::default();
        assert!(!fresh.migrate_servers(OFFICIAL));
    }

    #[test]
    fn official_server_is_pinned_first_and_cannot_be_removed() {
        let mut n = Net {
            servers: vec!["https://a.example".into(), "b.example:8080".into()],
            server: "b.example:8080".into(),
            ..Net::default()
        };
        let list = n.server_list(OFFICIAL);
        assert_eq!(list.len(), 3);
        assert!(list[0].official && list[0].addr == OFFICIAL);
        assert!(list[1..].iter().all(|e| !e.official));
        assert_eq!(n.default_index(OFFICIAL), 2);
        assert!(!n.remove_server(OFFICIAL, OFFICIAL));
        assert!(!n.remove_server("https://play.rugen.dev/", OFFICIAL));
        assert!(n.remove_server("http://b.example:8080", OFFICIAL));
        assert!(
            n.server.is_empty(),
            "removed default falls back to official"
        );
        assert_eq!(n.default_index(OFFICIAL), 0);
        assert_eq!(n.server_list(OFFICIAL).len(), 2);
        assert!(Net::default().server_list(OFFICIAL)[0].official);
    }

    #[test]
    fn input_delay_auto_or_frames() {
        let c: Config = toml::from_str("[net]\ninput_delay = 3\n").unwrap();
        assert_eq!(c.net.input_delay, InputDelay(Some(3)));
        let c: Config = toml::from_str("[net]\ninput_delay = \"auto\"\n").unwrap();
        assert_eq!(c.net.input_delay, InputDelay(None));
        assert_eq!(Config::default().net.input_delay, InputDelay(None));
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("input_delay = \"auto\""), "{text}");
        let p = Net::default().delay_policy();
        assert_eq!(InputDelay(None).frames(Some(10), &p), 1);
        assert_eq!(InputDelay(None).frames(Some(180), &p), 1);
        assert_eq!(InputDelay(None).frames(Some(300), &p), 1);
        assert_eq!(InputDelay(None).frames(Some(350), &p), 3);
        assert_eq!(InputDelay(None).frames(None, &p), 2);
        assert_eq!(InputDelay(Some(5)).frames(Some(10), &p), 5);
        // an old rugen.toml: max_prediction is read and dropped
        let c: Config = toml::from_str("[net]\nmax_prediction = 8\n").unwrap();
        assert_eq!(c.net.prediction_window, 12);
        assert!(c.net.direct);
        assert!(!toml::to_string(&c).unwrap().contains("max_prediction"));
        let c: Config = toml::from_str("[net]\ndirect = false\nmin_delay = 0\n").unwrap();
        assert!(!c.net.direct);
        assert_eq!(c.net.delay_policy().delay(10), 0);
        assert_eq!(InputDelay(None).step(1, false), InputDelay(Some(0)));
        assert_eq!(InputDelay(Some(0)).step(-1, false), InputDelay(None));
        assert_eq!(InputDelay(Some(8)).step(1, true), InputDelay(None));
        assert_eq!(InputDelay(Some(2)).to_string(), "2 F");
    }
}
