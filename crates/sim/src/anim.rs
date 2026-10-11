//! Animation playback state, matching Ikemen GO's `Animation` timing so
//! `AnimTime`, `AnimElemTime`, `AnimElemNo` and looping behave like MUGEN.

use rugen_formats::air::{Action, Frame};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimState {
    /// Action number, or -1 when none.
    pub action: i32,
    pub elem: usize,
    pub elem_time: i32,
    /// Ticks since the action started (wraps on loop).
    pub time: i32,
    pub loop_count: i32,
    pub loop_end: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            action: -1,
            elem: 0,
            elem_time: 0,
            time: 0,
            loop_count: 0,
            loop_end: false,
        }
    }
}

impl AnimState {
    pub fn start(action: i32) -> Self {
        Self {
            action,
            ..Default::default()
        }
    }

    pub fn frame<'a>(&self, a: &'a Action) -> Option<&'a Frame> {
        a.frames
            .get(self.elem.min(a.frames.len().saturating_sub(1)))
    }

    /// `AnimTime` trigger: `time - totaltime`, 0 exactly when the action ends.
    pub fn anim_time(&self, a: &Action) -> i32 {
        self.time - a.total_time
    }

    /// `AnimElemTime(elem)`; `None` when `elem` is out of range (bottom).
    pub fn anim_elem_time(&self, a: &Action, elem: i32) -> Option<i32> {
        if elem < 1 || elem as usize > a.frames.len() {
            return None;
        }
        let mut t = self.time;
        for f in &a.frames[..(elem - 1) as usize] {
            t -= f.time.max(0);
        }
        Some(t)
    }

    /// `AnimElemNo(time)`.
    pub fn anim_elem_no(&self, a: &Action, time: i32) -> Option<i32> {
        if time < -self.time || a.frames.is_empty() {
            return None;
        }
        let n = a.frames.len() as i32;
        let mut i = self.elem as i32;
        let mut t = time;
        let mut oldt = 0;
        if t <= 0 {
            t += self.elem_time;
            let mut looped = false;
            loop {
                if t >= 0 {
                    return Some(i + 1);
                }
                i -= 1;
                if i < 0 || (self.elem as i32 >= a.loop_start as i32 && i < a.loop_start as i32) {
                    if t == oldt {
                        break;
                    }
                    oldt = t;
                    looped = true;
                    i = n - 1;
                }
                t += a.frames[i as usize].time.max(0);
                if looped && i == n - 1 && a.frames[i as usize].time == -1 {
                    return Some(i + 1);
                }
            }
        } else {
            t += self.elem_time;
            loop {
                t -= a.frames[i as usize].time.max(0);
                if t < 0 || (i == n - 1 && a.frames[i as usize].time == -1) {
                    return Some(i + 1);
                }
                i += 1;
                if i >= n {
                    if t == oldt {
                        break;
                    }
                    oldt = t;
                    i = a.loop_start as i32;
                }
            }
        }
        Some(n)
    }

    /// `ChangeAnim elem=`: jump to a 1-based element.
    pub fn set_elem(&mut self, a: &Action, elem: i32, elem_time: i32) {
        let mut e = (elem.max(1) - 1) as usize;
        if e >= a.frames.len() {
            e = 0;
        }
        self.elem = e;
        let mut et = elem_time;
        if et != 0 {
            let ft = a.frames[e].time;
            if et < 0 || (ft != -1 && et >= ft) {
                et = 0;
            }
        }
        self.elem_time = et;
        self.loop_end = false;
        self.loop_count = 0;
        self.time = 0;
        self.time = -self.anim_elem_time(a, e as i32 + 1).unwrap_or(0) + et;
    }

    /// Advance one tick (Ikemen `Animation.Action`).
    pub fn step(&mut self, a: &Action) {
        let n = a.frames.len();
        if n == 0 {
            self.loop_end = true;
            return;
        }
        let next = |s: &mut Self| {
            if a.total_time != -1 || s.elem < n - 1 {
                s.elem_time = 0;
                loop {
                    s.elem += 1;
                    if (a.total_time == -1 && s.elem == n - 1)
                        || s.elem >= n
                        || a.frames[s.elem].time > 0
                    {
                        break;
                    }
                }
            }
        };
        if a.frames[self.elem.min(n - 1)].time <= 0 {
            next(self);
        }
        if self.elem < n {
            self.elem_time += 1;
            if self.elem_time >= a.frames[self.elem].time {
                next(self);
                if self.elem >= n {
                    self.elem = a.loop_start.min(n - 1);
                }
            }
        } else {
            self.elem = a.loop_start.min(n - 1);
        }
        if a.total_time != -1 && self.time >= a.total_time {
            self.time = a.total_time - a.loop_time;
            self.loop_count += 1;
        }
        self.time += 1;
        if a.total_time != -1 && self.time >= a.total_time {
            self.loop_end = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugen_formats::air::Air;

    fn action(src: &str) -> Action {
        Air::parse(src).actions.into_iter().next().unwrap().1
    }

    #[test]
    fn timing_like_mugen() {
        // 3 frames: 2,3,-1
        let a = action("[Begin Action 0]\n0,0,0,0,2\n0,1,0,0,3\n0,2,0,0,-1\n");
        let mut s = AnimState::start(0);
        assert_eq!(s.anim_elem_time(&a, 1), Some(0));
        assert_eq!(s.anim_elem_time(&a, 2), Some(-2));
        assert_eq!(s.anim_elem_time(&a, 4), None);
        assert_eq!(s.elem, 0);
        s.step(&a);
        assert_eq!((s.elem, s.time), (0, 1));
        s.step(&a);
        assert_eq!((s.elem, s.time), (1, 2));
        assert_eq!(s.anim_elem_time(&a, 2), Some(0), "AnimElem = 2 fires here");
        for _ in 0..3 {
            s.step(&a);
        }
        assert_eq!((s.elem, s.time), (2, 5));
        for _ in 0..10 {
            s.step(&a);
        }
        assert_eq!(s.elem, 2, "holds on -1 frame");
        assert_eq!(a.total_time, -1);
    }

    #[test]
    fn looping_and_animtime() {
        let a = action("[Begin Action 0]\n0,0,0,0,2\n0,1,0,0,2\n");
        let mut s = AnimState::start(0);
        assert_eq!(a.total_time, 4);
        assert_eq!(s.anim_time(&a), -4);
        for _ in 0..4 {
            s.step(&a);
        }
        assert_eq!(
            s.anim_time(&a),
            0,
            "AnimTime = 0 on the last tick of the loop"
        );
        assert_eq!(s.elem, 0);
        s.step(&a);
        assert_eq!(s.anim_time(&a), -3);
        assert_eq!(s.loop_count, 1);
    }

    #[test]
    fn set_elem() {
        let a = action("[Begin Action 0]\n0,0,0,0,2\n0,1,0,0,3\n0,2,0,0,4\n");
        let mut s = AnimState::start(0);
        s.set_elem(&a, 3, 1);
        assert_eq!((s.elem, s.elem_time, s.time), (2, 1, 6));
        assert_eq!(s.anim_elem_time(&a, 3), Some(1));
        assert_eq!(s.anim_elem_no(&a, 0), Some(3));
        assert_eq!(s.anim_elem_no(&a, -2), Some(2));
    }
}
