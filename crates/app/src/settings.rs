//! Data behind the OPTIONS (`ui/options.toml`) and CONTROLS
//! (`ui/controls.toml`) screens: setting rows grouped in tabs, their widgets
//! and help lines, and the per-action key bindings with conflicts, presets and
//! the live test pad. The TOML only lays these out.

use std::collections::HashSet;
use std::path::Path;

use rugen_sim::Buttons;
use serde_json::{Value, json};
use winit::keyboard::{Key, NamedKey};

use crate::config::{Config, PlayerKeys};

/// `(id, tab label, subtitle)`; `controls` opens its own page.
pub const TABS: [(&str, &str, &str); 6] = [
    ("gameplay", "GAMEPLAY", "Applies to offline matches"),
    ("video", "VIDEO", "Window and display"),
    ("audio", "AUDIO", "Volume levels"),
    (
        "controls",
        "CONTROLS",
        "Keys and gamepads, on their own page",
    ),
    ("online", "ONLINE", "Netplay and the rugen server"),
    ("interface", "INTERFACE", "How the menus look"),
];

pub const CONTROLS_TAB: usize = 3;
pub const INTERFACE_TAB: usize = 5;

const ROUND_TIMES: [i32; 4] = [30, 60, 99, 0];
const RESOLUTIONS: [(u32, u32); 3] = [(1280, 720), (1600, 900), (1920, 1080)];

enum Widget {
    /// `max` squares, `on` filled.
    Pips(u32, u32),
    /// Options and the selected index.
    Choice(Vec<String>, Option<usize>),
    /// Fill fraction.
    Slider(f32),
    /// Read-only value.
    Text,
    /// Opens another page.
    Link,
}

struct Row {
    id: &'static str,
    label: &'static str,
    widget: Widget,
    /// Value text shown next to the widget (slider number, read-only value).
    text: String,
    help: &'static str,
}

fn on_off(b: bool) -> Widget {
    Widget::Choice(vec!["OFF".into(), "ON".into()], Some(usize::from(b)))
}

fn percent(v: f32) -> String {
    format!("{}%", libm::roundf(v * 100.0))
}

fn rows(tab: usize, c: &Config) -> Vec<Row> {
    let row = |id, label, widget, text: String, help| Row {
        id,
        label,
        widget,
        text,
        help,
    };
    match TABS[tab].0 {
        "gameplay" => vec![
            row(
                "ai",
                "COMPUTER LEVEL",
                Widget::Pips(8, c.game.ai_level.clamp(0, 8) as u32),
                String::new(),
                "How hard the computer fights in arcade and versus-CPU matches.",
            ),
            row(
                "round_time",
                "ROUND TIME",
                Widget::Choice(
                    vec!["30".into(), "60".into(), "99".into(), "@inf".into()],
                    ROUND_TIMES
                        .iter()
                        .position(|t| *t == c.game.round_time.max(0)),
                ),
                String::new(),
                "Seconds on the round clock. Infinite turns the timer off.",
            ),
            row(
                "air_jumps",
                "AIR JUMPS",
                Widget::Choice(
                    (0..4).map(|i| i.to_string()).collect(),
                    usize::try_from(c.game.air_jumps).ok().filter(|i| *i < 4),
                ),
                String::new(),
                "Extra jumps in mid-air for every fighter. Stages may override it.",
            ),
            row(
                "air_dash",
                "AIR DASH",
                on_off(c.game.air_dash),
                String::new(),
                "Double-tap forward or back in the air to dash.",
            ),
            row(
                "fast_fall",
                "FAST FALL",
                on_off(c.game.fast_fall),
                String::new(),
                "Hold down while falling to drop faster.",
            ),
            row(
                "zoom_out",
                "CAMERA ZOOM OUT",
                Widget::Slider((c.camera.zoom_out - 0.25) / 0.75),
                format!("{:.2}", c.camera.zoom_out),
                "How far the camera may pull back before fighters stop dragging each other. Lower = wider view.",
            ),
            row(
                "zoom_in",
                "CAMERA ZOOM IN",
                Widget::Slider(c.camera.zoom_in - 1.0),
                format!("{:.2}", c.camera.zoom_in),
                "How close the camera moves in when the fighters stand together. 1.00 = never.",
            ),
        ],
        "video" => vec![
            row(
                "fullscreen",
                "FULLSCREEN",
                on_off(c.video.fullscreen),
                String::new(),
                "Borderless fullscreen on the current monitor. F11 toggles it anywhere.",
            ),
            row(
                "resolution",
                "WINDOW SIZE",
                Widget::Choice(
                    RESOLUTIONS
                        .iter()
                        .map(|(w, h)| format!("{w}x{h}"))
                        .collect(),
                    RESOLUTIONS
                        .iter()
                        .position(|r| *r == (c.video.width, c.video.height)),
                ),
                String::new(),
                "Size of the game window. Menus stretch to any shape of window.",
            ),
            row(
                "mouse",
                "MOUSE IN MENUS",
                on_off(c.input.mouse),
                String::new(),
                "Point and click in the menus. Off hides the cursor: Esc, Enter and the arrows always work.",
            ),
        ],
        "audio" => vec![
            row(
                "master",
                "MASTER VOLUME",
                Widget::Slider(c.audio.master),
                percent(c.audio.master),
                "Overall loudness of music and sound effects.",
            ),
            row(
                "music",
                "MUSIC VOLUME",
                Widget::Slider(c.audio.music),
                percent(c.audio.music),
                "Loudness of menu and stage music, relative to the master volume.",
            ),
        ],
        "controls" => vec![
            row(
                "keys1",
                "1P BINDINGS",
                Widget::Link,
                String::new(),
                "Rebind player 1 one action at a time, test keys on the arcade pad, pick a preset.",
            ),
            row(
                "keys2",
                "2P BINDINGS",
                Widget::Link,
                String::new(),
                "Rebind player 2 one action at a time, test keys on the arcade pad, pick a preset.",
            ),
        ],
        "online" => {
            let show = |s: &str| {
                if s.trim().is_empty() {
                    "NOT SET".to_string()
                } else {
                    s.trim().to_uppercase()
                }
            };
            vec![
                row(
                    "delay",
                    "NET INPUT DELAY",
                    {
                        let (opts, sel) = c.net.input_delay.choice();
                        Widget::Choice(opts, Some(sel))
                    },
                    c.net.input_delay.to_string(),
                    "AUTO picks 0-3 frames from the measured ping. More delay = fewer rollbacks.",
                ),
                row(
                    "server",
                    "SERVER",
                    Widget::Text,
                    show(&c.net.server),
                    "rugen-server address for rooms and ranked. Set [net] server in rugen.toml.",
                ),
                row(
                    "good_conn",
                    "GOOD CONNECTIONS ONLY",
                    on_off(c.net.good_connections_only),
                    String::new(),
                    "Quick match and ranked only pair you with opponents under 80 ms.",
                ),
            ]
        }
        _ => Vec::new(),
    }
}

fn row_json(r: &Row) -> Value {
    let adjustable = matches!(
        r.widget,
        Widget::Pips(..) | Widget::Choice(..) | Widget::Slider(_)
    );
    let (kind, max, on, options, sel, frac) = match &r.widget {
        Widget::Pips(m, o) => ("pips", *m, *o, Vec::new(), -1, 0.0),
        Widget::Choice(opts, s) => (
            "choice",
            0,
            0,
            opts.clone(),
            s.map_or(-1, |i| i as i64),
            0.0,
        ),
        Widget::Slider(f) => ("slider", 0, 0, Vec::new(), -1, f.clamp(0.0, 1.0)),
        Widget::Text => ("text", 0, 0, Vec::new(), -1, 0.0),
        Widget::Link => ("link", 0, 0, Vec::new(), -1, 0.0),
    };
    // plain value for screens that only show `{item.value}`
    let value = match &r.widget {
        Widget::Pips(_, o) if r.text.is_empty() => o.to_string(),
        Widget::Choice(opts, Some(i)) => opts[*i].replace("@inf", "INF"),
        Widget::Link => ">".into(),
        _ => r.text.clone(),
    };
    let id = r.id;
    json!({
        "id": id,
        "label": r.label,
        "value": value,
        "text": r.text,
        "has_text": !r.text.is_empty(),
        "help": r.help,
        "widget": kind,
        "is_pips": kind == "pips",
        "is_choice": kind == "choice",
        "is_slider": kind == "slider",
        "is_text": kind == "text",
        "is_link": kind == "link",
        "adjustable": adjustable,
        "max": max,
        "on": on,
        "options": options,
        "sel": sel,
        "frac": frac,
        "press": if adjustable { format!("cycle:{id}") } else { format!("open:{id}") },
        "left": if adjustable { format!("adjust:{id}:-1") } else { String::new() },
        "right": if adjustable { format!("adjust:{id}:1") } else { String::new() },
    })
}

/// Display name of a theme folder.
pub fn theme_label(id: &str) -> String {
    match id {
        "ink" => "SHONEN INK".into(),
        "dojo" => "NEON DOJO".into(),
        other => other.to_uppercase(),
    }
}

/// Key cap for a config key name: `(text, glyph)`.
pub fn key_cap(name: &str) -> (String, &'static str) {
    match name {
        "" => ("-".into(), ""),
        "ArrowUp" => (String::new(), "up"),
        "ArrowDown" => (String::new(), "down"),
        "ArrowLeft" => (String::new(), "left"),
        "ArrowRight" => (String::new(), "right"),
        "Backspace" => (String::new(), "back"),
        "Control" => ("CTRL".into(), ""),
        other => (other.to_uppercase(), ""),
    }
}

/// Everything `options.toml` binds to.
pub fn options_data(
    tab: usize,
    c: &Config,
    themes: &[String],
    root: &Path,
    focused: Option<&str>,
) -> Value {
    let tab = tab.min(TABS.len() - 1);
    let tabs: Vec<Value> = TABS
        .iter()
        .enumerate()
        .map(|(i, (id, label, _))| {
            json!({
                "id": id,
                "label": label,
                "active": i == tab,
                "ext": i == CONTROLS_TAB,
                "press": format!("tab:{i}"),
            })
        })
        .collect();
    let rows = rows(tab, c);
    let theme_cards: Vec<Value> = if tab == INTERFACE_TAB {
        // bundled themes first, in house order
        let rank = |t: &str| match t {
            "ink" => 0,
            "dojo" => 1,
            _ => 2,
        };
        let mut sorted: Vec<&String> = themes.iter().filter(|t| t.as_str() != "classic").collect();
        sorted.sort_by_key(|t| rank(t));
        sorted
            .into_iter()
            .map(|t| {
                let preview = root
                    .join("motif")
                    .join("themes")
                    .join(t)
                    .join("ui")
                    .join("preview.webp");
                let has = preview.exists();
                json!({
                    "id": t,
                    "label": theme_label(t),
                    "preview": if has { preview.display().to_string() } else { String::new() },
                    "has_preview": has,
                    "active": t.eq_ignore_ascii_case(c.video.theme.trim()),
                    "press": format!("theme:{t}"),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let index = |prefix: &str| {
        focused
            .and_then(|f| f.strip_prefix(prefix))
            .and_then(|r| r.split(']').next())
            .and_then(|i| i.parse::<usize>().ok())
    };
    let help = if let Some(i) = index("rows[") {
        rows.get(i).map_or("", |r| r.help).to_string()
    } else if index("themes[").is_some() {
        "Press enter to use this theme. It switches at once.".to_string()
    } else if tab == CONTROLS_TAB {
        "Press enter to open the controls page: one row per action, presets and a live test pad."
            .to_string()
    } else {
        format!(
            "{}. Up and down pick a category, right or enter goes into it.",
            TABS[tab].2
        )
    };
    let (left, left_glyph) = key_cap(&c.input.p1.left);
    let (right, right_glyph) = key_cap(&c.input.p1.right);
    json!({
        "tabs": tabs,
        "tab": { "id": TABS[tab].0, "title": TABS[tab].1, "subtitle": TABS[tab].2, "index": tab },
        "rows": rows.iter().map(row_json).collect::<Vec<_>>(),
        "show_rows": !rows.is_empty(),
        "themes": theme_cards,
        "show_themes": tab == INTERFACE_TAB,
        "help": help,
        "keys": { "left": left, "left_glyph": left_glyph, "right": right, "right_glyph": right_glyph },
        "capture": { "active": false, "prompt": "", "progress": "" },
    })
}

fn step(v: f32, d: f32, lo: f32, hi: f32) -> f32 {
    // snap to the step grid so repeated presses never drift
    (libm::roundf((v + d) / d.abs().max(1e-6)) * d.abs()).clamp(lo, hi)
}

/// Move setting `id` by `dir`; `wrap` cycles choices past their ends (enter key).
/// Returns false for unknown / read-only rows.
pub fn adjust(c: &mut Config, id: &str, dir: i32, wrap: bool) -> bool {
    let d = dir as f32;
    let cycle = |cur: usize, n: usize| -> usize {
        let next = cur as i32 + dir;
        if wrap {
            next.rem_euclid(n as i32) as usize
        } else {
            next.clamp(0, n as i32 - 1) as usize
        }
    };
    match id {
        "master" => c.audio.master = step(c.audio.master, 0.1 * d, 0.0, 1.0),
        "music" => c.audio.music = step(c.audio.music, 0.1 * d, 0.0, 1.0),
        "zoom_out" => c.camera.zoom_out = step(c.camera.zoom_out, 0.05 * d, 0.25, 1.0),
        "zoom_in" => c.camera.zoom_in = step(c.camera.zoom_in, 0.05 * d, 1.0, 2.0),
        "fullscreen" => c.video.fullscreen = !c.video.fullscreen,
        "air_dash" => c.game.air_dash = !c.game.air_dash,
        "fast_fall" => c.game.fast_fall = !c.game.fast_fall,
        "mouse" => c.input.mouse = !c.input.mouse,
        "ai" => {
            c.game.ai_level = if wrap && c.game.ai_level + dir > 8 {
                1
            } else {
                (c.game.ai_level + dir).clamp(1, 8)
            }
        }
        "delay" => c.net.input_delay = c.net.input_delay.step(dir, wrap),
        "good_conn" => c.net.good_connections_only = !c.net.good_connections_only,
        "air_jumps" => c.game.air_jumps = cycle(c.game.air_jumps.clamp(0, 3) as usize, 4) as i32,
        "round_time" => {
            let cur = ROUND_TIMES
                .iter()
                .position(|t| *t == c.game.round_time.max(0))
                .unwrap_or(2);
            c.game.round_time = ROUND_TIMES[cycle(cur, ROUND_TIMES.len())];
        }
        "resolution" => {
            let cur = RESOLUTIONS
                .iter()
                .position(|r| *r == (c.video.width, c.video.height))
                .unwrap_or(0);
            let (w, h) = RESOLUTIONS[cycle(cur, RESOLUTIONS.len())];
            c.video.width = w;
            c.video.height = h;
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------- controls

/// `(id, label, group)` in config / Buttons order.
pub const ACTIONS: [(&str, &str, &str); 11] = [
    ("up", "UP", "MOVE"),
    ("down", "DOWN", "MOVE"),
    ("left", "LEFT", "MOVE"),
    ("right", "RIGHT", "MOVE"),
    ("a", "A", "LOW ROW"),
    ("b", "B", "LOW ROW"),
    ("c", "C", "LOW ROW"),
    ("x", "X", "TOP ROW"),
    ("y", "Y", "TOP ROW"),
    ("z", "Z", "TOP ROW"),
    ("start", "START", "MENU"),
];

/// Gamepad buttons per action (the fixed gilrs mapping in `input.rs`).
const PAD_CAPS: [(&str, &str); 11] = [
    ("", "up"),
    ("", "down"),
    ("", "left"),
    ("", "right"),
    ("A", ""),
    ("B", ""),
    ("RT", ""),
    ("X", ""),
    ("Y", ""),
    ("RB", ""),
    ("START", ""),
];

/// One-press layouts for a whole player.
pub fn presets() -> [(&'static str, PlayerKeys); 3] {
    let keys = |n: [&str; 11]| {
        let mut k = PlayerKeys::p1();
        for (i, name) in n.iter().enumerate() {
            k.set(i, name);
        }
        k
    };
    [
        ("AZERTY ZQSD", PlayerKeys::p1()),
        (
            "QWERTY WASD",
            keys(["w", "s", "a", "d", "j", "k", "l", "u", "i", "o", "Enter"]),
        ),
        ("ARROWS + NUMPAD", PlayerKeys::p2()),
    ]
}

fn same_key(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Per action of `player`: the conflict text ("ALSO B", "ALSO 2P A") if its key is used twice.
pub fn conflicts(keys: [&PlayerKeys; 2], player: usize) -> [Option<String>; 11] {
    let me = keys[player].names();
    let other = keys[1 - player].names();
    std::array::from_fn(|i| {
        let k = me[i];
        if let Some(j) = other.iter().position(|o| same_key(k, o)) {
            return Some(format!("ALSO {}P {}", 2 - player, ACTIONS[j].1));
        }
        (0..11)
            .find(|&j| j != i && same_key(k, me[j]))
            .map(|j| format!("ALSO {}", ACTIONS[j].1))
    })
}

/// The controls page: which player and device are shown, the row listening
/// for a key, keys held right now (for the test pad).
#[derive(Default)]
pub struct Controls {
    pub player: usize,
    /// 0 = keyboard, n = gamepad n-1.
    pub device: usize,
    pub listen: Option<usize>,
    held: HashSet<String>,
    /// Connected gamepads: name and held buttons.
    pub pads: Vec<(String, Buttons)>,
}

fn button(b: &Buttons, i: usize) -> bool {
    [b.u, b.d, b.l, b.r, b.a, b.b, b.c, b.x, b.y, b.z, b.s][i]
}

impl Controls {
    pub fn new(player: usize) -> Self {
        Self {
            player: player.min(1),
            ..Default::default()
        }
    }

    fn keys_mut<'a>(&self, c: &'a mut Config) -> &'a mut PlayerKeys {
        if self.player == 0 {
            &mut c.input.p1
        } else {
            &mut c.input.p2
        }
    }

    fn on_pad(&self) -> Option<&(String, Buttons)> {
        self.device.checked_sub(1).and_then(|i| self.pads.get(i))
    }

    /// Track held keys and handle the page keys (Tab, Backspace/Delete, F5).
    /// Returns true for Enter: the host presses the focused node.
    pub fn key(&mut self, c: &mut Config, key: &Key, pressed: bool, focused: Option<&str>) -> bool {
        if let Some(name) = crate::input::key_name(key) {
            if pressed {
                self.held.insert(name.to_lowercase());
            } else {
                self.held.remove(&name.to_lowercase());
            }
        }
        if !pressed || self.listen.is_some() {
            return false;
        }
        let row = focused
            .and_then(|f| f.strip_prefix("binds["))
            .and_then(|r| r.split(']').next())
            .and_then(|i| i.parse::<usize>().ok())
            .filter(|i| *i < 11);
        match key {
            Key::Named(NamedKey::Tab) => {
                self.player = 1 - self.player;
                self.listen = None;
            }
            Key::Named(NamedKey::Backspace | NamedKey::Delete) => match row {
                Some(i) if self.on_pad().is_none() => self.keys_mut(c).set(i, ""),
                _ => return false,
            },
            Key::Named(NamedKey::F5) => {
                *self.keys_mut(c) = if self.player == 0 {
                    PlayerKeys::p1()
                } else {
                    PlayerKeys::p2()
                };
            }
            Key::Named(NamedKey::Enter) => return true,
            _ => return false,
        }
        false
    }

    /// The key that ends a listen: binds it (Escape cancels). Returns true when something changed.
    pub fn bind(&mut self, c: &mut Config, key: &Key) -> bool {
        let Some(i) = self.listen else {
            return false;
        };
        if *key == Key::Named(NamedKey::Escape) {
            self.listen = None;
            return true;
        }
        let Some(name) = crate::input::key_name(key) else {
            return false;
        };
        self.keys_mut(c).set(i, &name);
        self.listen = None;
        true
    }

    /// Page actions: `player:<n>`, `device:next`, `listen:<i>`, `preset:<i>`, `defaults`.
    /// Returns true when the action was ours.
    pub fn action(&mut self, c: &mut Config, a: &str) -> bool {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match head {
            "player" => {
                self.player = arg.parse::<usize>().unwrap_or(0).min(1);
                self.listen = None;
            }
            "device" => {
                self.device = (self.device + 1) % (self.pads.len() + 1);
                self.listen = None;
            }
            "listen" => {
                if self.on_pad().is_none() {
                    let i = arg.parse::<usize>().unwrap_or(0).min(10);
                    self.listen = if self.listen == Some(i) {
                        None
                    } else {
                        Some(i)
                    };
                }
            }
            "preset" => {
                if let Some((_, k)) = presets().into_iter().nth(arg.parse().unwrap_or(0)) {
                    *self.keys_mut(c) = k;
                    self.listen = None;
                }
            }
            "defaults" => {
                *self.keys_mut(c) = if self.player == 0 {
                    PlayerKeys::p1()
                } else {
                    PlayerKeys::p2()
                };
            }
            _ => return false,
        }
        true
    }

    /// Everything `controls.toml` binds to.
    pub fn data(&self, c: &Config, ticks: u64) -> Value {
        let keys = [&c.input.p1, &c.input.p2];
        let me = keys[self.player];
        let names = me.names();
        let pad = self.on_pad();
        let conflicts = if pad.is_some() {
            Default::default()
        } else {
            conflicts(keys, self.player)
        };
        let lit: [bool; 11] = std::array::from_fn(|i| match pad {
            Some((_, b)) => button(b, i),
            None => !names[i].is_empty() && self.held.contains(&names[i].to_lowercase()),
        });
        let cap = |i: usize| -> (String, &'static str) {
            match pad {
                Some(_) => (PAD_CAPS[i].0.to_string(), PAD_CAPS[i].1),
                None => key_cap(names[i]),
            }
        };
        let binds: Vec<Value> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, (id, label, group))| {
                let listening = self.listen == Some(i);
                let (text, glyph) = cap(i);
                let conflict = conflicts[i].clone();
                let dir = i < 4;
                json!({
                    "id": id,
                    "label": label,
                    "group": if pad.is_some() { "FIXED ON PAD" } else { *group },
                    "icon": if dir { String::new() } else if *id == "start" { "S".to_string() } else { label.to_string() },
                    "icon_glyph": if dir { *id } else { "" },
                    "round": !dir && *id != "start",
                    "square": dir || *id == "start",
                    "cap": text,
                    "glyph": glyph,
                    "listening": listening,
                    "lit": lit[i] && !listening,
                    "hot": lit[i] || listening,
                    "conflict": conflict.is_some() && !listening,
                    "clean": conflict.is_none() && !listening,
                    "conflict_text": conflict.clone().unwrap_or_default(),
                    "show_group": conflict.is_none() && !listening,
                    "show_conflict": conflict.is_some() && !listening,
                    "spaced": i == 4 || i == 10,
                    "press": format!("listen:{i}"),
                })
            })
            .collect();
        let n_conf = conflicts.iter().filter(|c| c.is_some()).count();
        let mut padj = serde_json::Map::new();
        for (i, (id, label, _)) in ACTIONS.iter().enumerate() {
            let (text, glyph) = cap(i);
            padj.insert(
                id.to_string(),
                json!({ "cap": text, "glyph": glyph, "lit": lit[i], "label": label }),
            );
        }
        let dx = (i32::from(lit[3]) - i32::from(lit[2])) * 18;
        let dy = (i32::from(lit[1]) - i32::from(lit[0])) * 18;
        padj.insert("dx".into(), json!(dx));
        padj.insert("dy".into(), json!(dy));
        let presets: Vec<Value> = presets()
            .iter()
            .enumerate()
            .map(|(i, (label, k))| {
                json!({
                    "label": label,
                    "active": pad.is_none() && k.names() == me.names(),
                    "press": format!("preset:{i}"),
                })
            })
            .collect();
        let device_label = match pad {
            Some((name, _)) => {
                let mut n = name.to_uppercase();
                n.truncate(18);
                n
            }
            None => "KEYBOARD".into(),
        };
        let other = 1 - self.player;
        let players: Vec<Value> = (0..2)
            .map(|p| {
                json!({
                    "label": format!("{}P", p + 1),
                    "active": p == self.player,
                    "press": format!("player:{p}"),
                })
            })
            .collect();
        json!({
            "player": self.player,
            "players": players,
            "device": {
                "label": device_label,
                "glyph": if pad.is_some() { "pad" } else { "kbd" },
                "is_pad": pad.is_some(),
                "count": self.pads.len() + 1,
                "press": "device:next",
            },
            "binds": binds,
            "conflicts": {
                "count": n_conf,
                "any": n_conf > 0,
                "none": n_conf == 0,
                "text": if n_conf == 0 { "NO CONFLICTS".to_string() } else if n_conf == 1 { "1 CONFLICT".to_string() } else { format!("{n_conf} CONFLICTS") },
            },
            "pad": Value::Object(padj),
            "presets": presets,
            "listening": self.listen.is_some(),
            "fixed": pad.is_some(),
            "other": format!("{}P", other + 1),
            "blink": ticks / 20 % 2 == 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicts_within_and_across_players() {
        let mut c = Config::default();
        assert!(
            conflicts([&c.input.p1, &c.input.p2], 0)
                .iter()
                .all(Option::is_none)
        );
        c.input.p1.a = "k".into(); // same as B
        c.input.p1.start = "ArrowUp".into(); // 2P up
        let cf = conflicts([&c.input.p1, &c.input.p2], 0);
        assert_eq!(cf[4].as_deref(), Some("ALSO B"));
        assert_eq!(cf[5].as_deref(), Some("ALSO A"));
        assert_eq!(cf[10].as_deref(), Some("ALSO 2P UP"));
        let cf2 = conflicts([&c.input.p1, &c.input.p2], 1);
        assert_eq!(cf2[0].as_deref(), Some("ALSO 1P START"));
    }

    #[test]
    fn rebind_one_action_and_presets() {
        let mut c = Config::default();
        let mut ctl = Controls::new(0);
        assert!(ctl.action(&mut c, "listen:4"));
        assert_eq!(ctl.listen, Some(4));
        assert!(ctl.bind(&mut c, &Key::Character("p".into())));
        assert_eq!(c.input.p1.a, "p");
        assert_eq!(c.input.p1.b, "k", "only that action changes");
        assert!(ctl.listen.is_none());
        ctl.action(&mut c, "preset:1");
        assert_eq!(c.input.p1.up, "w");
        assert_eq!(c.input.p1.start, "Enter");
        ctl.key(&mut c, &Key::Named(NamedKey::F5), true, None);
        assert_eq!(c.input.p1.up, "z");
        ctl.key(
            &mut c,
            &Key::Named(NamedKey::Backspace),
            true,
            Some("binds[2].row"),
        );
        assert_eq!(c.input.p1.left, "");
        let d = ctl.data(&c, 0);
        assert_eq!(d["binds"][2]["cap"], "-");
    }

    #[test]
    fn adjust_steps_and_wraps() {
        let mut c = Config::default();
        c.camera.zoom_out = 0.6;
        adjust(&mut c, "zoom_out", 1, false);
        assert!((c.camera.zoom_out - 0.65).abs() < 1e-4);
        c.game.round_time = 99;
        adjust(&mut c, "round_time", 1, false);
        assert_eq!(c.game.round_time, 0, "99 -> infinite");
        adjust(&mut c, "round_time", 1, false);
        assert_eq!(c.game.round_time, 0, "clamped at the end");
        adjust(&mut c, "round_time", 1, true);
        assert_eq!(c.game.round_time, 30, "enter wraps");
        let d = options_data(0, &c, &[], Path::new("."), Some("rows[5].row"));
        assert_eq!(d["rows"][1]["sel"], 0);
        assert!(
            d["help"]
                .as_str()
                .unwrap()
                .starts_with("How far the camera")
        );
    }
}
