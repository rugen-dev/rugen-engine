//! Raw inputs and the per-tick input buffer (Ikemen GO `InputBuffer`).
//!
//! Each key keeps a signed counter: `+n` = held for n ticks, `-n` = released
//! for n ticks. `State(key) == 1` means "just pressed this tick".

/// One tick of raw input, absolute directions (L/R, not B/F).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Buttons {
    pub u: bool,
    pub d: bool,
    pub l: bool,
    pub r: bool,
    pub a: bool,
    pub b: bool,
    pub c: bool,
    pub x: bool,
    pub y: bool,
    pub z: bool,
    pub s: bool,
}

impl Buttons {
    pub fn bits(self) -> u16 {
        [
            self.u, self.d, self.l, self.r, self.a, self.b, self.c, self.x, self.y, self.z, self.s,
        ]
        .iter()
        .enumerate()
        .fold(0, |acc, (i, &b)| acc | ((b as u16) << i))
    }
    pub fn from_bits(v: u16) -> Self {
        let g = |i: u16| v & (1 << i) != 0;
        Self {
            u: g(0),
            d: g(1),
            l: g(2),
            r: g(3),
            a: g(4),
            b: g(5),
            c: g(6),
            x: g(7),
            y: g(8),
            z: g(9),
            s: g(10),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    U,
    D,
    B,
    F,
    UB,
    UF,
    DB,
    DF,
    N,
    A,
    Bt,
    C,
    X,
    Y,
    Z,
    S,
}

impl Key {
    pub fn is_direction(self) -> bool {
        !matches!(
            self,
            Key::A | Key::Bt | Key::C | Key::X | Key::Y | Key::Z | Key::S
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputBuffer {
    // current (b) and previous (p) counters
    pub bb: i32,
    pub db: i32,
    pub fb: i32,
    pub ub: i32,
    pub nb: i32,
    pub ab: i32,
    pub btb: i32,
    pub cb: i32,
    pub xb: i32,
    pub yb: i32,
    pub zb: i32,
    pub sb: i32,
    pub bp: i32,
    pub dp: i32,
    pub fp: i32,
    pub up: i32,
    pub np: i32,
    pub ap: i32,
    pub btp: i32,
    pub cp: i32,
    pub xp: i32,
    pub yp: i32,
    pub zp: i32,
    pub sp: i32,
}

fn bump(held: bool, buf: &mut i32) {
    if held != (*buf > 0) {
        *buf = if held { 1 } else { -1 };
    } else if held {
        *buf += 1;
    } else {
        *buf -= 1;
    }
}

impl InputBuffer {
    /// Feed one tick. `b`/`f` are already relative to facing. Opposite
    /// directions cancel (neutral SOCD resolution).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        u: bool,
        d: bool,
        b: bool,
        f: bool,
        a: bool,
        bt: bool,
        c: bool,
        x: bool,
        y: bool,
        z: bool,
        s: bool,
    ) {
        self.up = self.ub;
        self.dp = self.db;
        self.bp = self.bb;
        self.fp = self.fb;
        self.np = self.nb;
        self.ap = self.ab;
        self.btp = self.btb;
        self.cp = self.cb;
        self.xp = self.xb;
        self.yp = self.yb;
        self.zp = self.zb;
        self.sp = self.sb;
        let (u, d) = if u && d { (false, false) } else { (u, d) };
        let (b, f) = if b && f { (false, false) } else { (b, f) };
        bump(u, &mut self.ub);
        bump(d, &mut self.db);
        bump(b, &mut self.bb);
        bump(f, &mut self.fb);
        bump(!(u || d || b || f), &mut self.nb);
        bump(a, &mut self.ab);
        bump(bt, &mut self.btb);
        bump(c, &mut self.cb);
        bump(x, &mut self.xb);
        bump(y, &mut self.yb);
        bump(z, &mut self.zb);
        bump(s, &mut self.sb);
    }

    fn cur(&self, k: Key) -> i32 {
        match k {
            Key::U => self.ub,
            Key::D => self.db,
            Key::B => self.bb,
            Key::F => self.fb,
            Key::N => self.nb,
            Key::A => self.ab,
            Key::Bt => self.btb,
            Key::C => self.cb,
            Key::X => self.xb,
            Key::Y => self.yb,
            Key::Z => self.zb,
            Key::S => self.sb,
            Key::UB | Key::UF | Key::DB | Key::DF => unreachable!(),
        }
    }
    fn prev(&self, k: Key) -> i32 {
        match k {
            Key::U => self.up,
            Key::D => self.dp,
            Key::B => self.bp,
            Key::F => self.fp,
            Key::N => self.np,
            Key::A => self.ap,
            Key::Bt => self.btp,
            Key::C => self.cp,
            Key::X => self.xp,
            Key::Y => self.yp,
            Key::Z => self.zp,
            Key::S => self.sp,
            Key::UB | Key::UF | Key::DB | Key::DF => unreachable!(),
        }
    }

    /// Ticks a key (with modifiers) has been in the asked-for state; `1` = just now.
    /// `tilde` = release, `dollar` = ignore conflicting directions.
    pub fn state(&self, key: Key, tilde: bool, dollar: bool) -> i32 {
        let (ub, db, bb, fb) = (self.ub, self.db, self.bb, self.fb);
        let min4 = ub.abs().min(db.abs()).min(bb.abs()).min(fb.abs());
        if !tilde && !dollar {
            return match key {
                Key::U => (-db.max(bb).max(fb)).min(ub),
                Key::D => (-ub.max(bb).max(fb)).min(db),
                Key::B => (-ub.max(db).max(fb)).min(bb),
                Key::F => (-ub.max(db).max(bb)).min(fb),
                Key::UF => (-db.max(bb)).min(ub.min(fb)),
                Key::UB => (-db.max(fb)).min(ub.min(bb)),
                Key::DF => (-ub.max(bb)).min(db.min(fb)),
                Key::DB => (-ub.max(fb)).min(db.min(bb)),
                k => self.cur(k),
            };
        }
        if !tilde && dollar {
            let held = match key {
                Key::U => ub > 0,
                Key::D => db > 0,
                Key::B => bb > 0,
                Key::F => fb > 0,
                Key::UB => ub > 0 && bb > 0,
                Key::UF => ub > 0 && fb > 0,
                Key::DB => db > 0 && bb > 0,
                Key::DF => db > 0 && fb > 0,
                k => return self.cur(k),
            };
            return if held { min4 } else { 0 };
        }
        if tilde && !dollar {
            let rel = |k: Key| self.cur(k) < 0 || self.prev(k) > 0;
            return match key {
                Key::U if rel(Key::U) => -((-db.max(bb).max(fb)).min(ub)),
                Key::D if rel(Key::D) => -((-ub.max(bb).max(fb)).min(db)),
                Key::B if rel(Key::B) => -((-ub.max(db).max(fb)).min(bb)),
                Key::F if rel(Key::F) => -((-ub.max(db).max(bb)).min(fb)),
                Key::UF if rel(Key::U) && rel(Key::F) => -((-db.max(bb)).min(ub.min(fb))),
                Key::UB if rel(Key::U) && rel(Key::B) => -((-db.max(fb)).min(ub.min(bb))),
                Key::DF if rel(Key::D) && rel(Key::F) => -((-ub.max(bb)).min(db.min(fb))),
                Key::DB if rel(Key::D) && rel(Key::B) => -((-ub.max(fb)).min(db.min(bb))),
                Key::N => -self.nb,
                k if !k.is_direction() && rel(k) => -self.cur(k),
                _ => 0,
            };
        }
        // tilde && dollar
        let f = |k: Key, others: i32| -> i32 {
            let c = self.cur(k);
            if c < 0 || self.prev(k) > 0 {
                if c < 0 { -c } else { others }
            } else {
                0
            }
        };
        match key {
            Key::U => f(Key::U, db.abs().min(bb.abs()).min(fb.abs())),
            Key::D => f(Key::D, ub.abs().min(bb.abs()).min(fb.abs())),
            Key::B => f(Key::B, ub.abs().min(db.abs()).min(fb.abs())),
            Key::F => f(Key::F, ub.abs().min(db.abs()).min(bb.abs())),
            Key::UB | Key::UF | Key::DB | Key::DF => {
                let (k1, k2) = match key {
                    Key::UB => (Key::U, Key::B),
                    Key::UF => (Key::U, Key::F),
                    Key::DB => (Key::D, Key::B),
                    _ => (Key::D, Key::F),
                };
                let (c1, c2) = (self.cur(k1), self.cur(k2));
                if (c1 < 0 || self.prev(k1) > 0) && (c2 < 0 || self.prev(k2) > 0) {
                    -(c1.min(c2))
                } else {
                    0
                }
            }
            k => self.state(k, true, false),
        }
    }

    /// Charge time for `~30B` style keys.
    pub fn charge(&self, key: Key, tilde: bool, dollar: bool) -> i32 {
        let ignore_recent = |b: i32| if b == 1 { i32::MIN } else { b };
        if !tilde && dollar {
            return match key {
                Key::U => self.ub,
                Key::D => self.db,
                Key::B => self.bb,
                Key::F => self.fb,
                Key::UB => self.ub.min(self.bb),
                Key::UF => self.ub.min(self.fb),
                Key::DB => self.db.min(self.bb),
                Key::DF => self.db.min(self.fb),
                k => self.cur(k),
            };
        }
        if tilde {
            return match key {
                Key::U => -ignore_recent(self.ub).max(self.up.max(0)),
                Key::D => -ignore_recent(self.db).max(self.dp.max(0)),
                Key::B => -ignore_recent(self.bb).max(self.bp.max(0)),
                Key::F => -ignore_recent(self.fb).max(self.fp.max(0)),
                Key::UB => self.up.min(self.bp),
                Key::UF => self.up.min(self.fp),
                Key::DB => self.dp.min(self.bp),
                Key::DF => self.dp.min(self.fp),
                k => self.prev(k).max(0),
            };
        }
        self.state(key, false, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn press_release_counters() {
        let mut ib = InputBuffer::default();
        ib.update(
            false, false, false, true, false, false, false, false, false, false, false,
        );
        assert_eq!(ib.state(Key::F, false, false), 1);
        ib.update(
            false, false, false, true, false, false, false, false, false, false, false,
        );
        assert_eq!(ib.state(Key::F, false, false), 2);
        ib.update(
            false, false, false, false, false, false, false, false, false, false, false,
        );
        assert_eq!(ib.state(Key::F, true, false), 1, "just released");
        assert_eq!(ib.state(Key::N, false, false), 1);
        // DF: both held, no conflict
        ib.update(
            false, true, false, true, false, false, false, false, false, false, false,
        );
        assert_eq!(ib.state(Key::DF, false, false), 1);
        assert!(
            ib.state(Key::D, false, false) < 0,
            "plain D is false while F is held"
        );
        assert_eq!(ib.state(Key::D, false, true), 1, "$D ignores the conflict");
    }
}

/// MUGEN-style AI "button jamming" (ported from Ikemen `AiInput`): directions
/// and buttons are held for random spans, more often at higher levels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiInput {
    /// 0 = U, 1 = UR, 2 = R, 3 = DR, 4 = D, 5 = DL, 6 = L, 7 = UL
    dir: i32,
    dirt: i32,
    at: i32,
    bt: i32,
    ct: i32,
    xt: i32,
    yt: i32,
    zt: i32,
    st: i32,
}

impl AiInput {
    /// Advance one tick. `active` is false during intros and win poses.
    pub fn update(&mut self, level: i32, active: bool, rng: &mut crate::Rng) -> Buttons {
        if !active {
            *self = Self {
                dir: self.dir,
                ..Default::default()
            };
            return Buttons::default();
        }
        // Rand(1, chance) == 1 → hold for Rand(1, time)
        let jam = |t: &mut i32, chance: i32, time: i32, rng: &mut crate::Rng| -> bool {
            *t -= 1;
            if *t <= 0 {
                if rng.below(chance.max(1) as u32) == 0 {
                    *t = 1 + rng.below(time.max(1) as u32) as i32;
                    return true;
                }
                *t = 0;
            }
            false
        };
        if jam(&mut self.dirt, 15, 60, rng) {
            self.dir = rng.below(8) as i32;
        }
        let chance = ((-11.25 * level as f32 + 165.0) * 7.0) as i32;
        for t in [
            &mut self.at,
            &mut self.bt,
            &mut self.ct,
            &mut self.xt,
            &mut self.yt,
            &mut self.zt,
        ] {
            jam(t, chance, 30, rng);
        }
        jam(&mut self.st, 3600, 30, rng);
        let d = self.dirt != 0;
        Buttons {
            u: d && matches!(self.dir, 7 | 0 | 1),
            d: d && matches!(self.dir, 3..=5),
            l: d && matches!(self.dir, 5..=7),
            r: d && matches!(self.dir, 1..=3),
            a: self.at != 0,
            b: self.bt != 0,
            c: self.ct != 0,
            x: self.xt != 0,
            y: self.yt != 0,
            z: self.zt != 0,
            s: self.st != 0,
        }
    }
}
