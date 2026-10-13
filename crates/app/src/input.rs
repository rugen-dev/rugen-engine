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
        "F5" => Some(NamedKey::F5),
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

struct Pad {
    id: gilrs::GamepadId,
    held: Buttons,
    /// Buttons pressed since the last tick (kept even if already released).
    taps: Buttons,
    stick: [f32; 2],
}

pub struct InputState {
    /// Per player: 11 keys in Buttons order.
    keys: [[Key; 11]; 2],
    keyboard: [Buttons; 2],
    /// Keys pressed since the last tick: a tap shorter than a tick (or one that
    /// lands while a slow frame blocks the loop) still reaches the game once.
    taps: [Buttons; 2],
    gilrs: Option<gilrs::Gilrs>,
    pads: Vec<Pad>,
    pad_slot: [i32; 2],
    /// Player whose gamepad pressed Select/Back (the pad's pause button).
    pad_pause: Option<usize>,
    deadzone: f32,
}

impl InputState {
    pub fn new(cfg: &config::Input) -> Self {
        let map = |k: &config::PlayerKeys| -> [Key; 11] {
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
            keyboard: [Buttons::default(); 2],
            taps: [Buttons::default(); 2],
            gilrs,
            pads: Vec::new(),
            pad_slot: [cfg.gamepad_p1, cfg.gamepad_p2],
            pad_pause: None,
            deadzone: cfg.deadzone.clamp(0.1, 0.95),
        };
        if let Some(g) = &s.gilrs {
            let ids: Vec<gilrs::GamepadId> = g.gamepads().map(|(id, _)| id).collect();
            for id in ids {
                s.pads.push(Pad {
                    id,
                    held: Buttons::default(),
                    taps: Buttons::default(),
                    stick: [0.0; 2],
                });
            }
            log::info!("{} gamepad(s) connected", s.pads.len());
        }
        s
    }

    /// Replace the keyboard mapping (after rebinding in the options screen).
    pub fn rebind(&mut self, cfg: &config::Input) {
        let map = |k: &config::PlayerKeys| -> [Key; 11] {
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
                    }
                    used = true;
                }
            }
        }
        used
    }

    /// Forget every held key (the window lost focus: its releases will not come).
    pub fn release_all(&mut self) {
        self.keyboard = [Buttons::default(); 2];
        self.taps = [Buttons::default(); 2];
    }

    /// The tick consumed this tick's taps.
    pub fn end_tick(&mut self) {
        self.taps = [Buttons::default(); 2];
        for pad in &mut self.pads {
            pad.taps = Buttons::default();
        }
    }

    /// True when a player mapping uses this key.
    pub fn bound(&self, key: &Key) -> bool {
        self.keys.iter().flatten().any(|k| k == key)
    }

    /// Buttons held on gamepads no player slot claims (they still drive menus).
    pub fn spare_pads(&self) -> Buttons {
        let mut out = Buttons::default();
        for (i, pad) in self.pads.iter().enumerate() {
            if self.pad_slot.contains(&(i as i32)) {
                continue;
            }
            out = Buttons::from_bits(out.bits() | self.pad_buttons(pad).bits());
        }
        out
    }

    /// Drain gamepad events.
    pub fn poll(&mut self) {
        let Some(g) = self.gilrs.as_mut() else { return };
        while let Some(ev) = g.next_event() {
            let idx = match self.pads.iter().position(|p| p.id == ev.id) {
                Some(i) => i,
                None => {
                    self.pads.push(Pad {
                        id: ev.id,
                        held: Buttons::default(),
                        taps: Buttons::default(),
                        stick: [0.0; 2],
                    });
                    self.pads.len() - 1
                }
            };
            let pad = &mut self.pads[idx];
            use gilrs::{Axis, Button, EventType};
            match ev.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _) => {
                    let pressed = matches!(ev.event, EventType::ButtonPressed(..));
                    let slot = match b {
                        Button::DPadUp => Some(0),
                        Button::DPadDown => Some(1),
                        Button::DPadLeft => Some(2),
                        Button::DPadRight => Some(3),
                        Button::South => Some(4),
                        Button::East => Some(5),
                        Button::RightTrigger2 => Some(6),
                        Button::West => Some(7),
                        Button::North => Some(8),
                        Button::RightTrigger => Some(9),
                        Button::Start => Some(10),
                        _ => None,
                    };
                    if let Some(s) = slot {
                        set(&mut pad.held, s, pressed);
                        if pressed {
                            set(&mut pad.taps, s, true);
                        }
                    }
                    if pressed && matches!(b, Button::Select | Button::Mode) {
                        let pi = idx as i32;
                        self.pad_pause = self.pad_slot.iter().position(|&slot| slot == pi);
                    }
                }
                EventType::AxisChanged(a, v, _) => match a {
                    Axis::LeftStickX => pad.stick[0] = v,
                    Axis::LeftStickY => pad.stick[1] = v,
                    _ => {}
                },
                EventType::Disconnected => {
                    pad.held = Buttons::default();
                    pad.taps = Buttons::default();
                    pad.stick = [0.0; 2];
                }
                _ => {}
            }
        }
    }

    /// Take a pending pad pause request: the player whose Select/Back was pressed.
    pub fn take_pause(&mut self) -> Option<usize> {
        self.pad_pause.take()
    }

    /// Connected gamepads in slot order: name and held buttons (stick included).
    pub fn pads(&self) -> Vec<(String, Buttons)> {
        let Some(g) = self.gilrs.as_ref() else {
            return Vec::new();
        };
        self.pads
            .iter()
            .filter_map(|p| {
                let gp = g.connected_gamepad(p.id)?;
                let mut b = p.held;
                let dz = self.deadzone;
                b.l |= p.stick[0] < -dz;
                b.r |= p.stick[0] > dz;
                b.u |= p.stick[1] > dz;
                b.d |= p.stick[1] < -dz;
                Some((gp.name().to_string(), b))
            })
            .collect()
    }

    /// A pad's held buttons, its stick past the deadzone and this tick's taps.
    fn pad_buttons(&self, pad: &Pad) -> Buttons {
        let mut b = Buttons::from_bits(pad.held.bits() | pad.taps.bits());
        let dz = self.deadzone;
        b.l |= pad.stick[0] < -dz;
        b.r |= pad.stick[0] > dz;
        b.u |= pad.stick[1] > dz;
        b.d |= pad.stick[1] < -dz;
        b
    }

    /// Current buttons for both players (keyboard OR gamepad), including keys
    /// tapped and released since the last tick.
    pub fn buttons(&self) -> [Buttons; 2] {
        let mut out =
            [0, 1].map(|p| Buttons::from_bits(self.keyboard[p].bits() | self.taps[p].bits()));
        for (p, o) in out.iter_mut().enumerate() {
            let slot = self.pad_slot[p];
            if slot < 0 {
                continue;
            }
            let Some(pad) = self.pads.get(slot as usize) else {
                continue;
            };
            *o = Buttons::from_bits(o.bits() | self.pad_buttons(pad).bits());
        }
        out
    }
}

/// Netplay: this machine plays one fighter, on whichever seat the server
/// (or `--p2`) gave it, so both players' controls (P1 / P2 keyboard
/// layouts, both gamepad slots) drive it. Without this, the P2 seat only
/// listened to the P2 bindings: a player on its usual P1 keys or its only
/// gamepad lost buttons it had no P2 binding for (Start: power charge).
pub fn seat_inputs(now: [Buttons; 2]) -> [Buttons; 2] {
    let merged = Buttons::from_bits(now[0].bits() | now[1].bits());
    [merged; 2]
}

fn set(b: &mut Buttons, i: usize, v: bool) {
    let slot = match i {
        0 => &mut b.u,
        1 => &mut b.d,
        2 => &mut b.l,
        3 => &mut b.r,
        4 => &mut b.a,
        5 => &mut b.b,
        6 => &mut b.c,
        7 => &mut b.x,
        8 => &mut b.y,
        9 => &mut b.z,
        _ => &mut b.s,
    };
    *slot = v;
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
    fn a_netplay_seat_hears_every_local_control() {
        // default bindings: P1 Start is `m`, P2 Start is Enter
        let mut s = InputState::new(&config::Input::default());
        for (key, name) in [
            (Key::Character("m".into()), "P1 start"),
            (Key::Named(NamedKey::Enter), "P2 start"),
        ] {
            s.key(&key, true);
            let seats = seat_inputs(s.buttons());
            assert!(seats[0].s && seats[1].s, "{name} reaches both seats");
            s.key(&key, false);
            s.end_tick();
        }
        // every button of the P1 layout reaches the P2 seat
        let p1 = config::PlayerKeys::p1();
        for (i, name) in p1.names().iter().enumerate() {
            let key = parse_key(name);
            s.key(&key, true);
            let seat2 = seat_inputs(s.buttons())[1];
            assert_eq!(seat2.bits(), 1 << i, "{name}");
            s.key(&key, false);
            s.end_tick();
        }
    }
}
