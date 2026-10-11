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
    /// Frames of local input delay.
    pub input_delay: usize,
    pub max_prediction: usize,
    /// Peer `ip:port` used by the NETPLAY menu entry (empty = entry hidden).
    pub remote: String,
    /// Which side this machine plays in the NETPLAY menu entry: 1 or 2.
    pub side: u8,
    /// Characters and stage for the NETPLAY menu entry (ids from chars/ and stages/).
    pub p1: String,
    pub p2: String,
    pub stage: String,
    /// `rugen-relay` server `ip:port` and room code; when set, the peer address is
    /// discovered through the relay instead of `remote`.
    pub relay: String,
    pub room: String,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            width: 960,
            height: 720,
            fullscreen: false,
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
        }
    }
}

impl Default for PlayerKeys {
    fn default() -> Self {
        Self::p1()
    }
}

impl PlayerKeys {
    fn p1() -> Self {
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
    fn p2() -> Self {
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
            input_delay: 2,
            max_prediction: 8,
            remote: String::new(),
            side: 1,
            p1: "kfm".into(),
            p2: "kfm".into(),
            stage: String::new(),
            relay: String::new(),
            room: String::new(),
        }
    }
}

impl Config {
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
        assert_eq!(back.video.width, 960);
    }
}
