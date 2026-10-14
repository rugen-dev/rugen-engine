//! User themes: `themes/<name>/` next to `chars/`. Any folder there with a
//! `system.def` shows up in OPTIONS → INTERFACE next to the bundled themes.
//!
//! A user theme may hold only the files it changes: an optional
//! `theme.toml` (`name = "Neon"`, `base = "ink"`) names a bundled theme
//! whose files fill in the rest, then the flat `motif/` and `data/` do, as
//! for the bundled themes. Without a `base` the folder is a plain MUGEN
//! screenpack: rugen draws its own menus only where the folder ships
//! `ui/*.toml` screens, MUGEN's elsewhere.
//!
//! While a user theme is active the game watches its files ([`Watch`]) and
//! reloads the screen on save: the screen's TOML, its images, `system.def`
//! and its fonts, keeping the focus and the screen's state; the fight HUD
//! (`fight.def`) follows at the next fight. A TOML that does not parse
//! keeps the last good screen and shows the error in a strip on top.
//!
//! Online matches never simulate with a user theme's `fight.def` (round
//! timings are part of the simulation): they use the bundled one
//! ([`sim_motif`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rugen_content::Library;

/// The bundled `motif/` and the player's `themes/`.
struct Roots {
    bundled: PathBuf,
    user: PathBuf,
}

static ROOTS: OnceLock<Roots> = OnceLock::new();

/// Remember where the bundled and the user themes live (once, at launch).
pub fn init(lib: &Library) {
    let _ = std::fs::create_dir_all(lib.themes_dir());
    let _ = ROOTS.set(Roots {
        bundled: lib.assets.join("motif"),
        user: lib.themes_dir(),
    });
}

/// A theme folder inside the player's `themes/`.
pub fn is_user(motif: &Path) -> bool {
    user_roots(ROOTS.get(), motif).is_some()
}

/// The roots when `motif` is a user theme.
fn user_roots<'a>(roots: Option<&'a Roots>, motif: &Path) -> Option<&'a Roots> {
    roots.filter(|r| motif.parent() == Some(r.user.as_path()))
}

/// `theme.toml` of a user theme.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Meta {
    /// Shown on the INTERFACE card (default: the folder name).
    pub name: Option<String>,
    /// Bundled theme (`ink`, `dojo`) whose files fill in the missing ones.
    pub base: Option<String>,
}

impl Meta {
    /// `dir/theme.toml`; a missing or broken file reads as empty.
    pub fn read(dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(dir.join("theme.toml")) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|e| {
            log::warn!("{}: {e}", dir.join("theme.toml").display());
            Self::default()
        })
    }
}

/// A plain folder name (no separators, not hidden).
fn plain(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':'])
}

/// The bundled theme a user theme builds on (`theme.toml` `base`).
fn base_dir(roots: &Roots, motif: &Path) -> Option<PathBuf> {
    let base = Meta::read(motif).base?;
    let base = base.trim();
    let d = roots.bundled.join("themes").join(base);
    (plain(base) && d.is_dir()).then_some(d)
}

/// Lookup order for a theme's own files (UI TOML, images, `news.json`):
/// the theme, then for a user theme its base and the bundled `motif/`, for
/// a bundled theme the flat `motif/` it lives under.
pub fn chain(motif: &Path) -> Vec<PathBuf> {
    chain_in(ROOTS.get(), motif)
}

fn chain_in(roots: Option<&Roots>, motif: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![motif.to_path_buf()];
    match user_roots(roots, motif) {
        Some(r) => {
            dirs.extend(base_dir(r, motif));
            dirs.push(r.bundled.clone());
        }
        _ => {
            if let Some(parent) = motif.parent()
                && parent.file_name().is_some_and(|n| n == "themes")
                && let Some(flat) = parent.parent()
            {
                dirs.push(flat.to_path_buf());
            }
        }
    }
    dirs
}

/// Lookup order for screenpack files (`system.def`, `fight.def`, their
/// sprites and fonts): [`chain`], then `data_dir`. Bundled themes keep the
/// simulation's [`rugen_sim::chardata::motif_chain`].
pub fn screenpack_chain(motif: &Path, data_dir: &Path) -> Vec<PathBuf> {
    screenpack_chain_in(ROOTS.get(), motif, data_dir)
}

fn screenpack_chain_in(roots: Option<&Roots>, motif: &Path, data_dir: &Path) -> Vec<PathBuf> {
    if user_roots(roots, motif).is_none() {
        return rugen_sim::chardata::motif_chain(motif, data_dir);
    }
    let mut dirs = chain_in(roots, motif);
    if !dirs.iter().any(|d| d == data_dir) {
        dirs.push(data_dir.to_path_buf());
    }
    dirs
}

/// The motif a match simulates with (`fight.def` round timings): a user
/// theme's own `fight.def` offline, else its base or the bundled default
/// theme. Online always the bundled one: both peers and the server's
/// re-simulation must agree.
pub fn sim_motif(motif: &Path, online: bool) -> PathBuf {
    sim_motif_in(ROOTS.get(), motif, online)
}

fn sim_motif_in(roots: Option<&Roots>, motif: &Path, online: bool) -> PathBuf {
    let Some(r) = user_roots(roots, motif) else {
        return motif.to_path_buf();
    };
    if !online && motif.join("fight.def").is_file() {
        return motif.to_path_buf();
    }
    let ink = r.bundled.join("themes").join("ink");
    match base_dir(r, motif) {
        Some(b) if !online => b,
        _ if ink.is_dir() => ink,
        _ => r.bundled.clone(),
    }
}

/// One card of the INTERFACE theme picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// `[video] theme` value.
    pub id: String,
    /// Shown on the card.
    pub label: String,
    /// The player's own (live reload) rather than bundled.
    pub user: bool,
    /// `ui/preview.webp` (or `.png`) when the theme has one.
    pub preview: Option<PathBuf>,
}

/// The theme picker's cards: every installed theme (`classic` aside).
pub fn cards(lib: &Library) -> Vec<Card> {
    lib.themes()
        .into_iter()
        .filter(|t| t != "classic")
        .map(|id| {
            let user = lib.user_theme_dir(&id);
            let dir = user.clone().unwrap_or_else(|| lib.motif_dir_for(&id));
            let label = match &user {
                Some(d) => Meta::read(d)
                    .name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| id.clone())
                    .to_uppercase(),
                None => crate::settings::theme_label(&id),
            };
            let preview = ["ui/preview.webp", "ui/preview.png"]
                .iter()
                .map(|f| dir.join(f))
                .find(|p| p.is_file());
            Card {
                id,
                label,
                user: user.is_some(),
                preview,
            }
        })
        .collect()
}

/// Open the player's `themes/` in the system file manager (made first).
pub fn open_folder(dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("{}: {e}", dir.display());
        return;
    }
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(dir).spawn() {
        Ok(_) => log::info!("opened {}", dir.display()),
        Err(e) => log::warn!("{opener} {}: {e}", dir.display()),
    }
}

/// Start a user theme: `themes/<name>/` with a `theme.toml` building on the
/// bundled `from` theme, copies of its `system.def` / `fight.def` and of
/// every `ui/*.toml` screen to edit (images, fonts and sprites come from
/// the base until the folder holds its own).
pub fn create(lib: &Library, name: &str, from: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(plain(name), "{name:?}: use a plain folder name");
    let base = lib.assets.join("motif").join("themes").join(from);
    anyhow::ensure!(
        base.join("system.def").is_file(),
        "no bundled theme {from:?} (ink, dojo)"
    );
    anyhow::ensure!(
        !lib.themes().iter().any(|t| t.eq_ignore_ascii_case(name)),
        "a theme named {name:?} exists already"
    );
    let dir = lib.themes_dir().join(name);
    anyhow::ensure!(!dir.exists(), "{} exists already", dir.display());
    std::fs::create_dir_all(dir.join("ui"))?;
    std::fs::write(
        dir.join("theme.toml"),
        format!(
            "# rugen user theme: files missing here come from the bundled \"{from}\" theme.\n\
             name = \"{}\"\nbase = \"{from}\"\n",
            name.to_uppercase()
        ),
    )?;
    for f in ["system.def", "fight.def"] {
        if base.join(f).is_file() {
            std::fs::copy(base.join(f), dir.join(f))?;
        }
    }
    for e in std::fs::read_dir(base.join("ui"))?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "toml") {
            std::fs::copy(&p, dir.join("ui").join(e.file_name()))?;
        }
    }
    Ok(dir)
}

/// Watches the active user theme's files.
pub struct Watch {
    dir: PathBuf,
    sig: u64,
    countdown: u32,
}

/// Ticks between two looks at the files (0.5 s).
const EVERY: u32 = 30;

impl Watch {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            sig: signature(dir),
            countdown: EVERY,
        }
    }

    /// Call once a tick: true when a file was added, removed or saved since
    /// the last look.
    pub fn poll(&mut self) -> bool {
        self.countdown = self.countdown.saturating_sub(1);
        if self.countdown > 0 {
            return false;
        }
        self.countdown = EVERY;
        let now = signature(&self.dir);
        std::mem::replace(&mut self.sig, now) != now
    }
}

/// Hash of every file's path, size and modification time under `dir`
/// (four levels deep, at most 4000 entries).
pub fn signature(dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut seen = 0usize;
    fn walk(d: &Path, depth: u32, h: &mut impl Hasher, seen: &mut usize) {
        let Ok(rd) = std::fs::read_dir(d) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            *seen += 1;
            if *seen > 4000 {
                return;
            }
            let Ok(md) = e.metadata() else { continue };
            e.file_name().hash(h);
            if md.is_dir() {
                if depth < 4 {
                    walk(&e.path(), depth + 1, h, seen);
                }
                continue;
            }
            md.len().hash(h);
            if let Ok(t) = md.modified()
                && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
            {
                d.as_nanos().hash(h);
            }
        }
    }
    walk(dir, 0, &mut h, &mut seen);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rugen-themes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A saved file changes the signature, an untouched folder keeps it.
    #[test]
    fn the_watch_sees_saves() {
        let d = tmp("watch");
        std::fs::create_dir_all(d.join("ui")).unwrap();
        std::fs::write(d.join("ui/title.toml"), "a").unwrap();
        let mut w = Watch::new(&d);
        for _ in 0..EVERY * 2 {
            assert!(!w.poll());
        }
        std::fs::write(d.join("ui/title.toml"), "ab").unwrap();
        let fired = (0..EVERY).any(|_| w.poll());
        assert!(fired, "a save is seen within half a second");
        std::fs::write(d.join("ui/new.webp"), "x").unwrap();
        assert!((0..EVERY).any(|_| w.poll()), "a new file too");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A user theme with `base = "ink"` falls back to ink, then the bundled
    /// `motif/`, then `data/`; bundled themes keep their chain; online
    /// matches never simulate with a user theme's fight.def.
    #[test]
    fn user_themes_fall_back_to_their_base() {
        let d = tmp("chain");
        let roots = Roots {
            bundled: d.join("motif"),
            user: d.join("themes"),
        };
        let ink = d.join("motif/themes/ink");
        let neon = d.join("themes/neon");
        std::fs::create_dir_all(&ink).unwrap();
        std::fs::create_dir_all(&neon).unwrap();
        let data = d.join("data");
        let r = Some(&roots);
        // no theme.toml: a plain screenpack
        assert_eq!(chain_in(r, &neon), [neon.clone(), roots.bundled.clone()]);
        std::fs::write(neon.join("theme.toml"), "base = \"ink\"").unwrap();
        assert_eq!(
            chain_in(r, &neon),
            [neon.clone(), ink.clone(), roots.bundled.clone()]
        );
        assert_eq!(
            screenpack_chain_in(r, &neon, &data),
            [
                neon.clone(),
                ink.clone(),
                roots.bundled.clone(),
                data.clone()
            ]
        );
        // a bundled theme: the theme, then the flat motif/
        assert_eq!(chain_in(r, &ink), [ink.clone(), roots.bundled.clone()]);
        assert_eq!(
            screenpack_chain_in(r, &ink, &data),
            rugen_sim::chardata::motif_chain(&ink, &data)
        );
        // a base outside motif/themes is ignored
        std::fs::write(neon.join("theme.toml"), "base = \"../../x\"").unwrap();
        assert_eq!(chain_in(r, &neon), [neon.clone(), roots.bundled.clone()]);
        // the simulation's fight.def
        std::fs::write(neon.join("theme.toml"), "base = \"ink\"").unwrap();
        assert_eq!(sim_motif_in(r, &neon, false), ink);
        std::fs::write(neon.join("fight.def"), "[Round]").unwrap();
        assert_eq!(sim_motif_in(r, &neon, false), neon);
        assert_eq!(sim_motif_in(r, &neon, true), ink, "online: bundled");
        assert_eq!(sim_motif_in(r, &ink, true), ink);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn theme_toml_reads_leniently() {
        let d = tmp("meta");
        assert_eq!(Meta::read(&d), Meta::default());
        std::fs::write(d.join("theme.toml"), "name = \"Neon\"\nbase = \"ink\"\n").unwrap();
        assert_eq!(
            Meta::read(&d),
            Meta {
                name: Some("Neon".into()),
                base: Some("ink".into())
            }
        );
        std::fs::write(d.join("theme.toml"), "name = [").unwrap();
        assert_eq!(Meta::read(&d), Meta::default());
        let _ = std::fs::remove_dir_all(&d);
    }
}
