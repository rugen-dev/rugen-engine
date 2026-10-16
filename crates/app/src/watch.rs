//! Watching a match from the server's input log: a live one (even
//! mid-match) or, later, a shared replay. The fighters and the stage come
//! through the package sync (downloaded like an opponent's fighter), the
//! inputs from `GET /v1/live/{id}` in chunks; the match is simulated
//! locally with the header's rules and seed, so it is the players' match
//! frame for frame.
//!
//! [`Feed`] holds the stream as it arrives (a worker polls the server);
//! [`Prep`] fetches the files. The fight plays the feed through
//! [`crate::play::NetMode::Stream`]: it waits for a short buffer, then
//! fast-forwards without drawing to [`LIVE_DELAY`] behind the players
//! ("CATCHING UP") and follows them from there.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rugen_content::Library;
use rugen_net::online::{OnlineClient, Transfer};
use rugen_proto::stream::{Chunk, MatchHeader};

use crate::lang::{t, tf};

/// How far behind the players a viewer follows (3 s): network jitter
/// never shows as a stall.
pub const LIVE_DELAY: usize = 180;
/// Frames buffered before the picture starts (a smooth start).
pub const START_BUFFER: usize = 90;
/// Behind by more than this: fast-forward instead of playing.
pub const CATCHUP_MARGIN: usize = 120;
/// Frames simulated per drawn frame while catching up.
pub const CATCHUP_STEPS: usize = 120;
/// How often the worker asks the server for new frames.
const POLL: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
pub struct FeedState {
    pub header: Option<MatchHeader>,
    pub frames: Vec<[u16; 2]>,
    /// No frame will follow.
    pub ended: bool,
    /// Frames the server held at the last answer.
    pub total: u32,
    pub error: Option<String>,
}

/// A match stream as it arrives, shared by the worker and the fight.
#[derive(Clone, Debug, Default)]
pub struct Feed(pub Arc<Mutex<FeedState>>);

impl Feed {
    /// A whole stream known up front (a shared replay).
    pub fn complete(header: MatchHeader, frames: Vec<[u16; 2]>) -> Self {
        let total = frames.len() as u32;
        Self(Arc::new(Mutex::new(FeedState {
            header: Some(header),
            frames,
            ended: true,
            total,
            error: None,
        })))
    }

    /// Follow live match `match_id`: a worker polls the server until the
    /// match ended and every frame arrived, or nobody holds the feed any more.
    pub fn follow(client: OnlineClient, match_id: u64) -> Self {
        let feed = Self::default();
        let weak = Arc::downgrade(&feed.0);
        std::thread::spawn(move || {
            let mut failures = 0;
            loop {
                let Some(state) = weak.upgrade() else { return };
                let have = state.lock().map_or(0, |s| s.frames.len()) as u32;
                drop(state);
                match client.live_chunk(match_id, have) {
                    Ok(c) => {
                        failures = 0;
                        let Some(state) = weak.upgrade() else { return };
                        let done = Self::apply(&state, c);
                        if done {
                            return;
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        // the server's viewer cap: no point asking again
                        if failures >= 10 || is_full(&e.to_string()) {
                            if let Some(state) = weak.upgrade()
                                && let Ok(mut s) = state.lock()
                            {
                                s.error = Some(e.to_string());
                            }
                            return;
                        }
                    }
                }
                std::thread::sleep(POLL);
            }
        });
        feed
    }

    /// Add a chunk; true when the stream is complete.
    fn apply(state: &Mutex<FeedState>, c: Chunk) -> bool {
        let Ok(mut s) = state.lock() else { return true };
        if let Some(h) = c.header {
            s.header = Some(h);
        }
        if c.from as usize == s.frames.len() {
            s.frames.extend_from_slice(&c.inputs);
        }
        s.total = c.total;
        s.ended = c.ended;
        c.ended && s.frames.len() >= c.total as usize
    }

    /// (frames arrived, the stream is complete).
    pub fn status(&self) -> (usize, bool) {
        self.0.lock().map_or((0, true), |s| {
            (
                s.frames.len(),
                s.ended && s.frames.len() >= s.total as usize,
            )
        })
    }

    pub fn frame(&self, i: usize) -> Option<[u16; 2]> {
        self.0.lock().ok()?.frames.get(i).copied()
    }

    pub fn header(&self) -> Option<MatchHeader> {
        self.0.lock().ok()?.header.clone()
    }

    pub fn error(&self) -> Option<String> {
        self.0.lock().ok()?.error.clone()
    }
}

/// Whether a stream error is the server's viewer cap (`RUGEN_LIVE_VIEWERS`).
pub fn is_full(e: &str) -> bool {
    e.contains("this match is full")
}

/// How many frames to simulate this drawn frame, `cursor` frames in with
/// `have` arrived: 0 while the start buffer fills (or nothing new), 1 to
/// follow, many to catch up to [`LIVE_DELAY`] behind.
pub fn steps(cursor: usize, have: usize, complete: bool, started: bool) -> usize {
    if !started && !complete && have < START_BUFFER {
        return 0;
    }
    let lag = have.saturating_sub(cursor);
    if lag == 0 {
        0
    } else if complete && cursor == 0 && lag > 0 {
        // a finished match (a replay) plays from the start
        1
    } else if lag > LIVE_DELAY + CATCHUP_MARGIN && !complete {
        (lag - LIVE_DELAY).min(CATCHUP_STEPS)
    } else {
        1
    }
}

/// Fetching the files of a watched match.
#[derive(Clone, Debug, Default)]
pub struct PrepState {
    /// (label, fraction) of the transfers.
    pub progress: Vec<(String, f32)>,
    /// P1, P2 and stage `.def` once every file is local.
    pub defs: Option<[PathBuf; 3]>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Prep(pub Arc<Mutex<PrepState>>);

impl Prep {
    /// Find (or download) the three packages of `header`.
    pub fn start(client: OnlineClient, lib: Library, header: MatchHeader) -> Self {
        let prep = Self::default();
        let st = prep.0.clone();
        std::thread::spawn(move || {
            let items = [
                (
                    header.fighters[0].name.to_uppercase(),
                    header.fighters[0].clone(),
                ),
                (
                    header.fighters[1].name.to_uppercase(),
                    header.fighters[1].clone(),
                ),
                (t("stages.stage").to_string(), header.stage.clone()),
            ];
            if let Ok(mut s) = st.lock() {
                s.progress = items.iter().map(|(l, _)| (l.clone(), 0.0)).collect();
            }
            let mut defs = Vec::new();
            for (i, (label, c)) in items.iter().enumerate() {
                let done = |f: f32| {
                    if let Ok(mut s) = st.lock()
                        && let Some(p) = s.progress.get_mut(i)
                    {
                        p.1 = f;
                    }
                };
                if lib.def_for_fingerprint(&c.fingerprint).is_none() {
                    let part = lib
                        .cache_dir()
                        .join("net/partial")
                        .join(format!("{}.part", c.fingerprint));
                    let t = Transfer::new();
                    let (t2, st2) = (t.clone(), st.clone());
                    let watch = std::thread::spawn(move || {
                        while Arc::strong_count(&t2) > 1 {
                            if let Ok(mut s) = st2.lock()
                                && let Some(p) = s.progress.get_mut(i)
                            {
                                p.1 = t2.fraction();
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    });
                    let r = client
                        .download_package(&c.fingerprint, c.size, &part, &t)
                        .map_err(|e| e.to_string())
                        .and_then(|()| {
                            lib.install_package(&part, &c.fingerprint, 1 << 30)
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        });
                    let _ = std::fs::remove_file(&part);
                    drop(t);
                    let _ = watch.join();
                    if let Err(e) = r {
                        if let Ok(mut s) = st.lock() {
                            s.error = Some(tf(
                                "matchup.could_not_download",
                                &[("what", &label), ("error", &e)],
                            ));
                        }
                        return;
                    }
                }
                done(1.0);
                match lib.def_for_fingerprint(&c.fingerprint) {
                    Some(d) => defs.push(d),
                    None => {
                        if let Ok(mut s) = st.lock() {
                            s.error = Some(tf(
                                "watch.is_missing_after_the_download",
                                &[("what", &label)],
                            ));
                        }
                        return;
                    }
                }
            }
            if let (Ok(mut s), Ok(d)) = (st.lock(), <[PathBuf; 3]>::try_from(defs)) {
                s.defs = Some(d);
            }
        });
        prep
    }

    pub fn snapshot(&self) -> PrepState {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_buffers_catches_up_then_follows() {
        // the start waits for a short buffer
        assert_eq!(steps(0, 30, false, false), 0);
        assert_eq!(steps(0, START_BUFFER, false, false), 1);
        // late: far behind the players, fast-forward to LIVE_DELAY behind
        let have = 3600;
        let n = steps(0, have, false, true);
        assert_eq!(n, CATCHUP_STEPS);
        let n = steps(have - LIVE_DELAY - CATCHUP_MARGIN - 10, have, false, true);
        assert_eq!(n, (CATCHUP_MARGIN + 10).min(CATCHUP_STEPS));
        // within the delay: one frame per frame, none when caught up
        assert_eq!(steps(have - LIVE_DELAY, have, false, true), 1);
        assert_eq!(steps(have, have, false, true), 0);
        // a finished stream (a replay) plays at normal speed from the start
        assert_eq!(steps(0, have, true, false), 1);
        assert_eq!(steps(100, have, true, true), 1);
    }

    #[test]
    fn chunks_fill_the_feed_in_order() {
        let feed = Feed::default();
        let c = |from: u32, n: usize, ended: bool, total: u32| Chunk {
            header: (from == 0).then(MatchHeader::default),
            ended,
            total,
            from,
            inputs: vec![[1, 2]; n],
        };
        assert!(!Feed::apply(&feed.0, c(0, 10, false, 10)));
        assert!(feed.header().is_some());
        // a stale chunk (from a lost race) is ignored
        assert!(!Feed::apply(&feed.0, c(4, 3, false, 10)));
        assert_eq!(feed.status(), (10, false));
        assert!(Feed::apply(&feed.0, c(10, 5, true, 15)));
        assert_eq!(feed.status(), (15, true));
        assert_eq!(feed.frame(14), Some([1, 2]));
        assert_eq!(feed.frame(15), None);
        let done = Feed::complete(MatchHeader::default(), vec![[0, 0]; 3]);
        assert_eq!(done.status(), (3, true));
    }
}
