//! Settings screen: volumes, camera limits, display, AI, net delay and key
//! rebinding. Writes `rugen.toml` when leaving.

use rugen_render::{Camera, Renderer};
use rugen_sim::Buttons;
use winit::keyboard::Key;

use crate::config::Config;
use crate::game::Transition;
use crate::stage::StageVisual;
use crate::system::SystemData;

const BUTTON_NAMES: [&str; 11] = [
    "UP", "DOWN", "LEFT", "RIGHT", "A", "B", "C", "X", "Y", "Z", "START",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Master,
    Music,
    ZoomOut,
    ZoomIn,
    Fullscreen,
    AiLevel,
    InputDelay,
    Theme,
    KeysP1,
    KeysP2,
    Back,
}

const ITEMS: [Item; 11] = [
    Item::Master,
    Item::Music,
    Item::ZoomOut,
    Item::ZoomIn,
    Item::Fullscreen,
    Item::AiLevel,
    Item::InputDelay,
    Item::Theme,
    Item::KeysP1,
    Item::KeysP2,
    Item::Back,
];

pub struct OptionsScreen {
    pub config: Config,
    cursor: usize,
    bg: Option<StageVisual>,
    /// Rebinding (player, next button index).
    capture: Option<(usize, usize)>,
    pub dirty: bool,
    themes: Vec<String>,
}

impl OptionsScreen {
    pub fn new(config: Config, sys: &SystemData, themes: Vec<String>) -> Self {
        let bg = StageVisual::load_prefixed(&sys.def_path, "optionbg", Some(&sys.sff_path))
            .or_else(|_| StageVisual::load_prefixed(&sys.def_path, "titlebg", Some(&sys.sff_path)))
            .ok();
        Self {
            config,
            cursor: 0,
            bg,
            capture: None,
            dirty: false,
            themes,
        }
    }

    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Raw key while rebinding. Returns true when consumed.
    pub fn raw_key(&mut self, key: &Key) -> bool {
        let Some((player, idx)) = self.capture else {
            return false;
        };
        if *key == Key::Named(winit::keyboard::NamedKey::Escape) {
            self.capture = None;
            return true;
        }
        let Some(name) = crate::input::key_name(key) else {
            return true;
        };
        let keys = if player == 0 {
            &mut self.config.input.p1
        } else {
            &mut self.config.input.p2
        };
        keys.set(idx, &name);
        self.dirty = true;
        self.capture = if idx + 1 < 11 {
            Some((player, idx + 1))
        } else {
            None
        };
        true
    }

    fn adjust(&mut self, item: Item, dir: i32) {
        let c = &mut self.config;
        let step = dir as f32;
        match item {
            Item::Master => c.audio.master = (c.audio.master + 0.1 * step).clamp(0.0, 1.0),
            Item::Music => c.audio.music = (c.audio.music + 0.1 * step).clamp(0.0, 1.0),
            Item::ZoomOut => c.camera.zoom_out = (c.camera.zoom_out + 0.05 * step).clamp(0.25, 1.0),
            Item::ZoomIn => c.camera.zoom_in = (c.camera.zoom_in + 0.05 * step).clamp(0.6, 1.6),
            Item::Fullscreen => c.video.fullscreen = !c.video.fullscreen,
            Item::AiLevel => c.game.ai_level = (c.game.ai_level + dir).clamp(1, 8),
            Item::InputDelay => c.net.input_delay = c.net.input_delay.step(dir, false),
            Item::Theme => {
                if self.themes.is_empty() {
                    return;
                }
                let cur = self
                    .themes
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case(c.video.theme.trim()))
                    .unwrap_or(0) as i32;
                let n = self.themes.len() as i32;
                c.video.theme = self.themes[(cur + dir).rem_euclid(n) as usize].clone();
            }
            Item::KeysP1 | Item::KeysP2 | Item::Back => return,
        }
        self.dirty = true;
    }

    fn value(&self, item: Item) -> String {
        let c = &self.config;
        match item {
            Item::Master => format!("{}%", libm::roundf(c.audio.master * 100.0)),
            Item::Music => format!("{}%", libm::roundf(c.audio.music * 100.0)),
            Item::ZoomOut => format!("{:.2}", c.camera.zoom_out),
            Item::ZoomIn => format!("{:.2}", c.camera.zoom_in),
            Item::Fullscreen => if c.video.fullscreen { "ON" } else { "OFF" }.into(),
            Item::AiLevel => c.game.ai_level.to_string(),
            Item::InputDelay => c.net.input_delay.to_string(),
            Item::Theme => c.video.theme.trim().to_uppercase(),
            Item::KeysP1 | Item::KeysP2 | Item::Back => String::new(),
        }
    }

    fn label(item: Item) -> &'static str {
        match item {
            Item::Master => "MASTER VOLUME",
            Item::Music => "MUSIC VOLUME",
            Item::ZoomOut => "CAMERA ZOOM OUT",
            Item::ZoomIn => "CAMERA ZOOM IN",
            Item::Fullscreen => "FULLSCREEN",
            Item::AiLevel => "COMPUTER LEVEL",
            Item::InputDelay => "NET INPUT DELAY",
            Item::Theme => "UI THEME",
            Item::KeysP1 => "P1 KEYS",
            Item::KeysP2 => "P2 KEYS",
            Item::Back => "BACK",
        }
    }

    pub fn tick(
        &mut self,
        pressed: [Buttons; 2],
        sys: &SystemData,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        if let Some(b) = self.bg.as_mut() {
            b.step();
        }
        if self.capture.is_some() {
            return None;
        }
        let any = |f: fn(&Buttons) -> bool| pressed.iter().any(f);
        let n = ITEMS.len();
        if any(|b| b.u) {
            self.cursor = (self.cursor + n - 1) % n;
            sfx(sys.title.move_snd);
        }
        if any(|b| b.d) {
            self.cursor = (self.cursor + 1) % n;
            sfx(sys.title.move_snd);
        }
        let item = ITEMS[self.cursor];
        if any(|b| b.l) {
            self.adjust(item, -1);
            sfx(sys.title.move_snd);
        }
        if any(|b| b.r) {
            self.adjust(item, 1);
            sfx(sys.title.move_snd);
        }
        if any(|b| b.a || b.b || b.c || b.x || b.y || b.z || b.s) {
            sfx(sys.title.done_snd);
            match item {
                Item::KeysP1 => self.capture = Some((0, 0)),
                Item::KeysP2 => self.capture = Some((1, 0)),
                Item::Back => return Some(Transition::Title),
                Item::Fullscreen => self.adjust(item, 1),
                _ => {}
            }
        }
        None
    }

    pub fn render(&mut self, r: &mut Renderer, sys: &mut SystemData) -> (Camera, [f64; 4]) {
        let lc = sys.localcoord;
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let sw = lc[1] * aspect;
        let clear = self
            .bg
            .as_ref()
            .map_or([0.0, 0.0, 0.0, 1.0], |b| b.clear_color);
        if let Some(b) = self.bg.as_mut() {
            b.draw(r, 0, [0.0, 0.0], 1.0, sw, Default::default());
        }
        let (item_font, active_font) = (sys.title.item_font, sys.title.active_font);
        let left = |f: Option<crate::sprites::FontRef>| {
            f.map(|mut f| {
                f.align = 1;
                f
            })
        };
        let right = |f: Option<crate::sprites::FontRef>| {
            f.map(|mut f| {
                f.align = -1;
                f
            })
        };
        sys.text(r, item_font, "OPTIONS", [lc[0] / 2.0, 24.0]);
        if let Some((player, idx)) = self.capture {
            let line = format!("P{} : PRESS KEY FOR {}", player + 1, BUTTON_NAMES[idx]);
            sys.text(r, active_font.or(item_font), &line, [lc[0] / 2.0, 110.0]);
            sys.text(r, item_font, "ESC TO CANCEL", [lc[0] / 2.0, 130.0]);
        } else {
            for (i, item) in ITEMS.iter().enumerate() {
                let y = 50.0 + i as f32 * 14.0;
                let font = if i == self.cursor {
                    active_font.or(item_font)
                } else {
                    item_font
                };
                sys.text(r, left(font), Self::label(*item), [40.0, y]);
                sys.text(r, right(font), &self.value(*item), [lc[0] - 40.0, y]);
            }
            let keys = &self.config.input;
            let show = |k: &crate::config::PlayerKeys| {
                let short = |s: &str| -> String {
                    match s {
                        "ArrowUp" => "UP".into(),
                        "ArrowDown" => "DN".into(),
                        "ArrowLeft" => "LT".into(),
                        "ArrowRight" => "RT".into(),
                        "Enter" => "ENT".into(),
                        "Space" => "SPC".into(),
                        "Shift" => "SHF".into(),
                        "Control" => "CTL".into(),
                        other => other.to_uppercase(),
                    }
                };
                let n = k.names();
                n.iter().map(|s| short(s)).collect::<Vec<_>>().join(" ")
            };
            sys.text(
                r,
                item_font,
                &format!("P1: {}", show(&keys.p1)),
                [lc[0] / 2.0, 205.0],
            );
            sys.text(
                r,
                item_font,
                &format!("P2: {}", show(&keys.p2)),
                [lc[0] / 2.0, 217.0],
            );
        }
        if let Some(b) = self.bg.as_mut() {
            b.draw(r, 1, [0.0, 0.0], 1.0, sw, Default::default());
        }
        (
            Camera {
                screen: [sw, lc[1]],
                center: [0.0, lc[1] / 2.0],
                zoom: (sw / lc[0]).min(1.0),
            },
            clear,
        )
    }
}
