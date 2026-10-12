//! Local input-to-display latency probe (`RUGEN_LATENCY=1`): every 5 s logs
//! how long a key press waits for the next simulation tick, how long the
//! newest tick waits to be presented, how long presenting blocked on vsync,
//! and an estimate of when the frame reaches the screen.
//!
//! The display's scan-out itself is not observable; the estimate models it:
//! when acquiring a swapchain image blocked (FIFO back-pressure), that image
//! was freed by a vblank and the frame drawn into it is shown `queued`
//! refreshes after that vblank; when it did not block (mailbox), the frame
//! is shown at the next vblank, on average half a refresh after presenting.

use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Probe {
    on: bool,
    refresh: Duration,
    /// First key event since the last tick.
    pending: Option<Instant>,
    /// Key event consumed by the newest tick, waiting for its present.
    consumed: Option<Instant>,
    scripted: bool,
    /// The newest tick, not presented yet.
    tick: Option<Instant>,
    ev_tick: Vec<f64>,
    tick_present: Vec<f64>,
    tick_display: Vec<f64>,
    key_display: Vec<f64>,
    acquire: Vec<f64>,
    renders: u32,
    since: Option<Instant>,
    mode: String,
    queued: u32,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn summary(v: &mut [f64]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    format!(
        "{mean:.1} (p95 {:.1})",
        v[(v.len() * 95 / 100).min(v.len() - 1)]
    )
}

impl Probe {
    pub fn new() -> Self {
        Self {
            on: std::env::var_os("RUGEN_LATENCY").is_some(),
            refresh: Duration::from_nanos(1_000_000_000 / 60),
            ..Default::default()
        }
    }

    pub fn set_mode(&mut self, mode: String, queued: u32) {
        self.mode = mode;
        self.queued = queued.max(1);
    }

    /// A key went down (window event).
    pub fn key(&mut self) {
        if self.on {
            self.pending.get_or_insert_with(Instant::now);
        }
    }

    /// A simulation tick finished (it read the input state at its start).
    /// `start` is when it began; scripted inputs (the harness) count as
    /// arriving at the tick start.
    pub fn ticked(&mut self, start: Instant, scripted: bool) {
        if !self.on {
            return;
        }
        let ev = self.pending.take().or(scripted.then_some(start));
        if let Some(e) = ev {
            self.ev_tick.push(ms(start.saturating_duration_since(e)));
            self.consumed = Some(e);
            self.scripted = scripted;
        }
        self.tick = Some(Instant::now());
    }

    /// A frame was presented at `at`; acquiring its image took `acquire`
    /// and returned at `acquired`.
    pub fn presented(&mut self, at: Instant, acquire: Duration, acquired: Option<Instant>) {
        if !self.on {
            return;
        }
        self.renders += 1;
        self.acquire.push(ms(acquire));
        let blocked = acquire > Duration::from_millis(1);
        let shown = match acquired {
            Some(a) if blocked => a + self.refresh * self.queued,
            _ => at + self.refresh / 2,
        };
        if let Some(t) = self.tick.take() {
            self.tick_present.push(ms(at.saturating_duration_since(t)));
            self.tick_display
                .push(ms(shown.saturating_duration_since(t)));
        }
        if let Some(e) = self.consumed.take() {
            // a real key press waits on average half a tick for the tick
            let wait = if self.scripted {
                self.refresh / 2
            } else {
                Duration::ZERO
            };
            self.key_display
                .push(ms(shown.saturating_duration_since(e) + wait));
        }
        let since = *self.since.get_or_insert(at);
        if at.duration_since(since) >= Duration::from_secs(5) {
            self.report(at.duration_since(since));
            self.since = Some(at);
        }
    }

    fn report(&mut self, over: Duration) {
        log::info!(
            "latency [{} x{}]: {:.0} renders/s | key->tick {} ms | tick->present {} ms | vsync wait {} ms | est. tick->display {} ms | est. key->display {} ms",
            self.mode,
            self.queued,
            f64::from(self.renders) / over.as_secs_f64(),
            summary(&mut self.ev_tick),
            summary(&mut self.tick_present),
            summary(&mut self.acquire),
            summary(&mut self.tick_display),
            summary(&mut self.key_display),
        );
        self.ev_tick.clear();
        self.tick_present.clear();
        self.tick_display.clear();
        self.key_display.clear();
        self.acquire.clear();
        self.renders = 0;
    }
}
