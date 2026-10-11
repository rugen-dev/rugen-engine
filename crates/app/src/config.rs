//! `rugen.toml`: video, input and net settings. Written with defaults on first run.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub video: Video,
    pub input: Input,
    pub net: Net,
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
            up: "ArrowUp".into(),
            down: "ArrowDown".into(),
            left: "ArrowLeft".into(),
            right: "ArrowRight".into(),
            a: "a".into(),
            b: "s".into(),
            c: "d".into(),
            x: "z".into(),
            y: "x".into(),
            z: "c".into(),
            start: "Enter".into(),
        }
    }
    fn p2() -> Self {
        Self {
            up: "i".into(),
            down: "k".into(),
            left: "j".into(),
            right: "l".into(),
            a: "b".into(),
            b: "n".into(),
            c: "m".into(),
            x: "t".into(),
            y: "y".into(),
            z: "u".into(),
            start: "Shift".into(),
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
        assert_eq!(back.input.p1.up, "ArrowUp");
        assert_eq!(back.net.port, 7000);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let back: Config = toml::from_str("[net]\nport = 9000\n").unwrap();
        assert_eq!(back.net.port, 9000);
        assert_eq!(back.video.width, 960);
    }
}
