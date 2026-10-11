//! Title screen: system.def `[Title Info]` menu over `[TitleBG]`.

use rugen_render::{Camera, Renderer};
use rugen_sim::Buttons;

use crate::game::{Mode, Transition};
use crate::stage::StageVisual;
use crate::system::SystemData;

pub struct TitleScreen {
    cursor: usize,
    bg: Option<StageVisual>,
    /// First visible item when the menu scrolls.
    top: usize,
    netplay: bool,
}

impl TitleScreen {
    pub fn new(sys: &SystemData, netplay: bool) -> Self {
        let bg = match StageVisual::load_prefixed(&sys.def_path, "titlebg", Some(&sys.sff_path)) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("title background: {e:#}");
                None
            }
        };
        Self {
            cursor: 0,
            bg,
            top: 0,
            netplay,
        }
    }

    /// Menu entries we can actually run; NETPLAY is added before EXIT when configured.
    fn items(sys: &SystemData, netplay: bool) -> Vec<(Mode, String)> {
        let mut out: Vec<(Mode, String)> = sys
            .title
            .items
            .iter()
            .filter_map(|(k, label)| {
                let mode = match k.as_str() {
                    "arcade" => Mode::Arcade,
                    "versus" => Mode::Versus,
                    "training" => Mode::Training,
                    "watch" => Mode::Watch,
                    "options" => Mode::Options,
                    "exit" => Mode::Exit,
                    _ => return None,
                };
                Some((mode, label.clone()))
            })
            .collect();
        let after_training = out
            .iter()
            .position(|(m, _)| *m == Mode::Training)
            .map_or(out.len(), |i| i + 1);
        out.insert(after_training, (Mode::Tutorial, "TUTORIAL".into()));
        if !out.iter().any(|(m, _)| *m == Mode::Options) {
            let at = out
                .iter()
                .position(|(m, _)| *m == Mode::Exit)
                .unwrap_or(out.len());
            out.insert(at, (Mode::Options, "OPTIONS".into()));
        }
        if netplay {
            let at = out
                .iter()
                .position(|(m, _)| *m == Mode::Exit)
                .unwrap_or(out.len());
            out.insert(at, (Mode::Netplay, "NETPLAY".into()));
        }
        out
    }

    /// `pressed` = buttons that went down this tick, per player.
    pub fn tick(
        &mut self,
        pressed: [Buttons; 2],
        sys: &SystemData,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        let items = Self::items(sys, self.netplay);
        if items.is_empty() {
            return Some(Transition::Select(Mode::Versus));
        }
        if let Some(b) = self.bg.as_mut() {
            b.step();
        }
        let any = |f: fn(&Buttons) -> bool| pressed.iter().any(f);
        if any(|b| b.u) {
            self.cursor = (self.cursor + items.len() - 1) % items.len();
            sfx(sys.title.move_snd);
        }
        if any(|b| b.d) {
            self.cursor = (self.cursor + 1) % items.len();
            sfx(sys.title.move_snd);
        }
        let visible = sys.title.visible_items.max(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + visible {
            self.top = self.cursor + 1 - visible;
        }
        if any(|b| b.a || b.b || b.c || b.x || b.y || b.z || b.s) {
            sfx(sys.title.done_snd);
            return Some(match items[self.cursor].0 {
                Mode::Exit => Transition::Quit,
                Mode::Netplay => Transition::Netplay,
                Mode::Options => Transition::Options,
                m => Transition::Select(m),
            });
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
        let items = Self::items(sys, self.netplay);
        let t = &sys.title;
        let visible = t.visible_items.max(1);
        let (pos, spacing, item_font, active_font, box_cursor) = (
            t.menu_pos,
            t.spacing,
            t.item_font,
            t.active_font,
            t.box_cursor,
        );
        for (i, (_, label)) in items.iter().enumerate().skip(self.top).take(visible) {
            let row = (i - self.top) as f32;
            let p = [pos[0] + spacing[0] * row, pos[1] + spacing[1] * row];
            let font = if i == self.cursor {
                active_font.or(item_font)
            } else {
                item_font
            };
            sys.text(r, font, label, p);
            if i == self.cursor
                && let Some(c) = box_cursor
            {
                let x0 = p[0] + c[0] - lc[0] / 2.0;
                let x1 = p[0] + c[2] - lc[0] / 2.0;
                r.rect([x0, p[1] + c[1]], [x1, p[1] + c[3]], [1.0, 1.0, 1.0, 0.25]);
            }
        }
        if let Some(b) = self.bg.as_mut() {
            b.draw(r, 1, [0.0, 0.0], 1.0, sw, Default::default());
        }
        (
            Camera {
                screen: [sw, lc[1]],
                center: [0.0, lc[1] / 2.0],
                zoom: 1.0,
            },
            clear,
        )
    }
}
