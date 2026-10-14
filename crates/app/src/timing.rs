//! Launch and fight-entry timings, logged at info level (`timing: ...`):
//! the first frame after launch, how long building screens took, and the
//! frame intervals of the first seconds of a fight (hitches show up as
//! long intervals). Cheap enough to stay on.

use std::sync::Mutex;
use std::time::{Duration, Instant};

struct State {
    launch: Instant,
    first_frame: bool,
    last_present: Option<Instant>,
    /// Fight entry: when the fight was asked for, the frame intervals so far.
    fight: Option<(Instant, Vec<Duration>)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| State {
        launch: Instant::now(),
        first_frame: false,
        last_present: None,
        fight: None,
    });
    f(s)
}

/// Note the launch (call first thing in `main`).
pub fn launch() {
    with(|_| {});
}

/// Milliseconds since launch.
pub fn since_launch() -> u128 {
    with(|s| s.launch.elapsed().as_millis())
}

/// Log how long `what` took.
pub fn took(what: &str, d: Duration) {
    log::info!("timing: {what} took {} ms", d.as_millis());
}

/// A fight was asked for (the VS screen ended).
pub fn fight_requested() {
    with(|s| s.fight = Some((Instant::now(), Vec::new())));
}

/// Frames of a fight measured before the summary is logged (5 s).
const FIGHT_FRAMES: usize = 300;

/// A frame was presented; `in_fight` while a match is on screen.
pub fn presented(in_fight: bool) {
    with(|s| {
        let now = Instant::now();
        if !s.first_frame {
            s.first_frame = true;
            log::info!(
                "timing: first frame {} ms after launch",
                s.launch.elapsed().as_millis()
            );
        }
        let gap = s.last_present.map(|t| now - t);
        s.last_present = Some(now);
        if !in_fight {
            return;
        }
        let Some((asked, frames)) = s.fight.as_mut() else {
            return;
        };
        if frames.is_empty() {
            log::info!(
                "timing: first fight frame {} ms after the fight was asked for",
                asked.elapsed().as_millis()
            );
            frames.push(Duration::ZERO);
            return;
        }
        frames.push(gap.unwrap_or_default());
        if frames.len() > FIGHT_FRAMES {
            let f = &frames[1..];
            let max = f.iter().max().copied().unwrap_or_default();
            let mean = f.iter().sum::<Duration>() / f.len() as u32;
            let over = |ms: u64| f.iter().filter(|d| **d > Duration::from_millis(ms)).count();
            log::info!(
                "timing: first {} fight frames: mean {:.1} ms, max {:.1} ms, {} over 20 ms, {} over 33 ms",
                f.len(),
                mean.as_secs_f64() * 1000.0,
                max.as_secs_f64() * 1000.0,
                over(20),
                over(33)
            );
            s.fight = None;
        }
    });
}
