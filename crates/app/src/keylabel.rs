//! The one place that turns a binding into what the UI shows: the key or
//! gamepad button a player actually presses for a MUGEN button (A B C X Y Z
//! S) or START. Move lists, the tutorial, hints and the controls page all
//! read it, so they name `J` (or the pad's `A`) rather than "A".
//!
//! Directions keep their arrows outside the controls page: `cap` gives the
//! real key for every slot, the text helpers draw directions as `↑↓←→`.

use crate::config::Input;
use crate::input::{Device, PadKind};

/// Slots in binding order: u d l r a b c x y z s start tag.
pub const SLOTS: usize = 13;
pub const START: usize = 11;
pub const TAG: usize = 12;

/// What a key cap shows: `text`, or a UI glyph name (`up`, `back`...) when
/// `glyph` is set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cap {
    pub text: String,
    pub glyph: &'static str,
}

impl Cap {
    fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            glyph: "",
        }
    }

    fn glyph(g: &'static str) -> Self {
        Self {
            text: String::new(),
            glyph: g,
        }
    }

    /// Plain-text form for fonts without the UI glyphs (arrows as `↑↓←→`).
    pub fn plain(&self) -> String {
        match self.glyph {
            "" => self.text.clone(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            "back" => "BKSP".into(),
            other => other.to_uppercase(),
        }
    }
}

/// Key cap for a config key name (`"j"`, `"ArrowUp"`, `"Space"`...):
/// arrow keys keep their arrows, letters show as letters, long names get
/// short forms.
pub fn key_cap(name: &str) -> Cap {
    match name {
        "" => Cap::text("-"),
        "ArrowUp" => Cap::glyph("up"),
        "ArrowDown" => Cap::glyph("down"),
        "ArrowLeft" => Cap::glyph("left"),
        "ArrowRight" => Cap::glyph("right"),
        "Backspace" => Cap::glyph("back"),
        "Control" => Cap::text("CTRL"),
        "Escape" => Cap::text("ESC"),
        "Delete" => Cap::text("DEL"),
        "Enter" => Cap::text("ENTER"),
        "Space" => Cap::text("SPACE"),
        "Shift" => Cap::text("SHIFT"),
        "Alt" => Cap::text("ALT"),
        "Tab" => Cap::text("TAB"),
        other => Cap::text(other.to_uppercase()),
    }
}

/// Gamepad button names per slot for a pad family (the fixed mapping of
/// `input::pad_slot`: South A, East B, RT2 C, West X, North Y, RB Z, LB S,
/// Start, LT2 TAG).
pub fn pad_cap(kind: PadKind, slot: usize) -> Cap {
    const DIRS: [&str; 4] = ["up", "down", "left", "right"];
    if slot < 4 {
        return Cap::glyph(DIRS[slot]);
    }
    let names: [&str; 9] = match kind {
        PadKind::PlayStation => ["X", "O", "R2", "SQ", "TRI", "R1", "L1", "OPT", "L2"],
        // Nintendo prints A on the east face, B on the south one
        PadKind::Nintendo => ["B", "A", "ZR", "Y", "X", "R", "L", "+", "ZL"],
        PadKind::Xbox => ["A", "B", "RT", "X", "Y", "RB", "LB", "MENU", "LT"],
        PadKind::Generic => ["A", "B", "RT", "X", "Y", "RB", "LB", "START", "LT"],
        PadKind::SteamDeck => ["A", "B", "R2", "X", "Y", "R1", "L1", "MENU", "L2"],
    };
    Cap::text(names[(slot - 4).min(8)])
}

/// What `player` presses for `slot` on `device`.
pub fn cap(input: &Input, player: usize, slot: usize, device: Device) -> Cap {
    match device {
        Device::Pad(kind) => pad_cap(kind, slot),
        Device::Keyboard => {
            let keys = if player == 0 { &input.p1 } else { &input.p2 };
            key_cap(keys.names()[slot.min(SLOTS - 1)])
        }
    }
}

/// Slot of a MUGEN button letter (`'a'`..`'z'`, `'s'`), case-insensitive.
pub fn button_slot(name: &str) -> Option<usize> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "a" => 4,
        "b" => 5,
        "c" => 6,
        "x" => 7,
        "y" => 8,
        "z" => 9,
        "s" => 10,
        "start" => START,
        _ => return None,
    })
}

/// One player's labels, taken once for a screen or a move list.
#[derive(Clone, Debug)]
pub struct Labels {
    caps: [Cap; SLOTS],
}

impl Labels {
    pub fn new(input: &Input, player: usize, device: Device) -> Self {
        Self {
            caps: std::array::from_fn(|i| cap(input, player, i, device)),
        }
    }

    pub fn cap(&self, slot: usize) -> &Cap {
        &self.caps[slot.min(SLOTS - 1)]
    }

    /// Text for a slot; directions always as arrows.
    pub fn text(&self, slot: usize) -> String {
        const ARROWS: [&str; 4] = ["↑", "↓", "←", "→"];
        if slot < 4 {
            return ARROWS[slot].into();
        }
        self.cap(slot).plain()
    }

    /// A MUGEN button name (`A`, `s`, `START`) → the bound key text; other
    /// text passes through.
    pub fn button(&self, name: &str) -> String {
        match button_slot(name) {
            Some(i) => self.text(i),
            None => name.to_string(),
        }
    }

    pub fn start(&self) -> String {
        self.text(START)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_name_the_bound_key() {
        let input = Input::default();
        let p1 = Labels::new(&input, 0, Device::Keyboard);
        // AZERTY P1: A is J, S is M, START is P, directions keep arrows
        assert_eq!(p1.button("A"), "J");
        assert_eq!(p1.button("s"), "M");
        assert_eq!(p1.start(), "P");
        assert_eq!(p1.text(0), "↑", "up is Z but shows as an arrow");
        assert_eq!(p1.cap(0).text, "Z", "the controls page shows the key");
        assert_eq!(p1.button("HOLD"), "HOLD");
        let p2 = Labels::new(&input, 1, Device::Keyboard);
        assert_eq!(p2.cap(0).glyph, "up", "arrow keys keep the arrow glyph");
        assert_eq!(p2.start(), "ENTER");
        let pad = Labels::new(&input, 0, Device::Pad(PadKind::PlayStation));
        assert_eq!(pad.button("a"), "X");
        assert_eq!(pad.button("s"), "L1");
        let pad = Labels::new(&input, 0, Device::Pad(PadKind::Generic));
        assert_eq!(pad.start(), "START");
        assert_eq!(key_cap("Space").text, "SPACE");
        assert_eq!(key_cap("").text, "-");
    }
}
