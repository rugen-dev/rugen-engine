//! The updater's state machine: a check thread and an apply thread write a
//! shared [`Snapshot`] the menus read every frame (never blocking the UI)
//! and `rugen update` polls from the terminal.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{ArchiveKind, CheckCache, Install, Release, apply, net};

/// Where the updater stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No check done yet (disabled, or the cache is fresh and empty).
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists (`Snapshot::latest`).
    Available,
    Downloading,
    Verifying,
    Installing,
    /// Installed: restart to run it.
    Ready,
    /// The last check or install failed (`Snapshot::error`).
    Failed,
}

impl Phase {
    /// A thread is working.
    pub fn busy(self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading | Self::Verifying | Self::Installing
        )
    }
}

/// A copy of the updater's state.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub phase: Phase,
    /// The running version.
    pub current: String,
    /// The newest release known (from the last check, or the cache).
    pub latest: Option<Release>,
    /// Download progress, 0..1 (1 once past the download).
    pub progress: f32,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// Bumped on every change: screens refresh when it moves.
    pub generation: u64,
    /// Unix seconds of the last successful check (0: never).
    pub checked_at: u64,
}

impl Snapshot {
    /// A newer release than the running one is known.
    pub fn newer(&self) -> Option<&Release> {
        self.latest
            .as_ref()
            .filter(|r| crate::is_newer(&r.version, &self.current))
    }
}

struct State {
    snap: Snapshot,
}

/// One updater per process: the install it would replace, the cache file
/// and the release URL it asks.
pub struct Updater {
    state: Mutex<State>,
    install: Install,
    cache_path: PathBuf,
    url: String,
}

impl Updater {
    pub fn new(current: &str, install: Install, cache_path: PathBuf, url: String) -> Arc<Self> {
        let cache = CheckCache::load(&cache_path);
        let phase = match &cache.latest {
            Some(r) if crate::is_newer(&r.version, current) => Phase::Available,
            Some(_) => Phase::UpToDate,
            None => Phase::Idle,
        };
        Arc::new(Self {
            state: Mutex::new(State {
                snap: Snapshot {
                    phase,
                    current: current.to_string(),
                    latest: cache.latest,
                    progress: 0.0,
                    received: 0,
                    total: None,
                    error: None,
                    generation: 0,
                    checked_at: cache.checked_at,
                },
            }),
            install,
            cache_path,
            url,
        })
    }

    pub fn install(&self) -> &Install {
        &self.install
    }

    /// The release URL asked.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .map(|s| s.snap.clone())
            .unwrap_or_else(|e| e.into_inner().snap.clone())
    }

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s.snap);
        s.snap.generation += 1;
    }

    /// A check is due (cache older than [`crate::CHECK_EVERY_SECS`]).
    pub fn due(&self) -> bool {
        let at = self.snapshot().checked_at;
        CheckCache {
            checked_at: at,
            latest: None,
        }
        .due(crate::now_secs())
    }

    /// Ask GitHub in the background. `force` ignores the cache's age.
    /// Returns false when nothing was started (fresh cache, already busy).
    pub fn check(self: &Arc<Self>, force: bool) -> bool {
        if self.snapshot().phase.busy() || (!force && !self.due()) {
            return false;
        }
        self.update(|s| {
            s.phase = Phase::Checking;
            s.error = None;
        });
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || me.check_now());
        if spawned.is_err() {
            self.update(|s| {
                s.phase = Phase::Failed;
                s.error = Some("could not start the check".into());
            });
            return false;
        }
        true
    }

    /// The check, on this thread.
    pub fn check_now(&self) {
        self.update(|s| s.phase = Phase::Checking);
        match net::fetch_latest(&self.url) {
            Ok(release) => {
                let now = crate::now_secs();
                CheckCache {
                    checked_at: now,
                    latest: Some(release.clone()),
                }
                .save(&self.cache_path);
                log::info!(
                    "update check: latest {} ({}), running {}",
                    release.version,
                    release.date,
                    self.snapshot().current
                );
                self.update(|s| {
                    s.phase = if crate::is_newer(&release.version, &s.current) {
                        Phase::Available
                    } else {
                        Phase::UpToDate
                    };
                    s.latest = Some(release);
                    s.checked_at = now;
                    s.error = None;
                });
            }
            Err(e) => {
                log::warn!("update check: {e:#}");
                self.update(|s| {
                    s.phase = Phase::Failed;
                    s.error = Some(format!("{e:#}"));
                });
            }
        }
    }

    /// Why UPDATE NOW cannot run here, if it cannot.
    pub fn blocker(&self) -> Option<String> {
        let snap = self.snapshot();
        let release = snap.newer()?;
        match &self.install {
            Install::Package(_) => Some("installed by your package manager".into()),
            Install::Dev(_) => Some("not a release folder".into()),
            Install::Portable(dir) => {
                if release.own_archive().is_none() {
                    Some("no build for this machine in the release".into())
                } else if release.asset(crate::SUMS_ASSET).is_none() {
                    Some("release has no SHA256SUMS".into())
                } else if !crate::writable(dir) {
                    Some("install folder is read-only".into())
                } else {
                    None
                }
            }
        }
    }

    /// Download, verify, unpack and swap in the newer release, in the
    /// background. False when nothing was started ([`Updater::blocker`],
    /// busy, nothing newer).
    pub fn start_apply(self: &Arc<Self>) -> bool {
        if self.snapshot().phase.busy() || self.blocker().is_some() {
            return false;
        }
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("update-apply".into())
            .spawn(move || me.apply_now())
            .is_ok()
    }

    /// The whole apply, on this thread.
    pub fn apply_now(&self) {
        match self.apply_inner() {
            Ok(()) => self.update(|s| {
                s.phase = Phase::Ready;
                s.progress = 1.0;
                s.error = None;
            }),
            Err(e) => {
                log::error!("update: {e:#}");
                self.update(|s| {
                    s.phase = Phase::Failed;
                    s.error = Some(format!("{e:#}"));
                });
            }
        }
    }

    fn apply_inner(&self) -> anyhow::Result<()> {
        let snap = self.snapshot();
        let release = snap
            .newer()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no newer release known"))?;
        let Install::Portable(dir) = &self.install else {
            anyhow::bail!("not a portable install");
        };
        let archive = release
            .own_archive()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no archive for this machine"))?;
        let kind = ArchiveKind::of(&archive.name)
            .ok_or_else(|| anyhow::anyhow!("{}: unknown archive type", archive.name))?;
        let sums = release
            .asset(crate::SUMS_ASSET)
            .ok_or_else(|| anyhow::anyhow!("release has no {}", crate::SUMS_ASSET))?;
        let stage = dir.join(apply::STAGE_DIR);
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)?;
        let result = self.stage_and_swap(dir, &stage, &release, &archive, kind, &sums.url);
        let _ = std::fs::remove_dir_all(&stage);
        result
    }

    fn stage_and_swap(
        &self,
        dir: &Path,
        stage: &Path,
        release: &Release,
        archive: &crate::Asset,
        kind: ArchiveKind,
        sums_url: &str,
    ) -> anyhow::Result<()> {
        self.update(|s| {
            s.phase = Phase::Downloading;
            s.progress = 0.0;
            s.received = 0;
            s.total = Some(archive.size).filter(|n| *n > 0);
        });
        // the checksums first: a release without ours is refused before
        // the big download
        let sums = net::fetch_text(sums_url, 1024 * 1024)?;
        let expected = crate::expected_sha(&sums, &archive.name).ok_or_else(|| {
            anyhow::anyhow!("{} has no entry for {}", crate::SUMS_ASSET, archive.name)
        })?;
        let file = stage.join(&archive.name);
        let mut last = 0u64;
        let actual = net::download(
            &archive.url,
            &file,
            crate::MAX_DOWNLOAD,
            &mut |got, total| {
                // a snapshot per 256 KiB (and the end): the menus redraw at 60 Hz anyway
                if got - last >= 256 * 1024 || Some(got) == total {
                    last = got;
                    self.update(|s| {
                        s.received = got;
                        s.total = total.or(s.total);
                        s.progress = match s.total {
                            Some(t) if t > 0 => (got as f64 / t as f64).min(1.0) as f32,
                            _ => 0.0,
                        };
                    });
                }
            },
        )?;
        self.update(|s| {
            s.phase = Phase::Verifying;
            s.progress = 1.0;
        });
        anyhow::ensure!(
            actual == expected,
            "{} does not match {}: expected {expected}, got {actual}",
            archive.name,
            crate::SUMS_ASSET
        );
        let tree = stage.join("tree");
        apply::extract(&file, kind, &tree)?;
        let _ = std::fs::remove_file(&file);
        self.update(|s| s.phase = Phase::Installing);
        apply::apply(dir, &tree)?;
        log::info!(
            "update: {} installed in {} (restart to run it)",
            release.version,
            dir.display()
        );
        Ok(())
    }

    /// Start the installed binary with `args`; the caller then quits.
    pub fn relaunch(&self, args: &[String]) -> anyhow::Result<()> {
        let exe = self.install.dir().join(crate::exe_name());
        apply::relaunch(&exe, args)
    }
}
