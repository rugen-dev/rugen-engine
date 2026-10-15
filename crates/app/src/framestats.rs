//! Frame profiling behind `RUGEN_FRAMESTATS=1`: CPU time per phase (update,
//! data rebuild, UI layout, draw building, GPU submit, swapchain present)
//! plus the renderer's draw calls and texture uploads, logged once a second.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Phase {
    /// One shell tick (input, screen logic; includes Data and Layout).
    Update = 0,
    /// Rebuilding a TOML screen's data context.
    Data,
    /// The UI engine's tick (layout + navigation).
    Layout,
    /// Building the frame's draw list.
    Build,
    /// Vertex upload, bind groups, encode, submit (CPU render cost).
    Submit,
    /// Swapchain acquire and present: the vsync wait when the driver blocks
    /// there (GL's `eglSwapBuffers`), not render cost.
    Present,
}

const PHASES: usize = 6;
const NAMES: [&str; PHASES] = ["update", "data", "layout", "build", "submit", "present"];

static SUM: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];
static MAX: [AtomicU64; PHASES] = [const { AtomicU64::new(0) }; PHASES];

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUGEN_FRAMESTATS").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Time `f` as `phase` when profiling is on.
pub fn time<T>(phase: Phase, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let t = Instant::now();
    let out = f();
    add(phase, t.elapsed());
    out
}

pub fn add(phase: Phase, d: Duration) {
    let us = d.as_micros() as u64;
    SUM[phase as usize].fetch_add(us, Ordering::Relaxed);
    MAX[phase as usize].fetch_max(us, Ordering::Relaxed);
}

/// Once-a-second report.
pub struct Reporter {
    since: Instant,
    frames: u64,
}

impl Default for Reporter {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
        }
    }
}

impl Reporter {
    /// Count a presented frame; log the averages once a second.
    pub fn frame(&mut self, screen: &str, r: &rugen_render::Renderer) {
        self.frames += 1;
        let el = self.since.elapsed();
        if el < Duration::from_secs(1) {
            return;
        }
        let st = r.take_stats();
        let n = self.frames.max(1);
        let mut line = format!("framestats {screen}: {n} fps");
        for (i, name) in NAMES.iter().enumerate() {
            let sum = SUM[i].swap(0, Ordering::Relaxed);
            let max = MAX[i].swap(0, Ordering::Relaxed);
            line += &format!(
                " | {name} {:.2}ms (max {:.2})",
                sum as f64 / n as f64 / 1000.0,
                max as f64 / 1000.0
            );
        }
        line += &format!(
            " | draws {} quads {} | uploads {} ({} KiB) textures {} palettes {}",
            st.draws,
            st.quads,
            st.uploads,
            st.upload_bytes / 1024,
            st.textures,
            st.palettes
        );
        log::info!("{line}");
        self.since = Instant::now();
        self.frames = 0;
    }
}
