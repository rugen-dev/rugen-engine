//! Keyboard + gamepad → per-player [`Buttons`].

use rugen_sim::Buttons;
use winit::keyboard::{Key, NamedKey};

use crate::config;

/// Parse a config key name into a winit logical key.
pub fn parse_key(name: &str) -> Key {
    let named = match name {
        "ArrowUp" => Some(NamedKey::ArrowUp),
        "ArrowDown" => Some(NamedKey::ArrowDown),
        "ArrowLeft" => Some(NamedKey::ArrowLeft),
        "ArrowRight" => Some(NamedKey::ArrowRight),
        "Enter" => Some(NamedKey::Enter),
        "Space" => Some(NamedKey::Space),
        "Shift" => Some(NamedKey::Shift),
        "Control" => Some(NamedKey::Control),
        "Alt" => Some(NamedKey::Alt),
        "Tab" => Some(NamedKey::Tab),
        "Backspace" => Some(NamedKey::Backspace),
        "Escape" => Some(NamedKey::Escape),
        "Delete" => Some(NamedKey::Delete),
        "F1" => Some(NamedKey::F1),
        "F2" => Some(NamedKey::F2),
        "F5" => Some(NamedKey::F5),
        "Home" => Some(NamedKey::Home),
        "End" => Some(NamedKey::End),
        "PageUp" => Some(NamedKey::PageUp),
        "PageDown" => Some(NamedKey::PageDown),
        "F11" => Some(NamedKey::F11),
        _ => None,
    };
    match named {
        Some(n) => Key::Named(n),
        None => Key::Character(name.to_lowercase().into()),
    }
}

/// Config name for a logical key (inverse of `parse_key`), None if not bindable.
pub fn key_name(key: &Key) -> Option<String> {
    Some(match key {
        Key::Named(n) => match n {
            NamedKey::ArrowUp => "ArrowUp",
            NamedKey::ArrowDown => "ArrowDown",
            NamedKey::ArrowLeft => "ArrowLeft",
            NamedKey::ArrowRight => "ArrowRight",
            NamedKey::Enter => "Enter",
            NamedKey::Space => "Space",
            NamedKey::Shift => "Shift",
            NamedKey::Control => "Control",
            NamedKey::Alt => "Alt",
            NamedKey::Tab => "Tab",
            NamedKey::Backspace => "Backspace",
            _ => return None,
        }
        .to_string(),
        Key::Character(c) => {
            let c = c.to_lowercase();
            if c.chars().count() == 1 {
                c
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

/// Bit of START in the raw per-player masks (past the 11 [`Buttons`] bits,
/// which `Buttons::from_bits` drops: START never reaches the simulation).
pub const START_BIT: u16 = 1 << 11;

/// The device a player last used: hints show its keys or buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    #[default]
    Keyboard,
    Pad(PadKind),
}

/// Button naming of a gamepad family (gilrs reports positions: South, East...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadKind {
    #[default]
    Generic,
    Xbox,
    PlayStation,
    Nintendo,
}

impl PadKind {
    /// Guess the family from the name the driver reports.
    pub fn from_name(name: &str) -> Self {
        let n = name.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|w| n.contains(w));
        if any(&[
            "playstation",
            "dualshock",
            "dualsense",
            "ps3",
            "ps4",
            "ps5",
            "sony",
        ]) {
            Self::PlayStation
        } else if any(&["nintendo", "switch", "joy-con", "pro controller"]) {
            Self::Nintendo
        } else if any(&["xbox", "x-box", "xinput", "microsoft"]) {
            Self::Xbox
        } else {
            Self::Generic
        }
    }
}

struct Pad {
    id: gilrs::GamepadId,
    /// Raw mask: Buttons bits + [`START_BIT`].
    held: u16,
    /// Buttons pressed since the last tick (kept even if already released).
    taps: u16,
    stick: [f32; 2],
    kind: PadKind,
}

impl Pad {
    fn new(id: gilrs::GamepadId, kind: PadKind) -> Self {
        Self {
            id,
            held: 0,
            taps: 0,
            stick: [0.0; 2],
            kind,
        }
    }
}

/// Gamepad button → slot (order u d l r a b c x y z s start tag), the fixed
/// mapping the controls page shows.
pub fn pad_slot(b: gilrs::Button) -> Option<usize> {
    use gilrs::Button;
    Some(match b {
        Button::DPadUp => 0,
        Button::DPadDown => 1,
        Button::DPadLeft => 2,
        Button::DPadRight => 3,
        Button::South => 4,
        Button::East => 5,
        Button::RightTrigger2 => 6,
        Button::West => 7,
        Button::North => 8,
        Button::RightTrigger => 9,
        Button::LeftTrigger => 10,
        Button::Start => 11,
        Button::LeftTrigger2 => 12,
        _ => return None,
    })
}

pub struct InputState {
    /// Per player: 12 keys (u d l r a b c x y z s start).
    keys: [[Key; 13]; 2],
    /// Raw masks: Buttons bits + [`START_BIT`].
    keyboard: [u16; 2],
    /// Keys pressed since the last tick: a tap shorter than a tick (or one that
    /// lands while a slow frame blocks the loop) still reaches the game once.
    taps: [u16; 2],
    gilrs: Option<gilrs::Gilrs>,
    pads: Vec<Pad>,
    pad_slot: [i32; 2],
    /// Player whose gamepad pressed Select/Back (the pad's pause button).
    pad_pause: Option<usize>,
    deadzone: f32,
    /// What each player pressed last: keyboard or its gamepad.
    device: [Device; 2],
}

impl InputState {
    pub fn new(cfg: &config::Input) -> Self {
        let map = |k: &config::PlayerKeys| -> [Key; 13] {
            let n = k.names();
            std::array::from_fn(|i| parse_key(n[i]))
        };
        let gilrs = match gilrs::Gilrs::new() {
            Ok(g) => Some(g),
            Err(e) => {
                log::warn!("gamepads unavailable: {e}");
                None
            }
        };
        let mut s = Self {
            keys: [map(&cfg.p1), map(&cfg.p2)],
            keyboard: [0; 2],
            taps: [0; 2],
            gilrs,
            pads: Vec::new(),
            pad_slot: [cfg.gamepad_p1, cfg.gamepad_p2],
            pad_pause: None,
            deadzone: cfg.deadzone.clamp(0.1, 0.95),
            device: [Device::Keyboard; 2],
        };
        if let Some(g) = &s.gilrs {
            let found: Vec<(gilrs::GamepadId, PadKind)> = g
                .gamepads()
                .map(|(id, gp)| (id, PadKind::from_name(gp.name())))
                .collect();
            for (id, kind) in found {
                s.pads.push(Pad::new(id, kind));
            }
            log::info!("{} gamepad(s) connected", s.pads.len());
        }
        s
    }

    /// Replace the keyboard mapping (after rebinding in the options screen).
    pub fn rebind(&mut self, cfg: &config::Input) {
        let map = |k: &config::PlayerKeys| -> [Key; 13] {
            let n = k.names();
            std::array::from_fn(|i| parse_key(n[i]))
        };
        self.keys = [map(&cfg.p1), map(&cfg.p2)];
        self.release_all();
        self.pad_slot = [cfg.gamepad_p1, cfg.gamepad_p2];
    }

    /// Keyboard event. Returns true if the key belonged to a player mapping.
    pub fn key(&mut self, key: &Key, pressed: bool) -> bool {
        let mut used = false;
        for p in 0..2 {
            for (i, k) in self.keys[p].iter().enumerate() {
                if k == key {
                    set(&mut self.keyboard[p], i, pressed);
                    if pressed {
                        set(&mut self.taps[p], i, true);
                        self.device[p] = Device::Keyboard;
                    }
                    used = true;
                }
            }
        }
        used
    }

    /// Forget every held key (the window lost focus: its releases will not come).
    pub fn release_all(&mut self) {
        self.keyboard = [0; 2];
        self.taps = [0; 2];
    }

    /// The tick consumed this tick's taps.
    pub fn end_tick(&mut self) {
        self.taps = [0; 2];
        for pad in &mut self.pads {
            pad.taps = 0;
        }
    }

    /// True when a player mapping uses this key.
    pub fn bound(&self, key: &Key) -> bool {
        self.keys.iter().flatten().any(|k| k == key)
    }

    /// Raw masks (Buttons bits + [`START_BIT`]) held on gamepads no player
    /// slot claims (they still drive menus).
    pub fn spare_pads(&self) -> u16 {
        let mut out = 0;
        for (i, pad) in self.pads.iter().enumerate() {
            if self.pad_slot.contains(&(i as i32)) {
                continue;
            }
            out |= self.pad_buttons(pad);
        }
        out
    }

    /// The device each player used last.
    pub fn devices(&self) -> [Device; 2] {
        self.device
    }

    /// Drain gamepad events.
    pub fn poll(&mut self) {
        let Some(g) = self.gilrs.as_mut() else { return };
        while let Some(ev) = g.next_event() {
            let idx = match self.pads.iter().position(|p| p.id == ev.id) {
                Some(i) => i,
                None => {
                    let kind = PadKind::from_name(g.gamepad(ev.id).name());
                    self.pads.push(Pad::new(ev.id, kind));
                    self.pads.len() - 1
                }
            };
            let owner = self.pad_slot.iter().position(|&slot| slot == idx as i32);
            let pad = &mut self.pads[idx];
            use gilrs::{Axis, Button, EventType};
            match ev.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _) => {
                    let pressed = matches!(ev.event, EventType::ButtonPressed(..));
                    if let Some(s) = pad_slot(b) {
                        set(&mut pad.held, s, pressed);
                        if pressed {
                            set(&mut pad.taps, s, true);
                        }
                    }
                    if pressed && let Some(p) = owner {
                        self.device[p] = Device::Pad(pad.kind);
                    }
                    if pressed && matches!(b, Button::Select | Button::Mode) {
                        self.pad_pause = owner;
                    }
                }
                EventType::AxisChanged(a, v, _) => match a {
                    Axis::LeftStickX => pad.stick[0] = v,
                    Axis::LeftStickY => pad.stick[1] = v,
                    _ => {}
                },
                EventType::Disconnected => {
                    pad.held = 0;
                    pad.taps = 0;
                    pad.stick = [0.0; 2];
                    if let Some(p) = owner {
                        self.device[p] = Device::Keyboard;
                    }
                }
                _ => {}
            }
        }
    }

    /// Take a pending pad pause request: the player whose Select/Back was pressed.
    pub fn take_pause(&mut self) -> Option<usize> {
        self.pad_pause.take()
    }

    /// Connected gamepads in slot order: name, family and held raw mask
    /// (stick included, [`START_BIT`] for Start).
    pub fn pads(&self) -> Vec<(String, PadKind, u16)> {
        let Some(g) = self.gilrs.as_ref() else {
            return Vec::new();
        };
        self.pads
            .iter()
            .filter_map(|p| {
                let gp = g.connected_gamepad(p.id)?;
                Some((gp.name().to_string(), p.kind, self.stick_mask(p, p.held)))
            })
            .collect()
    }

    fn stick_mask(&self, pad: &Pad, mut b: u16) -> u16 {
        let dz = self.deadzone;
        for (bit, on) in [
            (2, pad.stick[0] < -dz),
            (3, pad.stick[0] > dz),
            (0, pad.stick[1] > dz),
            (1, pad.stick[1] < -dz),
        ] {
            if on {
                b |= 1 << bit;
            }
        }
        b
    }

    /// A pad's held buttons, its stick past the deadzone and this tick's taps.
    fn pad_buttons(&self, pad: &Pad) -> u16 {
        self.stick_mask(pad, pad.held | pad.taps)
    }

    /// Raw masks for both players (keyboard OR gamepad, Buttons bits +
    /// [`START_BIT`]), including keys tapped and released since the last tick.
    pub fn raw(&self) -> [u16; 2] {
        let mut out = [0, 1].map(|p| self.keyboard[p] | self.taps[p]);
        for (p, o) in out.iter_mut().enumerate() {
            let slot = self.pad_slot[p];
            if slot < 0 {
                continue;
            }
            if let Some(pad) = self.pads.get(slot as usize) {
                *o |= self.pad_buttons(pad);
            }
        }
        out
    }

    /// Current fighter buttons for both players (START left out).
    #[cfg(test)]
    pub fn buttons(&self) -> [Buttons; 2] {
        self.raw().map(Buttons::from_bits)
    }
}

/// Netplay: this machine plays one fighter, on whichever seat the server
/// (or `--p2`) gave it, so both players' controls (P1 / P2 keyboard
/// layouts, both gamepad slots) drive it. Without this, the P2 seat only
/// listened to the P2 bindings: a player on its usual P1 keys or its only
/// gamepad lost buttons it had no P2 binding for (S: power charge).
pub fn seat_inputs(now: [Buttons; 2]) -> [Buttons; 2] {
    let merged = Buttons::from_bits(now[0].bits() | now[1].bits());
    [merged; 2]
}

/// Set bit `i` (order u d l r a b c x y z s start) of a raw mask.
fn set(b: &mut u16, i: usize, v: bool) {
    if v {
        *b |= 1 << i;
    } else {
        *b &= !(1 << i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_between_two_ticks_still_counts_once() {
        // default bindings: P2 moves with the arrows
        let mut s = InputState::new(&config::Input::default());
        let right = Key::Named(NamedKey::ArrowRight);
        assert!(s.bound(&right));
        // pressed and released before the tick samples the state
        s.key(&right, true);
        s.key(&right, false);
        assert!(s.buttons()[1].r, "the tap is seen");
        s.end_tick();
        assert!(!s.buttons()[1].r, "and only once");
        // a held key stays held across ticks
        s.key(&right, true);
        s.end_tick();
        assert!(s.buttons()[1].r);
        // focus loss forgets it (its release never comes)
        s.release_all();
        assert!(!s.buttons()[1].r);
    }

    #[test]
    fn start_is_apart_from_the_s_button() {
        let mut s = InputState::new(&config::Input::default());
        // P1 START (`p`): raw START bit, never a fighter button
        s.key(&Key::Character("p".into()), true);
        assert_eq!(s.raw()[0], START_BIT);
        assert_eq!(s.buttons()[0].bits(), 0, "START never reaches the fight");
        s.key(&Key::Character("p".into()), false);
        s.end_tick();
        // P1 S (`m`): the fighter's s button
        s.key(&Key::Character("m".into()), true);
        assert!(s.buttons()[0].s);
        assert_eq!(s.raw()[0] & START_BIT, 0);
    }

    #[test]
    fn a_netplay_seat_hears_every_local_control() {
        // default bindings: P1 S is `m`, P2 S is `7`
        let mut s = InputState::new(&config::Input::default());
        for (key, name) in [
            (Key::Character("m".into()), "P1 s"),
            (Key::Character("7".into()), "P2 s"),
        ] {
            s.key(&key, true);
            let seats = seat_inputs(s.buttons());
            assert!(seats[0].s && seats[1].s, "{name} reaches both seats");
            s.key(&key, false);
            s.end_tick();
        }
        // every fighter button of the P1 layout reaches the P2 seat
        let p1 = config::PlayerKeys::p1();
        for (i, name) in p1.names()[..11].iter().enumerate() {
            let key = parse_key(name);
            s.key(&key, true);
            let seat2 = seat_inputs(s.buttons())[1];
            assert_eq!(seat2.bits(), 1 << i, "{name}");
            s.key(&key, false);
            s.end_tick();
        }
    }
}
