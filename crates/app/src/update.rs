//! The in-game updater's handle: one [`rugen_update::Updater`] per
//! process, started with the game, read by the title pill and the OPTIONS ›
//! UPDATE tab, driven by their actions and by `rugen update`.
//!
//! `[update] check = false` in rugen.toml or `--no-update-check` skip the
//! launch check; `RUGEN_UPDATE_URL` points the check elsewhere (tests).

use std::sync::{Arc, Mutex, OnceLock};

use rugen_content::Library;
use rugen_update::{Install, Phase, Snapshot, Updater};
use serde_json::{Value, json};

use crate::config::Config;
use crate::game::Transition;
use crate::lang::tf;

static UPDATER: OnceLock<Arc<Updater>> = OnceLock::new();
/// The arguments to start the new binary with after an update.
static ARGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Where the last opened link went (the rows say COPIED for a moment).
static OPENED: Mutex<Option<String>> = Mutex::new(None);

/// `cache/update.json` of the player's data.
fn cache_path(lib: &Library) -> std::path::PathBuf {
    lib.cache_dir().join("update.json")
}

/// The process's updater, made on the first call.
pub fn init(lib: &Library) -> Arc<Updater> {
    Arc::clone(UPDATER.get_or_init(|| {
        let install = Install::detect();
        log::info!("update: install {install:?}");
        Updater::new(
            env!("CARGO_PKG_VERSION"),
            install,
            cache_path(lib),
            rugen_update::latest_url(),
        )
    }))
}

pub fn get() -> Option<Arc<Updater>> {
    UPDATER.get().cloned()
}

/// Game start: remove the previous binary Windows could not delete and,
/// unless turned off, ask for the latest release (at most every six
/// hours) in the background.
pub fn launch(lib: &Library, config: &Config, args: &[String]) {
    let up = init(lib);
    if let Ok(mut a) = ARGS.lock() {
        *a = args.to_vec();
    }
    if let Install::Portable(dir) = up.install() {
        for p in rugen_update::cleanup_old(dir) {
            log::info!("update: removed {}", p.display());
        }
    }
    // harness runs (screenshots, CI) never call GitHub unless pointed at a server
    let harness = args.iter().any(|a| a == "--shot" || a == "--frames")
        && std::env::var_os("RUGEN_UPDATE_URL").is_none();
    if !config.update.check || harness || args.iter().any(|a| a == "--no-update-check") {
        log::info!("update: launch check off");
        return;
    }
    if up.check(false) {
        log::info!("update: checking {}", up.url());
    }
}

pub fn snapshot() -> Option<Snapshot> {
    get().map(|u| u.snapshot())
}

/// Bumps whenever the updater's state moves (screens refresh on it).
pub fn generation() -> u64 {
    snapshot().map_or(0, |s| s.generation)
}

/// The title pill: `{on, line}`.
pub fn title_json() -> Value {
    match snapshot().as_ref().and_then(|s| s.newer().cloned()) {
        Some(r) => {
            json!({ "on": true, "line": tf("update.update_version_available", &[("version", &r.version)]) })
        }
        None => json!({ "on": false, "line": "" }),
    }
}

/// Copy `url` to the clipboard and open it in the browser when there is one.
pub fn open_url(url: &str) {
    let copied = crate::clipboard::copy(url);
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let opened = std::process::Command::new(opener).arg(url).spawn().is_ok();
    log::info!("update: {url} (clipboard {copied}, browser {opened})");
    if let Ok(mut o) = OPENED.lock() {
        *o = Some(url.to_string());
    }
}

/// The link opened last (shown as COPIED on its row).
pub fn opened() -> Option<String> {
    OPENED.lock().ok().and_then(|o| o.clone())
}

/// The UPDATE tab's actions (`open:update_<what>`).
pub fn action(what: &str) -> Option<Transition> {
    let up = get()?;
    match what {
        "check" => {
            up.check(true);
        }
        "now" => {
            let snap = up.snapshot();
            if snap.phase == Phase::Ready {
                return restart();
            }
            if !up.start_apply() {
                log::info!("update: nothing started ({:?})", up.blocker());
            }
        }
        "restart" => return restart(),
        "notes" => {
            let url = up
                .snapshot()
                .latest
                .map(|r| r.url)
                .unwrap_or_else(|| rugen_update::NOTES_URL.to_string());
            open_url(&url);
        }
        "package" => {
            let snap = up.snapshot();
            let url = snap
                .newer()
                .and_then(|r| r.package(rugen_update::package_kind()?))
                .map(|a| a.url.clone())
                .or_else(|| snap.latest.as_ref().map(|r| r.url.clone()))
                .unwrap_or_else(|| rugen_update::NOTES_URL.to_string());
            open_url(&url);
        }
        _ => return None,
    }
    None
}

/// Start the installed binary with the game's arguments, then quit.
fn restart() -> Option<Transition> {
    let up = get()?;
    let args: Vec<String> = ARGS.lock().map(|a| a.clone()).unwrap_or_default();
    match up.relaunch(&args) {
        Ok(()) => Some(Transition::Quit),
        Err(e) => {
            log::error!("update: restart: {e:#}");
            None
        }
    }
}

/// `rugen update [--check] [--yes]`: check (and install) from the terminal.
pub fn cli(lib: &Library, args: &[String]) -> anyhow::Result<()> {
    let check_only = args.iter().any(|a| a == "--check");
    let up = init(lib);
    println!("rugen {}", env!("CARGO_PKG_VERSION"));
    println!("install: {}", describe(up.install()));
    up.check_now();
    let snap = up.snapshot();
    let Some(latest) = snap.latest.clone() else {
        anyhow::bail!(
            "check failed: {}",
            snap.error.unwrap_or_else(|| "no release".into())
        );
    };
    println!(
        "latest:  {} ({}) {}",
        latest.version, latest.date, latest.url
    );
    let Some(newer) = snap.newer() else {
        println!("up to date");
        return Ok(());
    };
    if let Some(why) = up.blocker() {
        println!("a newer release exists but cannot be installed here: {why}");
        if let Install::Package(_) = up.install()
            && let Some(a) = rugen_update::package_kind().and_then(|k| newer.package(k))
        {
            println!("download the package: {}", a.url);
        }
        return Ok(());
    }
    if check_only {
        println!("run `rugen update` to install {}", newer.version);
        return Ok(());
    }
    let archive = newer
        .own_archive()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    println!("installing {} from {archive}", newer.version);
    let worker = Arc::clone(&up);
    let t = std::thread::spawn(move || worker.apply_now());
    let mut last = String::new();
    while !t.is_finished() {
        let s = up.snapshot();
        let line = match s.phase {
            Phase::Downloading => match s.total {
                Some(total) if total > 0 => format!(
                    "downloading {:>3}% ({} / {} MB)",
                    (s.progress * 100.0) as u32,
                    s.received >> 20,
                    total >> 20
                ),
                _ => format!("downloading {} MB", s.received >> 20),
            },
            Phase::Verifying => "verifying".into(),
            Phase::Installing => "installing".into(),
            _ => String::new(),
        };
        if line != last {
            eprint!("\r\x1b[K{line}");
            last = line;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!();
    let _ = t.join();
    let s = up.snapshot();
    match s.phase {
        Phase::Ready => {
            println!(
                "installed {} in {}; start rugen again to run it",
                newer.version,
                up.install().dir().display()
            );
            Ok(())
        }
        _ => anyhow::bail!("{}", s.error.unwrap_or_else(|| "update failed".into())),
    }
}

/// One line about the install for the terminal.
pub fn describe(install: &Install) -> String {
    match install {
        Install::Portable(d) => format!("portable folder {}", d.display()),
        Install::Package(exe) => format!("system package ({})", exe.display()),
        Install::Dev(d) => format!("development build in {}", d.display()),
    }
}
