//! Background preparation of the library: converting `chars/` and
//! `stages/` archives into content files, describing them (portraits,
//! fingerprint, profile) and verifying fighters, so the game opens straight
//! to the title like MUGEN and nothing slow runs on the UI thread.
//!
//! A few low-priority worker threads take entries from a queue (stages
//! first, then fighters); a picked fighter jumps to the front
//! ([`Preparer::prioritize`]). Screens ask for an entry's [`State`]: menus
//! draw unready fighters dimmed with a progress ring, the versus screen
//! waits for an unready pick. Only finished content files are ever
//! returned (conversions write a hidden file and rename it when verified),
//! so online matches and replays never see a half-converted fighter.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use rugen_content::{Entry, Kind, Library, Manifest};

/// Where an entry is.
#[derive(Clone, Debug)]
pub enum State {
    Queued,
    /// Being prepared: started at, expected seconds.
    Working(Instant, f32),
    Ready(Box<(PathBuf, Manifest)>),
    Failed(String),
}

impl State {
    /// 0..1 for a progress ring (time against the expected duration while
    /// working, never full before it is done).
    pub fn progress(&self) -> f32 {
        match self {
            State::Queued => 0.0,
            State::Working(since, secs) => {
                (since.elapsed().as_secs_f32() / secs.max(0.1)).clamp(0.0, 0.95)
            }
            State::Ready(_) => 1.0,
            State::Failed(_) => 0.0,
        }
    }

    pub fn done(&self) -> bool {
        matches!(self, State::Ready(_) | State::Failed(_))
    }
}

struct Item {
    stage: bool,
    entry: Entry,
    state: State,
    /// Expected seconds of work.
    estimate: f32,
}

#[derive(Default)]
struct Shared {
    items: Vec<Item>,
    /// Indices into `items`, next first.
    queue: VecDeque<usize>,
}

pub struct Preparer {
    shared: Mutex<Shared>,
    wake: Condvar,
    lib: Library,
}

static PREPARER: OnceLock<Arc<Preparer>> = OnceLock::new();

/// The running preparer (the full game; `rugen play` and the tools prepare
/// what they need on the spot).
pub fn get() -> Option<&'static Arc<Preparer>> {
    PREPARER.get()
}

/// Worker threads and encoding threads per conversion for this machine:
/// one conversion at a time, spread over every core but two (kept for the
/// game's own frames). A conversion already fills the cores it gets: two
/// at once measured no faster on 12 cores (86 G6 fighters: 6.1 s each
/// either way) and peaked 0.5 GB higher.
fn budget() -> (usize, usize) {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    (1, cores.saturating_sub(2).max(1))
}

/// Seconds an entry is expected to take: converting an archive goes at
/// roughly 8 MB of archive a second here (plus extraction), describing and
/// verifying a content file well under one.
fn estimate(entry: &Entry) -> f32 {
    let size = std::fs::metadata(&entry.source).map_or(0, |m| m.len()) as f32;
    match entry.kind {
        Kind::Zip | Kind::Rar => 1.0 + size / 8.0e6,
        Kind::Dir => 2.0,
        Kind::Content => 0.4,
    }
}

/// Start preparing everything in `lib` in the background.
pub fn start(lib: &Library) -> Arc<Preparer> {
    if let Some(p) = PREPARER.get() {
        return p.clone();
    }
    let (workers, per) = budget();
    rugen_formats::set_encode_threads(per);
    let p = Arc::new(Preparer {
        shared: Mutex::new(Shared::default()),
        wake: Condvar::new(),
        lib: lib.clone(),
    });
    let _ = PREPARER.set(p.clone());
    let p = PREPARER.get().cloned().unwrap_or(p);
    {
        let mut s = p.lock();
        // stages first: a handful, mostly quick, and every fight needs one
        for (stage, entries) in [(true, lib.stages()), (false, lib.chars())] {
            for entry in entries {
                let estimate = estimate(&entry);
                let n = s.items.len();
                s.queue.push_back(n);
                s.items.push(Item {
                    stage,
                    entry,
                    state: State::Queued,
                    estimate,
                });
            }
        }
        log::info!(
            "preparing {} fighters and stages in the background ({workers} at a time, {per} threads each)",
            s.items.len()
        );
    }
    for k in 0..workers {
        let p = p.clone();
        let _ = std::thread::Builder::new()
            .name(format!("prepare-{k}"))
            .spawn(move || {
                lower_priority();
                p.work();
            });
    }
    p
}

/// Run this thread (and the encoder threads it starts, which inherit it)
/// below the game's priority so menus stay smooth while converting.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    nice::lower();
}

#[cfg(target_os = "linux")]
mod nice {
    #![allow(unsafe_code)]

    /// Linux keeps a nice value per thread: 10 for this one.
    pub fn lower() {
        // SAFETY: setpriority only reads its integer arguments; who = 0 is
        // the calling thread on Linux
        let r = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
        if r != 0 {
            log::debug!("could not lower the preparation thread's priority");
        }
    }
}

impl Preparer {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn work(&self) {
        loop {
            let (i, stage, entry) = {
                let mut s = self.lock();
                let i = loop {
                    if let Some(i) = s.queue.pop_front() {
                        break i;
                    }
                    s = self.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                };
                let it = &mut s.items[i];
                it.state = State::Working(Instant::now(), it.estimate);
                (i, it.stage, it.entry.clone())
            };
            let started = Instant::now();
            let result = if stage {
                self.lib.prepare_stage(&entry)
            } else {
                crate::verify::prepare_verified(&self.lib, &entry)
            };
            let state = match result {
                Ok((dir, m)) => State::Ready(Box::new((dir, m))),
                Err(e) => {
                    log::warn!("{}: {e}", entry.id);
                    State::Failed(e.to_string())
                }
            };
            log::debug!(
                "prepared {} in {} ms",
                entry.id,
                started.elapsed().as_millis()
            );
            let mut s = self.lock();
            s.items[i].state = state;
            if s.items.iter().all(|it| it.state.done()) {
                log::info!(
                    "timing: everything prepared {} ms after launch",
                    crate::timing::since_launch()
                );
            }
            // a finished conversion replaced its archive with a content file
            if let State::Ready(r) = &s.items[i].state
                && let Some(c) = r.1.content.clone()
                && s.items[i].entry.kind != Kind::Content
                && s.items[i].entry.kind != Kind::Dir
            {
                s.items[i].entry = Entry {
                    id: s.items[i].entry.id.clone(),
                    source: c,
                    kind: Kind::Content,
                };
            }
        }
    }

    fn find(s: &Shared, stage: bool, id: &str) -> Option<usize> {
        s.items
            .iter()
            .position(|it| it.stage == stage && it.entry.id.eq_ignore_ascii_case(id))
    }

    /// The state of fighter (or stage) `id`; `None` when it is not in the
    /// library (added after launch: prepare it on the spot).
    pub fn state(&self, stage: bool, id: &str) -> Option<State> {
        let s = self.lock();
        Self::find(&s, stage, id).map(|i| s.items[i].state.clone())
    }

    /// Prepare `id` next (a fighter just picked, a stage chosen).
    pub fn prioritize(&self, stage: bool, id: &str) {
        let mut s = self.lock();
        if let Some(i) = Self::find(&s, stage, id)
            && let Some(at) = s.queue.iter().position(|&q| q == i)
        {
            s.queue.remove(at);
            s.queue.push_front(i);
        }
    }

    /// Fighters (or stages) ready / in the library.
    pub fn counts(&self, stage: bool) -> (usize, usize) {
        let s = self.lock();
        let all = s.items.iter().filter(|it| it.stage == stage);
        let total = all.clone().count();
        let ready = all.filter(|it| it.state.done()).count();
        (ready, total)
    }

    /// Everything (fighters and stages) prepared or failed.
    pub fn all_done(&self) -> bool {
        self.lock().items.iter().all(|it| it.state.done())
    }

    /// Every entry of one kind with its state, in library order.
    pub fn entries(&self, stage: bool) -> Vec<(Entry, State)> {
        self.lock()
            .items
            .iter()
            .filter(|it| it.stage == stage)
            .map(|it| (it.entry.clone(), it.state.clone()))
            .collect()
    }
}

/// A fighter's prepared files: from the preparer when it runs (`None`
/// while it is still being prepared, after asking for it next), else
/// prepared and verified on the spot.
pub fn ready_char(lib: &Library, entry: &Entry) -> Option<(PathBuf, Manifest)> {
    match get().and_then(|p| p.state(false, &entry.id).map(|s| (p, s))) {
        Some((_, State::Ready(r))) => Some(*r),
        Some((_, State::Failed(_))) => None,
        Some((p, _)) => {
            p.prioritize(false, &entry.id);
            None
        }
        None => crate::verify::prepare_verified(lib, entry).ok(),
    }
}

/// [`ready_char`] for a stage.
pub fn ready_stage(lib: &Library, entry: &Entry) -> Option<(PathBuf, Manifest)> {
    match get().and_then(|p| p.state(true, &entry.id).map(|s| (p, s))) {
        Some((_, State::Ready(r))) => Some(*r),
        Some((_, State::Failed(_))) => None,
        Some((p, _)) => {
            p.prioritize(true, &entry.id);
            None
        }
        None => lib.prepare_stage(entry).ok(),
    }
}

/// A fight setup's stand-in path for a fighter or stage still being
/// prepared: resolved to its def before the fight loads
/// ([`resolve_pending`]); never a real file.
pub fn pending_path(stage: bool, id: &str) -> PathBuf {
    PathBuf::from(format!(
        "pending:{}:{id}",
        if stage { "stage" } else { "char" }
    ))
}

/// `(stage, id)` of a [`pending_path`].
pub fn pending_of(p: &Path) -> Option<(bool, String)> {
    let s = p.to_str()?.strip_prefix("pending:")?;
    let (kind, id) = s.split_once(':')?;
    Some((kind == "stage", id.to_string()))
}

/// What a fight setup still waits for: `(stage, id, progress)` of each
/// pending entry (empty = ready to load).
pub fn waiting_for(setup: &crate::play::FightSetup) -> Vec<(bool, String, f32)> {
    let mut paths: Vec<&Path> = vec![&setup.p1, &setup.p2];
    if let Some(s) = &setup.stage {
        paths.push(s);
    }
    if let Some(t) = &setup.teams {
        for side in &t.partners {
            paths.extend(side.iter().map(PathBuf::as_path));
        }
    }
    let mut out = Vec::new();
    for p in paths {
        let Some((stage, id)) = pending_of(p) else {
            continue;
        };
        let st = get().and_then(|pr| pr.state(stage, &id));
        match st {
            Some(State::Ready(_)) => {}
            Some(State::Failed(_)) | None => {}
            Some(s) => {
                if let Some(pr) = get() {
                    pr.prioritize(stage, &id);
                }
                out.push((stage, id, s.progress()));
            }
        }
    }
    out
}

/// Replace a setup's pending paths with the prepared defs (call once
/// [`waiting_for`] is empty). `false` when one failed.
pub fn resolve_pending(setup: &mut crate::play::FightSetup) -> bool {
    let mut ok = true;
    let mut fix = |p: &mut PathBuf, id_out: Option<&mut String>| {
        let Some((stage, id)) = pending_of(p) else {
            return;
        };
        match get().and_then(|pr| pr.state(stage, &id)) {
            Some(State::Ready(r)) => {
                *p = r.1.def_path(&r.0);
                if let Some(o) = id_out {
                    *o = r.1.id.clone();
                }
            }
            _ => ok = false,
        }
    };
    let [id1, id2] = &mut setup.ids;
    fix(&mut setup.p1, Some(id1));
    fix(&mut setup.p2, Some(id2));
    if let Some(s) = setup.stage.as_mut() {
        fix(s, None);
        if pending_of(s).is_some() {
            setup.stage = None;
        }
    }
    if let Some(t) = setup.teams.as_mut() {
        for side in &mut t.partners {
            for p in side.iter_mut() {
                fix(p, None);
            }
        }
    }
    ok
}

/// Load a fight's fighters (and read its stage) on a helper thread while
/// the versus screen shows, so the fight starts without reading or parsing
/// anything: [`rugen_sim::CharData::preload`] keeps them for the fight's
/// own load. Pending entries are skipped (nothing to read yet).
pub fn preload(setup: &crate::play::FightSetup, data_dir: &Path) {
    let mut chars: Vec<PathBuf> = vec![setup.p1.clone()];
    if setup.p2 != setup.p1 {
        chars.push(setup.p2.clone());
    }
    if let Some(t) = &setup.teams {
        for side in &t.partners {
            chars.extend(side.iter().cloned());
        }
    }
    chars.retain(|p| pending_of(p).is_none() && !p.as_os_str().is_empty());
    let stage = setup.stage.clone().filter(|p| pending_of(p).is_none());
    let data_dir = data_dir.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("preload".into())
        .spawn(move || {
            let t = Instant::now();
            for c in &chars {
                rugen_sim::CharData::preload(c, &data_dir);
            }
            // the stage's files into the page cache (its sprites decode on draw)
            if let Some(def) = stage
                && let Ok(d) = rugen_formats::def::Def::open(&def)
            {
                let _ = rugen_formats::vfs::read(&def);
                if let Some(spr) = d.section("bgdef").and_then(|b| b.raw("spr"))
                    && let Ok(p) = rugen_sim::chardata::resolve(
                        def.parent().unwrap_or(Path::new(".")),
                        spr.trim(),
                    )
                {
                    let _ = rugen_formats::vfs::read(&p);
                }
            }
            log::info!(
                "timing: preloading the fight took {} ms",
                t.elapsed().as_millis()
            );
        });
}

/// A placeholder manifest for a fighter or stage not prepared yet: its id
/// as the name, nothing else.
pub fn placeholder(entry: &Entry, stage: bool) -> Manifest {
    Manifest {
        cache_version: 0,
        id: entry.id.clone(),
        def: String::new(),
        display_name: entry.id.replace('_', " "),
        author: String::new(),
        portrait_small: None,
        portrait_big: None,
        source_hash: String::new(),
        verify: None,
        art: None,
        thumb: None,
        fingerprint: None,
        package_def: None,
        content_size: 0,
        profile: None,
        card: None,
        fair_error: None,
        stage,
        min_reader: 0,
        content: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_paths_round_trip() {
        let p = pending_path(false, "G6_Naruto");
        assert_eq!(pending_of(&p), Some((false, "G6_Naruto".to_string())));
        assert_eq!(
            pending_of(&pending_path(true, "a:b")),
            Some((true, "a:b".into()))
        );
        assert_eq!(pending_of(Path::new("chars/x.rgc/x.def")), None);
    }

    #[test]
    fn progress_never_fills_before_done() {
        let s = State::Working(Instant::now() - std::time::Duration::from_secs(60), 1.0);
        assert!(s.progress() < 1.0);
        assert_eq!(State::Queued.progress(), 0.0);
    }
}
