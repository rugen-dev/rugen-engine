//! Unpack a release archive into a staging folder next to the install and
//! swap it in: the binary, `motif/` and `data/` are replaced, player files
//! are never touched. Renames only (the staging folder sits on the same
//! file system), so a failure half-way leaves the old tree in `.rugen-old/`
//! and the next start cleans up ([`crate::cleanup_old`]).

use std::path::{Component, Path, PathBuf};

use anyhow::Context;

use crate::ArchiveKind;

/// Staging folder inside the install (download + unpacked tree).
pub const STAGE_DIR: &str = ".rugen-update";
/// The replaced files wait here until the swap went through.
pub const OLD_DIR: &str = ".rugen-old";

/// Names at the install root the archive never replaces.
const KEEP: [&str; 7] = [
    "rugen.toml",
    "themes",
    "cache",
    "replays",
    "saves",
    STAGE_DIR,
    OLD_DIR,
];

/// A relative path with only normal components (no `..`, no root).
fn safe_relative(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Unpack `archive` into `dest`. Tarballs hold one top folder
/// (`rugen-X-linux-x86_64/`), zips the files themselves: either way `dest`
/// ends up holding `rugen[.exe]`, `motif/`, `data/`...
pub fn extract(archive: &Path, kind: ArchiveKind, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    match kind {
        ArchiveKind::Zip => extract_zip(archive, dest),
        ArchiveKind::TarGz => extract_tar_gz(archive, dest),
    }
    .with_context(|| format!("unpack {}", archive.display()))
}

fn extract_zip(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    // a zip may also carry one top folder: strip it when every entry shares it
    let count = zip.len();
    let names: Vec<PathBuf> = (0..count)
        .filter_map(|i| zip.by_index(i).ok()?.enclosed_name())
        .collect();
    let strip = common_top(names.into_iter(), count);
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let rel = match strip {
            true => name.components().skip(1).collect::<PathBuf>(),
            false => name,
        };
        let Some(rel) = safe_relative(&rel) else {
            continue;
        };
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        set_mode(&out, entry.unix_mode());
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let open = || -> anyhow::Result<tar::Archive<flate2::read::GzDecoder<std::fs::File>>> {
        let file = std::fs::File::open(archive)?;
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(file)))
    };
    // first pass: is there one top folder to strip?
    let mut paths = Vec::new();
    for e in open()?.entries()? {
        paths.push(e?.path()?.into_owned());
    }
    let strip = common_top(paths.iter().cloned(), paths.len());
    let mut tar = open()?;
    for e in tar.entries()? {
        let mut entry = e?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            continue;
        }
        let path = entry.path()?.into_owned();
        let rel = if strip {
            path.components().skip(1).collect::<PathBuf>()
        } else {
            path
        };
        let Some(rel) = safe_relative(&rel) else {
            continue;
        };
        let out = dest.join(&rel);
        if kind.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

/// Every one of `count` entry paths starts with the same single folder.
fn common_top(paths: impl Iterator<Item = PathBuf>, count: usize) -> bool {
    if count == 0 {
        return false;
    }
    let mut top: Option<std::ffi::OsString> = None;
    for p in paths {
        let mut c = p.components();
        let Some(Component::Normal(first)) = c.next() else {
            return false;
        };
        match &top {
            None => top = Some(first.to_os_string()),
            Some(t) if t.as_os_str() == first => {}
            Some(_) => return false,
        }
    }
    top.is_some()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m & 0o7777));
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) {}

/// The unpacked tree looks like a rugen release: the binary and `motif/`.
pub fn staged_ok(staged: &Path) -> anyhow::Result<()> {
    let exe = staged.join(crate::exe_name());
    anyhow::ensure!(exe.is_file(), "archive has no {}", crate::exe_name());
    anyhow::ensure!(staged.join("motif").is_dir(), "archive has no motif/");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&exe)?.permissions();
        if perm.mode() & 0o111 == 0 {
            perm.set_mode(0o755);
            std::fs::set_permissions(&exe, perm)?;
        }
    }
    Ok(())
}

/// Rename `from` over `to`; Windows refuses to replace an existing file,
/// so it goes aside first.
fn replace_path(from: &Path, to: &Path, aside: &Path) -> anyhow::Result<()> {
    if to.exists() {
        if let Some(p) = aside.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::rename(to, aside).with_context(|| format!("move {} aside", to.display()))?;
    }
    std::fs::rename(from, to).with_context(|| format!("put {} in place", to.display()))
}

/// Swap the unpacked release at `staged` into `install`: the binary,
/// `motif/`, `data/` and the top-level files (README, licences, the
/// `.desktop` file, `chars/README.md`, `stages/README.md`). Everything in
/// [`KEEP`] and every other file the player added stays.
///
/// The running binary: Unix lets a new file be renamed over it; Windows
/// keeps it as `rugen.exe.old` until the next start deletes it.
pub fn apply(install: &Path, staged: &Path) -> anyhow::Result<()> {
    staged_ok(staged)?;
    let old = install.join(OLD_DIR);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir_all(&old)?;
    let exe = crate::exe_name();
    for entry in std::fs::read_dir(staged)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_s = name.to_string_lossy().to_string();
        if KEEP.contains(&name_s.as_str()) {
            continue;
        }
        let from = entry.path();
        let to = install.join(&name);
        let is_dir = entry.file_type()?.is_dir();
        if name_s == "chars" || name_s == "stages" {
            // only the README the release ships; the player's files stay
            std::fs::create_dir_all(&to)?;
            for f in std::fs::read_dir(&from)? {
                let f = f?;
                if f.file_type()?.is_file() {
                    let dst = to.join(f.file_name());
                    replace_path(&f.path(), &dst, &old.join(&name).join(f.file_name()))?;
                }
            }
            continue;
        }
        if name_s == exe {
            let aside = if cfg!(windows) {
                install.join(format!("{exe}.old"))
            } else {
                old.join(exe)
            };
            let _ = std::fs::remove_file(&aside);
            replace_path(&from, &to, &aside)?;
            continue;
        }
        let _ = is_dir;
        replace_path(&from, &to, &old.join(&name))?;
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Start `exe` with `args` (the relaunch after an update); the caller quits.
pub fn relaunch(exe: &Path, args: &[String]) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    cmd.spawn()
        .with_context(|| format!("start {}", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_checked() {
        assert_eq!(
            safe_relative(Path::new("motif/./a.webp")),
            Some(PathBuf::from("motif/a.webp"))
        );
        assert_eq!(safe_relative(Path::new("../x")), None);
        assert_eq!(safe_relative(Path::new("/etc/passwd")), None);
        assert_eq!(safe_relative(Path::new(".")), None);
    }

    #[test]
    fn one_top_folder_is_detected() {
        let one = ["r/", "r/rugen", "r/motif/a"].map(PathBuf::from);
        assert!(common_top(one.iter().cloned(), one.len()));
        let flat = ["rugen", "motif/a"].map(PathBuf::from);
        assert!(!common_top(flat.iter().cloned(), flat.len()));
        assert!(!common_top(std::iter::empty(), 0));
    }
}
