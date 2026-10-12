//! `rugen.toml`: video, input and net settings. Written with defaults on first run.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub video: Video,
    pub camera: CameraCfg,
    pub audio: AudioCfg,
    pub input: Input,
    pub net: Net,
    pub game: GameCfg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Seconds on the round clock in offline matches; 0 = no timer.
    pub round_time: i32,
}

impl Default for GameCfg {
    fn default() -> Self {
        Self {
            ai_level: 4,
            air_jumps: 0,
            air_dash: false,
            fast_fall: false,
            round_time: 99,
        }
    }
}

/// Camera limits applied on top of every stage: players can separate until the
/// view has pulled back to `zoom_out`, and the view closes in up to `zoom_in`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraCfg {
    /// Smallest scale (1 = never pull back, 0.5 = twice the width visible).
    pub zoom_out: f32,
    /// Largest scale when players are close (1 = never close in).
    pub zoom_in: f32,
}

impl Default for CameraCfg {
    fn default() -> Self {
        Self {
            zoom_out: 0.6,
            zoom_in: 1.25,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    /// UI theme: a folder under `motif/themes/` (`ink`, `dojo`, ...) or `classic` for the flat `motif/`.
    pub theme: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub start: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Net {
    /// Local UDP port for `--host` / `--join`.
    pub port: u16,
    /// Frames of local input delay: `"auto"` (picked from the measured ping
    /// when the match starts) or a number of frames.
    pub input_delay: InputDelay,
    pub max_prediction: usize,
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
    /// Default `rugen-server` (`https://rugen.example.com` or `host:port`)
    /// for the ONLINE menu. All online traffic goes through it.
    pub server: String,
    /// Favourite servers shown in the ONLINE server list.
    pub servers: Vec<String>,
    /// Skip the UDP fast path and relay over the WebSocket only.
    pub websocket_only: bool,
}

impl Net {
    /// The server list: `server` first, then the favourites, no duplicates.
    pub fn server_list(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for s in std::iter::once(&self.server).chain(&self.servers) {
            let s = s.trim();
            if !s.is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(s)) {
                v.push(s.to_string());
            }
        }
        v
    }
}

/// Estimated ping limit for `good_connections_only`.
pub const GOOD_PING_MS: u32 = 80;

/// `input_delay = "auto"` or `input_delay = 2` in rugen.toml.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InputDelay(pub Option<usize>);

impl InputDelay {
    pub const MAX: usize = 8;

    /// Frames for a measured round trip (auto) or the fixed setting.
    pub fn frames(self, rtt_ms: Option<u32>) -> usize {
        match (self.0, rtt_ms) {
            (Some(n), _) => n,
            (None, Some(rtt)) => rugen_net::auto_input_delay(rtt),
            // auto without a measurement (relayed path): a safe middle
            (None, None) => 2,
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
            start: "m".into(),
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
            start: "Enter".into(),
        }
    }
    /// Set the key for the button at `i` (Buttons order: u d l r a b c x y z s).
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
            _ => &mut self.start,
        };
        *slot = name.to_string();
    }

    /// Mapping in Buttons order: u d l r a b c x y z s.
    pub fn names(&self) -> [&str; 11] {
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
            &self.start,
        ]
    }
}

impl Default for Net {
    fn default() -> Self {
        Self {
            port: 7000,
            input_delay: InputDelay(None),
            max_prediction: 8,
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
                Ok(c) => c,
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
    fn partial_file_fills_defaults() {
        let back: Config = toml::from_str("[net]\nport = 9000\n").unwrap();
        assert_eq!(back.net.port, 9000);
        assert_eq!(back.video.width, 1280);
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
        assert_eq!(InputDelay(None).frames(Some(10)), 0);
        assert_eq!(InputDelay(None).frames(Some(100)), 2);
        assert_eq!(InputDelay(None).frames(Some(150)), 3);
        assert_eq!(InputDelay(Some(5)).frames(Some(10)), 5);
        assert_eq!(InputDelay(None).step(1, false), InputDelay(Some(0)));
        assert_eq!(InputDelay(Some(0)).step(-1, false), InputDelay(None));
        assert_eq!(InputDelay(Some(8)).step(1, true), InputDelay(None));
        assert_eq!(InputDelay(Some(2)).to_string(), "2 F");
    }
}
