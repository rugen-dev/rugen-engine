//! Visual effects that live in the simulation state: PalFX, AfterImage and
//! EnvShake (ported from Ikemen GO). They are plain data so rollback can
//! snapshot them; the app turns them into draw parameters.

use crate::explod::Trans;

/// Effective palette transform for one draw, in 0..=1 units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PalFxOut {
    pub mul: [f32; 3],
    pub add: [f32; 3],
    /// 1 = full colour, 0 = greyscale.
    pub color: f32,
    pub invert: bool,
}

impl Default for PalFxOut {
    fn default() -> Self {
        Self {
            mul: [1.0; 3],
            add: [0.0; 3],
            color: 1.0,
            invert: false,
        }
    }
}

impl PalFxOut {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// Apply `extra` on top of `self` (Ikemen `stacked`).
    pub fn stacked(self, extra: PalFxOut) -> PalFxOut {
        PalFxOut {
            mul: [
                self.mul[0] * extra.mul[0],
                self.mul[1] * extra.mul[1],
                self.mul[2] * extra.mul[2],
            ],
            add: [
                self.add[0] + extra.add[0],
                self.add[1] + extra.add[1],
                self.add[2] + extra.add[2],
            ],
            color: self.color * extra.color,
            invert: self.invert != extra.invert,
        }
    }
}

/// A running PalFX controller (player, AllPalFX or BGPalFX).
#[derive(Clone, Debug, PartialEq)]
pub struct PalFx {
    /// Ticks left; -1 = forever, 0 = off.
    pub time: i32,
    pub color: f32,
    pub add: [i32; 3],
    pub mul: [i32; 3],
    pub sinadd: [i32; 3],
    pub sinmul: [i32; 3],
    pub sincolor: i32,
    /// Periods for sinadd, sinmul, sincolor (unused slot 3 kept for parity).
    pub cycletime: [i32; 4],
    pub sintime: [i32; 4],
    pub invertall: bool,
}

impl Default for PalFx {
    fn default() -> Self {
        Self {
            time: 0,
            color: 1.0,
            add: [0; 3],
            mul: [256; 3],
            sinadd: [0; 3],
            sinmul: [0; 3],
            sincolor: 0,
            cycletime: [0; 4],
            sintime: [0; 4],
            invertall: false,
        }
    }
}

impl PalFx {
    pub fn active(&self) -> bool {
        self.time != 0
    }

    fn wave(&self, slot: usize) -> f32 {
        let ct = self.cycletime[slot];
        if ct <= 1 {
            return 0.0;
        }
        let mut st = 2.0 * std::f32::consts::PI * self.sintime[slot] as f32;
        if ct == 2 {
            st += std::f32::consts::PI / 2.0;
        }
        libm::sinf(st / ct as f32)
    }

    /// Effective values for this tick.
    pub fn sample(&self) -> PalFxOut {
        if !self.active() {
            return PalFxOut::default();
        }
        let (wa, wm, wc) = (self.wave(0), self.wave(1), self.wave(2));
        let mut out = PalFxOut::default();
        for i in 0..3 {
            let add = self.add[i] + (wa * self.sinadd[i] as f32) as i32;
            let mul = self.mul[i] + (wm * self.sinmul[i] as f32) as i32;
            out.add[i] = add as f32 / 255.0;
            out.mul[i] = mul as f32 / 256.0;
        }
        out.color = self.color + (wc * self.sincolor as f32) as i32 as f32 / 256.0;
        out.invert = self.invertall;
        out
    }

    /// Advance timers (once per tick while running).
    pub fn step(&mut self) {
        if !self.active() {
            return;
        }
        for i in 0..4 {
            if self.cycletime[i] > 0 {
                self.sintime[i] = (self.sintime[i] + 1) % self.cycletime[i];
            }
        }
        if self.time > 0 {
            self.time -= 1;
        }
    }
}

/// One recorded afterimage frame: enough to redraw the sprite as it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AfterImg {
    pub pos: [f32; 2],
    pub facing: f32,
    pub scale: [f32; 2],
    pub angle: f32,
    /// Data index whose AIR / SFF to draw from.
    pub anim_owner: usize,
    pub sprite_data: usize,
    pub action: i32,
    pub elem: usize,
    pub priority: i32,
}

pub const MAX_AIMG_LENGTH: i32 = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct AfterImage {
    pub time: i32,
    pub length: i32,
    pub postbright: [i32; 3],
    pub add: [i32; 3],
    pub mul: [f32; 3],
    pub timegap: i32,
    pub framegap: i32,
    pub trans: Trans,
    pub alpha: [i32; 2],
    /// First entry: starting palette parameters; later ones derived in `setup`.
    pub palfx: Vec<PalFxOut>,
    pub imgs: Vec<Option<AfterImg>>,
    pub imgidx: i32,
    pub restgap: i32,
    pub reccount: i32,
    pub timecount: i32,
    pub ignorehitpause: bool,
}

impl Default for AfterImage {
    fn default() -> Self {
        Self {
            time: 0,
            length: 20,
            postbright: [0; 3],
            add: [10, 10, 25],
            mul: [0.65, 0.65, 0.75],
            timegap: 1,
            framegap: 4,
            trans: Trans::Default,
            alpha: [-1, 0],
            palfx: vec![PalFxOut {
                mul: [120.0 / 256.0, 120.0 / 256.0, 220.0 / 256.0],
                add: [30.0 / 255.0; 3],
                color: 1.0,
                invert: false,
            }],
            imgs: Vec::new(),
            imgidx: 0,
            restgap: 0,
            reccount: 0,
            timecount: 0,
            ignorehitpause: true,
        }
    }
}

impl AfterImage {
    /// `palcolor`: 0..=256 saturation.
    pub fn set_pal_color(&mut self, color: i32) {
        self.palfx[0].color = color.clamp(0, 256) as f32 / 256.0;
    }
    pub fn set_pal_invertall(&mut self, inv: bool) {
        self.palfx[0].invert = inv;
    }
    pub fn set_pal_bright(&mut self, i: usize, v: i32) {
        self.palfx[0].add[i] = v as f32 / 255.0;
    }
    pub fn set_pal_contrast(&mut self, i: usize, v: i32) {
        self.palfx[0].mul[i] = v as f32 / 256.0;
    }

    /// Build the ring buffer and the per-image palette chain.
    pub fn setup(&mut self) {
        if self.length <= 0 {
            return;
        }
        // MUGEN's buffer holds exactly `length` frames.
        let ring = self.length as usize;
        self.imgs.resize(ring, None);
        self.imgidx = self.imgidx.clamp(0, ring as i32 - 1);
        let pfxsize = ((self.length / self.framegap.max(1)) as usize).max(1);
        self.palfx.truncate(1);
        let mut pb = self.postbright;
        for i in 1..pfxsize {
            let prev = self.palfx[i - 1];
            let mut next = prev;
            for (j, add) in self.add.iter().enumerate() {
                next.add[j] = prev.add[j] + (add + pb[j]) as f32 / 255.0;
                next.mul[j] = prev.mul[j] * self.mul[j];
            }
            pb = [0; 3];
            self.palfx.push(next);
        }
    }

    pub fn is_active(&self) -> bool {
        !(self.time == 0
            || (self.time > 0 && self.timecount >= self.timegap * self.length + self.time - 1)
            || self.timegap < 1
            || self.timegap > 32767
            || self.framegap < 1
            || self.framegap > 32767)
    }

    /// Record the current sprite (call once per tick while active).
    pub fn record(&mut self, img: AfterImg) {
        if self.time == 0 {
            self.reccount = 0;
            self.timegap = 0;
            return;
        }
        if self.imgs.is_empty() {
            return;
        }
        if self.restgap <= 0 {
            let mut img = img;
            img.priority -= 2;
            let idx = self.imgidx as usize % self.imgs.len();
            self.imgs[idx] = Some(img);
            self.imgidx += 1;
            if self.imgidx >= self.imgs.len() as i32 {
                self.imgidx = 0;
            }
            self.reccount += 1;
            self.restgap = self.timegap;
        }
        self.restgap -= 1;
        self.timecount += 1;
    }

    /// Images to draw this tick, oldest last: (image, palette fx, priority).
    pub fn visible(&self, cur_priority: i32) -> Vec<(AfterImg, PalFxOut, i32)> {
        let mut out = Vec::new();
        let ring = self.imgs.len() as i32;
        if ring <= 0 || self.framegap <= 0 || self.timegap <= 0 {
            return out;
        }
        // MUGEN samples one slot closer to the current frame than Ikemen.
        let usable = (self.reccount.min(self.length - 1)).max(0);
        let max_steps = usable / self.framegap;
        for step in 0..max_steps {
            let i = (step + 1) * self.framegap;
            let mut idx = (self.imgidx - i) % ring;
            if idx < 0 {
                idx += ring;
            }
            let Some(mut img) = self.imgs[idx as usize] else {
                continue;
            };
            if img.priority >= cur_priority {
                img.priority = cur_priority - 2;
            }
            if self.time < 0
                || (self.timecount / self.timegap - i) < (self.time - 2) / self.timegap + 1
            {
                let Some(pfx) = self.palfx.get(step as usize) else {
                    continue;
                };
                out.push((img, *pfx, img.priority - step));
            }
        }
        out
    }
}

/// Screen shake (EnvShake / fall.envshake). Amplitude is in stage pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvShake {
    pub time: i32,
    pub freq: f32,
    pub ampl: f32,
    /// None until defaulted from `freq` in `start`.
    pub phase: Option<f32>,
    pub mul: f32,
    pub dir: f32,
    pub cur_time: i32,
    pub offset: [f32; 2],
}

impl Default for EnvShake {
    fn default() -> Self {
        Self {
            time: 0,
            freq: 60.0,
            ampl: -4.0,
            phase: None,
            mul: 1.0,
            dir: 0.0,
            cur_time: 0,
            offset: [0.0; 2],
        }
    }
}

impl EnvShake {
    pub fn start(&mut self) {
        if self.phase.is_none() {
            self.phase = Some(if self.freq >= 90.0 { 90.0 } else { 0.0 });
        }
        if self.time <= 0 {
            *self = Self::default();
            return;
        }
        self.cur_time = self.time;
    }

    pub fn active(&self) -> bool {
        self.cur_time > 0
    }

    pub fn update(&mut self) {
        if self.cur_time <= 0 {
            if self.time > 0 {
                *self = Self::default();
            }
            return;
        }
        let elapsed = (self.time - self.cur_time) as f32;
        let phase = self.phase.unwrap_or(0.0) + self.freq * elapsed;
        let mut factor = 1.0f32;
        if self.mul != 0.0 && self.mul != 1.0 {
            let cycles = libm::floorf(self.freq * elapsed / 360.0);
            factor *= libm::powf(self.mul, cycles);
        }
        let ampl = self.ampl * factor;
        if ampl == 0.0 {
            self.offset = [0.0; 2];
        } else {
            let rad = phase * std::f32::consts::PI / 180.0;
            let dir = self.dir * std::f32::consts::PI / 180.0;
            let off = ampl * libm::sinf(rad);
            self.offset = [off * libm::sinf(-dir), off * libm::cosf(-dir)];
        }
        self.cur_time -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palfx_sin_and_timer() {
        let mut p = PalFx {
            time: 3,
            add: [10, 0, 0],
            sinadd: [100, 0, 0],
            cycletime: [4, 0, 0, 0],
            ..Default::default()
        };
        assert!(p.active());
        let s0 = p.sample();
        assert!((s0.add[0] - 10.0 / 255.0).abs() < 1e-5);
        p.step();
        let s1 = p.sample();
        assert!(s1.add[0] > s0.add[0]);
        p.step();
        p.step();
        assert!(!p.active());
        assert!(p.sample().is_identity());
    }

    #[test]
    fn afterimage_ring() {
        let mut ai = AfterImage {
            time: -1,
            length: 12,
            framegap: 3,
            ..Default::default()
        };
        ai.setup();
        assert_eq!(ai.imgs.len(), 12);
        assert_eq!(ai.palfx.len(), 4);
        let img = |x: f32| AfterImg {
            pos: [x, 0.0],
            facing: 1.0,
            scale: [1.0; 2],
            angle: 0.0,
            anim_owner: 0,
            sprite_data: 0,
            action: 0,
            elem: 0,
            priority: 0,
        };
        for t in 0..9 {
            ai.record(img(t as f32));
        }
        // MUGEN: images 3, 6 and 9 ticks back
        let v = ai.visible(0);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].0.pos[0], 6.0);
        assert_eq!(v[1].0.pos[0], 3.0);
        assert_eq!(v[2].0.pos[0], 0.0);
        assert!(v[1].1.mul[0] < v[0].1.mul[0]);
        assert!(v[0].2 > v[1].2);
    }

    #[test]
    fn envshake_decays_to_zero() {
        let mut e = EnvShake {
            time: 10,
            ..Default::default()
        };
        e.start();
        assert_eq!(e.phase, Some(0.0));
        let mut moved = false;
        for _ in 0..10 {
            e.update();
            moved |= e.offset[1] != 0.0;
        }
        assert!(moved);
        assert!(!e.active());
        e.update();
        assert_eq!(e.offset, [0.0; 2]);
    }
}
