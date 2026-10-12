//! Input prediction strategies, measured on recorded input streams.
//!
//! ggrs 0.11 predicts a missing remote input by repeating the last one it
//! received (`InputQueue::input`, not configurable). This module measures
//! how often a prediction made `lag` frames ahead is wrong for that and
//! for the alternatives ("held directions persist, attack buttons are
//! taps"), so the choice rests on data. On rugen's recorded matches
//! (`examples/predict.rs`, 76k frames) releasing buttons is clearly worse
//! (24% vs 7% wrong one frame ahead), and the best tap model gains at most
//! ~2 points at 4-6 frames while losing 2-5 points at 1-2 and 8-10 frames;
//! it would also need a ggrs fork (its prediction is one constant per
//! predicted run). The engine keeps ggrs' prediction and spends the effort
//! on cheap rollbacks instead.

/// Direction bits of [`rugen_sim::Buttons::bits`] (U D L R).
pub const DIRS: u16 = 0b1111;

/// A way to guess the input `lag` frames after the last known one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// ggrs: the last received input, unchanged.
    RepeatLast,
    /// Directions persist, buttons released.
    DirectionsOnly,
    /// Directions persist; a button stays held only while it has been held
    /// for at most `n` frames by the predicted frame (taps end).
    TapLength(u32),
}

impl Strategy {
    /// Predict the input `lag` frames after `history`'s last entry.
    pub fn predict(self, history: &[u16], lag: usize) -> u16 {
        let Some(&last) = history.last() else {
            return 0;
        };
        match self {
            Strategy::RepeatLast => last,
            Strategy::DirectionsOnly => last & DIRS,
            Strategy::TapLength(n) => {
                let mut keep = last & DIRS;
                for bit in 4..11 {
                    let m = 1u16 << bit;
                    if last & m == 0 {
                        continue;
                    }
                    let held = history.iter().rev().take_while(|v| *v & m != 0).count();
                    if held + lag <= n as usize || held > 2 * n as usize {
                        // still inside a tap, or a long hold (charge)
                        keep |= m;
                    }
                }
                keep
            }
        }
    }
}

/// Fraction of frames whose input, predicted `lag` frames ahead, was
/// wrong, over all `streams`.
pub fn misprediction_rate(streams: &[Vec<u16>], lag: usize, s: Strategy) -> f64 {
    let (mut wrong, mut n) = (0u64, 0u64);
    for st in streams {
        for f in lag..st.len() {
            let from = (f - lag).saturating_sub(30);
            let guess = s.predict(&st[from..=f - lag], lag);
            n += 1;
            wrong += u64::from(guess != st[f]);
        }
    }
    if n == 0 { 0.0 } else { wrong as f64 / n as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = 1 << 4;
    const R: u16 = 0b1000;

    #[test]
    fn strategies_predict_as_documented() {
        let h = [0, R, R, R | A];
        assert_eq!(Strategy::RepeatLast.predict(&h, 3), R | A);
        assert_eq!(Strategy::DirectionsOnly.predict(&h, 3), R);
        // A held 1 frame: within a 4-frame tap for 3 more frames
        assert_eq!(Strategy::TapLength(4).predict(&h, 3), R | A);
        assert_eq!(Strategy::TapLength(4).predict(&h, 4), R);
        // a long hold (charge) is kept
        let charge = [A; 12];
        assert_eq!(Strategy::TapLength(4).predict(&charge, 5), A);
        assert_eq!(Strategy::RepeatLast.predict(&[], 1), 0);
    }

    #[test]
    fn misprediction_rate_counts_wrong_guesses() {
        // R held 6 frames, then a 3-frame tap of A, then idle
        let s = vec![vec![R, R, R, R, R, R, A, A, A, 0, 0, 0]];
        let r1 = misprediction_rate(&s, 1, Strategy::RepeatLast);
        // changes at frames 6 and 9: 2 wrong of 11
        assert!((r1 - 2.0 / 11.0).abs() < 1e-9);
        assert!(misprediction_rate(&s, 1, Strategy::DirectionsOnly) > r1);
        assert_eq!(misprediction_rate(&[], 1, Strategy::RepeatLast), 0.0);
    }
}
