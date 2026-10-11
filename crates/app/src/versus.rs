//! VS screen: both fighters' big portraits over `[VersusBG]` for `[VS Screen] time` ticks.

use rugen_render::{Camera, Draw, Renderer, TextureId};
use rugen_sim::Buttons;

use crate::game::Transition;
use crate::play::FightSetup;
use crate::stage::StageVisual;
use crate::system::SystemData;

#[derive(Debug)]
pub struct Portrait {
    pub name: String,
    pub image: Option<(u32, u32, Vec<u8>, [i32; 2])>,
    tex: Option<TextureId>,
}

impl Portrait {
    pub fn new(name: String, image: Option<(u32, u32, Vec<u8>, [i32; 2])>) -> Self {
        Self {
            name,
            image,
            tex: None,
        }
    }
}

pub struct VersusScreen {
    setup: FightSetup,
    sides: [Portrait; 2],
    bg: Option<StageVisual>,
    left: i32,
}

impl VersusScreen {
    pub fn new(setup: FightSetup, sides: [Portrait; 2], sys: &SystemData) -> Self {
        let bg = match StageVisual::load_prefixed(&sys.def_path, "versusbg", Some(&sys.sff_path)) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("versus background: {e:#}");
                None
            }
        };
        Self {
            setup,
            sides,
            bg,
            left: sys.versus.time.max(1),
        }
    }

    pub fn tick(&mut self, pressed: [Buttons; 2]) -> Option<Transition> {
        if let Some(b) = self.bg.as_mut() {
            b.step();
        }
        self.left -= 1;
        let skip = pressed
            .iter()
            .any(|b| b.a || b.b || b.c || b.x || b.y || b.z || b.s);
        if self.left <= 0 || skip {
            return Some(Transition::Fight(self.setup.clone()));
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
        let ox = -lc[0] / 2.0;
        let vs = sys.versus.clone();
        for side in 0..2 {
            let p = &mut self.sides[side];
            if p.tex.is_none()
                && let Some((pw, ph, rgba, _)) = &p.image
            {
                p.tex = Some(r.add_rgba(*pw, *ph, rgba));
            }
            if let Some(tex) = p.tex
                && let Some((_, _, _, axis)) = p.image
            {
                let mut d = Draw::new(
                    tex,
                    None,
                    [vs.face_offset[side][0] + ox, vs.face_offset[side][1]],
                    [axis[0] as f32, axis[1] as f32],
                );
                d.scale = vs.face_scale[side];
                d.hflip = vs.face_facing[side] < 0.0;
                r.draw(d);
            }
            let name = p.name.clone();
            sys.text(r, vs.name_font[side], &name, vs.name_offset[side]);
        }
        if !vs.match_text.is_empty() {
            let line = vs.match_text.replace("%i", "1");
            sys.text(r, vs.match_font, &line, vs.match_offset);
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
