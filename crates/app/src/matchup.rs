//! Client side of the "bring your own character" matchup, shared by the
//! ONLINE screens and `rugen play --server`:
//!
//! ```text
//! pick     declare our fighter (fingerprint + fairness profile + card; P1
//!          also the stage) → the server answers FAIR / UNBALANCED / BLOCKED
//!          at once; from now on our .rgc / .rgs uploads in the background when
//!          the server lacks it (resumable; the server re-hashes it)
//! confirm  both players accept (UNBALANCED explicitly, BLOCKED never)
//! sync     download what we lack into cache/net, report ready (files,
//!          rules, game version)
//! started  the relay ticket arrives: fight with the synced files
//! ```
//!
//! Everything runs on a worker thread; the UI reads [`Shared`] snapshots.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rugen_content::Library;
use rugen_net::online::{
    FighterDecl, MatchupView, OnlineClient, ReadyReport, SessionTicket, Transfer, VerdictKind,
};

/// Largest decompressed size of a downloaded content file (bombs stop here).
const MAX_UNPACKED: u64 = 1 << 30;

/// First 12 hex digits of a fingerprint, for logs.
pub fn short(fp: &str) -> String {
    fp.chars().take(12).collect()
}

/// Game version both sides must agree on.
pub fn engine() -> String {
    format!("rugen {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XferState {
    /// Asking the server / building the package.
    Checking,
    Running,
    /// Already on the server (upload) or in our cache (download): instant.
    Cached,
    Done,
    Failed(String),
}

/// One package transfer the UI shows as a bar.
#[derive(Clone, Debug)]
pub struct Xfer {
    pub label: String,
    pub fingerprint: String,
    pub progress: Arc<Transfer>,
    pub state: XferState,
}

impl Xfer {
    fn new(label: &str, fp: &str) -> Self {
        Self {
            label: label.to_string(),
            fingerprint: fp.to_string(),
            progress: Transfer::new(),
            state: XferState::Checking,
        }
    }

    pub fn fraction(&self) -> f32 {
        match self.state {
            XferState::Cached | XferState::Done => 1.0,
            _ => self.progress.fraction(),
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.state, XferState::Cached | XferState::Done)
    }
}

/// The match is on: files, ticket and the final view.
#[derive(Clone, Debug)]
pub struct Ready {
    /// P1, P2 and stage .def (local or downloaded into cache/net).
    pub defs: [PathBuf; 3],
    pub ticket: SessionTicket,
    pub view: MatchupView,
}

/// What the worker publishes for the UI.
#[derive(Clone, Debug, Default)]
pub struct Shared {
    pub view: Option<MatchupView>,
    /// Importing / fingerprinting our pick.
    pub preparing: bool,
    /// Our declaration (with the local card path for the UI).
    pub mine: Option<FighterDecl>,
    pub mine_card: Option<PathBuf>,
    pub uploads: Vec<Xfer>,
    pub downloads: Vec<Xfer>,
    /// We sent CONFIRM for the current verdict.
    pub accepted: bool,
    /// A confirmation was dropped because the verdict changed.
    pub verdict_changed: bool,
    /// Non-fatal message (refused confirm, retrying...).
    pub notice: String,
    /// Fatal: the matchup is over (back to the room).
    pub error: Option<String>,
    pub ready: Option<Ready>,
}

impl Shared {
    pub fn verdict(&self) -> Option<VerdictKind> {
        self.view
            .as_ref()
            .and_then(|v| v.verdict.as_ref())
            .map(|v| v.kind)
    }
}

enum Cmd {
    Pick {
        char_id: String,
        stage_id: Option<String>,
    },
    Accept,
    Leave,
}

/// A matchup in progress (one worker thread).
pub struct Driver {
    pub id: u64,
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
}

impl Driver {
    /// Start following matchup `id` (we are P`side`, 1 or 2; 3 = spectator).
    pub fn start(client: OnlineClient, lib: Library, id: u64, preferred_stage: String) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let s = shared.clone();
        std::thread::Builder::new()
            .name(format!("matchup-{id}"))
            .spawn(move || {
                Worker {
                    client,
                    lib,
                    id,
                    preferred_stage,
                    shared: s,
                    view: None,
                    mine: Vec::new(),
                    downloading: HashSet::new(),
                    ready_sent: false,
                }
                .run(rx)
            })
            .ok();
        Self { id, tx, shared }
    }

    pub fn pick(&self, char_id: &str, stage_id: Option<String>) {
        if let Ok(mut s) = self.shared.lock() {
            s.preparing = true;
            s.notice.clear();
        }
        let _ = self.tx.send(Cmd::Pick {
            char_id: char_id.to_string(),
            stage_id,
        });
    }

    pub fn accept(&self) {
        let _ = self.tx.send(Cmd::Accept);
    }

    pub fn leave(&self) {
        let _ = self.tx.send(Cmd::Leave);
    }

    pub fn snapshot(&self) -> Shared {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

struct Worker {
    client: OnlineClient,
    lib: Library,
    id: u64,
    preferred_stage: String,
    shared: Arc<Mutex<Shared>>,
    view: Option<MatchupView>,
    /// Our declarations (fighter, and the stage as P1): (decl, prepared dir).
    mine: Vec<(FighterDecl, PathBuf, rugen_content::Manifest)>,
    downloading: HashSet<String>,
    ready_sent: bool,
}

impl Worker {
    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }

    fn fatal(&self, msg: String) {
        log::warn!("matchup {}: {msg}", self.id);
        // stop our transfers
        self.with(|s| {
            for x in s.uploads.iter().chain(&s.downloads) {
                x.progress.cancel.store(true, Ordering::Relaxed);
            }
            if s.error.is_none() {
                s.error = Some(msg);
            }
        });
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let mut last_poll = Instant::now() - Duration::from_secs(5);
        loop {
            match rx.recv_timeout(Duration::from_millis(120)) {
                Ok(Cmd::Pick { char_id, stage_id }) => self.pick(&char_id, stage_id),
                Ok(Cmd::Accept) => self.accept(),
                Ok(Cmd::Leave) | Err(RecvTimeoutError::Disconnected) => {
                    if self.with(|s| s.ready.is_none()) {
                        let _ = self.client.leave_matchup(self.id);
                    }
                    self.with(|s| {
                        for x in s.uploads.iter().chain(&s.downloads) {
                            x.progress.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.with(|s| s.error.is_some() || s.ready.is_some()) {
                // finished: idle until the UI drops us (a Leave after an
                // error is harmless, the server already failed the matchup)
                continue;
            }
            if last_poll.elapsed() >= Duration::from_millis(400) {
                last_poll = Instant::now();
                match self.client.matchup(self.id) {
                    Ok(v) => self.update(v),
                    Err(e) => self.with(|s| s.notice = format!("server: {e}")),
                }
            }
            self.advance();
        }
    }

    fn update(&mut self, v: MatchupView) {
        let side = usize::from(v.side == 2);
        self.with(|s| {
            // the server drops confirmations when the verdict changes
            if s.accepted && v.side <= 2 && !v.accepted[side] && v.phase == "confirm" {
                s.accepted = false;
                s.verdict_changed = true;
            }
            if v.accepted[side] {
                s.verdict_changed = false;
            }
            s.view = Some(v.clone());
        });
        if v.phase == "failed" {
            let why = if v.error.is_empty() {
                "the matchup failed".to_string()
            } else {
                v.error.clone()
            };
            self.fatal(why);
        }
        // mirror rule: P2 brings nothing
        if v.mirror && v.side == 2 && v.fighters[1].is_none() && v.phase == "pick" {
            let _ = self.client.pick(self.id, None, None);
        }
        self.view = Some(v);
    }

    fn pick(&mut self, char_id: &str, stage_id: Option<String>) {
        // which side we are decides whether we bring the stage
        if self.view.is_none() {
            match self.client.matchup(self.id) {
                Ok(v) => self.update(v),
                Err(e) => {
                    self.with(|s| {
                        s.preparing = false;
                        s.notice = format!("server: {e}");
                    });
                    return;
                }
            }
        }
        let r = self.prepare_pick(char_id, stage_id);
        self.with(|s| s.preparing = false);
        let (decls, card) = match r {
            Ok(x) => x,
            Err(e) => {
                // a local problem: stay on the matchup, let the player re-pick
                log::warn!("matchup {}: {e}", self.id);
                self.with(|s| s.notice = e);
                return;
            }
        };
        let fighter = decls.first().map(|d| &d.0);
        let stage = decls.get(1).map(|d| &d.0);
        let t = Instant::now();
        match self.client.pick(self.id, fighter, stage) {
            Ok(v) => {
                log::info!(
                    "matchup {}: picked {} ({}), verdict {:?} in {:?}",
                    self.id,
                    fighter.map(|f| f.name.as_str()).unwrap_or("-"),
                    fighter.map(|f| short(&f.fingerprint)).unwrap_or_default(),
                    v.verdict.as_ref().map(|v| v.kind),
                    t.elapsed()
                );
                self.with(|s| {
                    s.mine = fighter.cloned();
                    s.mine_card = card;
                    s.accepted = false;
                    s.verdict_changed = false;
                    s.notice.clear();
                });
                self.update(v);
            }
            Err(e) => {
                self.with(|s| s.notice = format!("pick refused: {e}"));
                return;
            }
        }
        self.mine = decls;
        self.start_uploads();
    }

    /// Prepare (import + verify + fingerprint) our fighter and, as P1, the stage.
    #[allow(clippy::type_complexity)]
    fn prepare_pick(
        &self,
        char_id: &str,
        stage_id: Option<String>,
    ) -> Result<
        (
            Vec<(FighterDecl, PathBuf, rugen_content::Manifest)>,
            Option<PathBuf>,
        ),
        String,
    > {
        let entry = self
            .lib
            .find_char(char_id)
            .ok_or_else(|| format!("{char_id} is not installed"))?;
        let (dir, m) = crate::verify::prepare_verified(&self.lib, &entry)
            .map_err(|e| format!("{char_id}: {e}"))?;
        let decl = self.lib.declare(&dir, &m).ok_or_else(|| {
            format!(
                "{char_id} cannot go online: {}",
                m.fair_error
                    .clone()
                    .unwrap_or_else(|| "no fingerprint".into())
            )
        })?;
        let card = m.card.as_ref().map(|c| dir.join(c));
        let mut out = vec![(decl, dir, m)];
        let p1 = self.view.as_ref().is_none_or(|v| v.side == 1);
        if p1 {
            let stages = self.lib.stages();
            let want = stage_id
                .or_else(|| Some(self.preferred_stage.clone()).filter(|s| !s.trim().is_empty()))
                .unwrap_or_else(|| "training".into());
            let e = stages
                .iter()
                .find(|e| e.id.eq_ignore_ascii_case(&want))
                .or_else(|| stages.first())
                .ok_or("no stage installed in stages/")?;
            let (sdir, sm) = self
                .lib
                .prepare_stage(e)
                .map_err(|err| format!("stage {}: {err}", e.id))?;
            let sd = self
                .lib
                .declare(&sdir, &sm)
                .ok_or_else(|| format!("stage {} cannot go online", e.id))?;
            out.push((sd, sdir, sm));
        }
        Ok((out, card))
    }

    /// From pick time: upload whatever the server lacks, in the background.
    fn start_uploads(&mut self) {
        let mut rows = Vec::new();
        for (i, (d, dir, m)) in self.mine.iter().enumerate() {
            if d.profile.as_ref().is_some_and(|p| p.blocked()) {
                // never ship what can never be played
                continue;
            }
            let label = if i == 0 { "YOUR FIGHTER" } else { "STAGE" };
            let x = Xfer::new(label, &d.fingerprint);
            rows.push(x.clone());
            let (c, lib, fp, dir, m, shared) = (
                self.client.clone(),
                self.lib.clone(),
                d.fingerprint.clone(),
                dir.clone(),
                m.clone(),
                self.shared.clone(),
            );
            std::thread::spawn(move || {
                let set = |st: XferState| {
                    if let Ok(mut s) = shared.lock()
                        && let Some(x) = s.uploads.iter_mut().find(|x| x.fingerprint == fp)
                    {
                        x.state = st;
                    }
                };
                let r = (|| -> Result<bool, String> {
                    if c.pkg_status(&fp).map_err(|e| e.to_string())?.complete {
                        return Ok(true);
                    }
                    let path = lib.package(&dir, &m).map_err(|e| e.to_string())?;
                    set(XferState::Running);
                    let t = Instant::now();
                    let st = c
                        .upload_package(&fp, &path, &x.progress)
                        .map_err(|e| e.to_string())?;
                    log::info!(
                        "upload {}: {} KB in {:?}",
                        short(&fp),
                        st.size >> 10,
                        t.elapsed()
                    );
                    Ok(false)
                })();
                match r {
                    Ok(true) => {
                        log::info!("upload {}: already on the server", short(&fp));
                        set(XferState::Cached)
                    }
                    Ok(false) => set(XferState::Done),
                    Err(e) => {
                        log::warn!("upload {}: {e}", short(&fp));
                        set(XferState::Failed(e))
                    }
                }
            });
        }
        self.with(|s| s.uploads = rows);
    }

    fn accept(&mut self) {
        let unbalanced = self
            .view
            .as_ref()
            .and_then(|v| v.verdict.as_ref())
            .is_some_and(|v| v.kind == VerdictKind::Unbalanced);
        match self.client.accept(self.id, unbalanced) {
            Ok(v) => {
                self.with(|s| {
                    s.accepted = true;
                    s.verdict_changed = false;
                    s.notice.clear();
                });
                self.update(v);
            }
            Err(e) => self.with(|s| s.notice = e.to_string()),
        }
    }

    fn advance(&mut self) {
        let Some(v) = self.view.clone() else { return };
        // an upload that failed ends the matchup: the opponent could never get it
        let failed = self.with(|s| {
            s.uploads.iter().find_map(|x| match &x.state {
                XferState::Failed(e) => Some(format!("could not upload {}: {e}", x.label)),
                _ => None,
            })
        });
        if let Some(msg) = failed {
            let _ = self.client.leave_matchup(self.id);
            self.fatal(msg.to_lowercase());
            return;
        }
        if !matches!(v.phase.as_str(), "sync" | "started") {
            return;
        }
        let fps = v.fingerprints();
        let mut all_local = true;
        for (i, fp) in fps.iter().enumerate() {
            let Some(fp) = fp else {
                all_local = false;
                continue;
            };
            if self.lib.find_fingerprint(fp).is_some() {
                if !self.downloading.contains(*fp) && i < 2 && v.side as usize != i + 1 {
                    // the opponent's fighter is already in our cache
                    self.downloading.insert(fp.to_string());
                    let label = if i < 2 { "OPPONENT" } else { "STAGE" };
                    let mut x = Xfer::new(label, fp);
                    x.state = XferState::Cached;
                    self.with(|s| s.downloads.push(x));
                }
                continue;
            }
            all_local = false;
            if self.downloading.contains(*fp) || v.missing.iter().any(|m| m == fp) {
                continue;
            }
            let card = if i < 2 { &v.fighters[i] } else { &v.stage };
            let Some(size) = card.as_ref().map(|c| c.decl.size) else {
                continue;
            };
            self.downloading.insert(fp.to_string());
            let label = if i < 2 { "OPPONENT" } else { "STAGE" };
            let x = Xfer::new(label, fp);
            self.with(|s| s.downloads.push(x.clone()));
            self.spawn_download(x, size);
        }
        // a download that failed ends the matchup
        let failed = self.with(|s| {
            s.downloads.iter().find_map(|x| match &x.state {
                XferState::Failed(e) => Some(format!("could not download {}: {e}", x.label)),
                _ => None,
            })
        });
        if let Some(msg) = failed {
            let _ = self.client.leave_matchup(self.id);
            self.fatal(msg.to_lowercase());
            return;
        }
        if !all_local {
            return;
        }
        let fp = |i: usize| fps[i].unwrap_or_default().to_string();
        if !self.ready_sent && v.phase == "sync" && v.side <= 2 {
            let report = ReadyReport {
                p1: fp(0),
                p2: fp(1),
                stage: fp(2),
                engine: engine(),
                rules: v.rules.clone(),
            };
            match self.client.ready(self.id, &report) {
                Ok(nv) => {
                    log::info!("matchup {}: ready with every file", self.id);
                    self.ready_sent = true;
                    self.update(nv);
                }
                Err(e) => {
                    self.fatal(format!("ready refused: {e}"));
                    return;
                }
            }
        }
        let Some(v) = self.view.clone() else { return };
        if let Some(ticket) = v.ticket.clone() {
            let defs: Option<Vec<PathBuf>> = (0..3)
                .map(|i| self.lib.def_for_fingerprint(&fp(i)))
                .collect();
            match defs.and_then(|d| <[PathBuf; 3]>::try_from(d).ok()) {
                Some(defs) => {
                    log::info!(
                        "matchup {}: started, session {:08x}: {} vs {}",
                        self.id,
                        ticket.session,
                        defs[0].display(),
                        defs[1].display()
                    );
                    self.with(|s| {
                        s.ready = Some(Ready {
                            defs,
                            ticket,
                            view: v,
                        })
                    });
                }
                None => self.fatal("synced files vanished from the cache".into()),
            }
        }
    }

    fn spawn_download(&self, x: Xfer, size: u64) {
        let (c, lib, shared) = (self.client.clone(), self.lib.clone(), self.shared.clone());
        std::thread::spawn(move || {
            let fp = x.fingerprint.clone();
            let set = |st: XferState| {
                if let Ok(mut s) = shared.lock()
                    && let Some(r) = s.downloads.iter_mut().find(|r| r.fingerprint == fp)
                {
                    r.state = st;
                }
            };
            set(XferState::Running);
            let part = lib
                .cache_dir()
                .join("net/partial")
                .join(format!("{fp}.part"));
            let t = Instant::now();
            let r = c
                .download_package(&fp, size, &part, &x.progress)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    lib.install_package(&part, &fp, MAX_UNPACKED)
                        .map_err(|e| e.to_string())
                });
            let _ = std::fs::remove_file(&part);
            match r {
                Ok((dir, m)) => {
                    log::info!(
                        "download {}: {} ({} KB) in {:?} → {}",
                        short(&fp),
                        m.display_name,
                        size >> 10,
                        t.elapsed(),
                        dir.display()
                    );
                    set(XferState::Done)
                }
                Err(e) => {
                    log::warn!("download {}: {e}", short(&fp));
                    set(XferState::Failed(e))
                }
            }
        });
    }
}
