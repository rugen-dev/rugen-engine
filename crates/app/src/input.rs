//! Keyboard + gamepad → per-player [`Buttons`].

use rugen_sim::Buttons;
use winit::keyboard::{Key, NamedKey};

use crate::config;

/// Parse a config key name into a winit logical key.
fn parse_key(name: &str) -> Key {
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
    stick: [f32; 2],
}

pub struct InputState {
    /// Per player: 11 keys in Buttons order.
    keys: [[Key; 11]; 2],
    keyboard: [Buttons; 2],
    gilrs: Option<gilrs::Gilrs>,
    pads: Vec<Pad>,
    pad_slot: [i32; 2],
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
            gilrs,
            pads: Vec::new(),
            pad_slot: [cfg.gamepad_p1, cfg.gamepad_p2],
            deadzone: cfg.deadzone.clamp(0.1, 0.95),
        };
        if let Some(g) = &s.gilrs {
            let ids: Vec<gilrs::GamepadId> = g.gamepads().map(|(id, _)| id).collect();
            for id in ids {
                s.pads.push(Pad {
                    id,
                    held: Buttons::default(),
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
        self.keyboard = [Buttons::default(); 2];
        self.pad_slot = [cfg.gamepad_p1, cfg.gamepad_p2];
    }

    /// Keyboard event. Returns true if the key belonged to a player mapping.
    pub fn key(&mut self, key: &Key, pressed: bool) -> bool {
        let mut used = false;
        for p in 0..2 {
            for (i, k) in self.keys[p].iter().enumerate() {
                if k == key {
                    set(&mut self.keyboard[p], i, pressed);
                    used = true;
                }
            }
        }
        used
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
                    }
                }
                EventType::AxisChanged(a, v, _) => match a {
                    Axis::LeftStickX => pad.stick[0] = v,
                    Axis::LeftStickY => pad.stick[1] = v,
                    _ => {}
                },
                EventType::Disconnected => {
                    pad.held = Buttons::default();
                    pad.stick = [0.0; 2];
                }
                _ => {}
            }
        }
    }

    /// Current buttons for both players (keyboard OR gamepad).
    pub fn buttons(&self) -> [Buttons; 2] {
        let mut out = self.keyboard;
        for (p, o) in out.iter_mut().enumerate() {
            let slot = self.pad_slot[p];
            if slot < 0 {
                continue;
            }
            let Some(pad) = self.pads.get(slot as usize) else {
                continue;
            };
            let mut b = pad.held;
            let dz = self.deadzone;
            b.l |= pad.stick[0] < -dz;
            b.r |= pad.stick[0] > dz;
            b.u |= pad.stick[1] > dz;
            b.d |= pad.stick[1] < -dz;
            *o = Buttons::from_bits(o.bits() | b.bits());
        }
        out
    }
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
