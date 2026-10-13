//! Unpacking imported archives into a scratch folder before conversion.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub(crate) fn extract_zip(src: &Path, dst: &Path) -> Result<()> {
    let f = fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(f)?;
    archive.extract(dst)?;
    Ok(())
}

/// RAR has no pure-Rust decoder; use `unrar`, `7z` or `bsdtar` from PATH.
pub(crate) fn extract_rar(src: &Path, dst: &Path) -> Result<()> {
    use std::process::Command;
    let attempts: [(&str, Vec<String>); 3] = [
        (
            "unrar",
            vec![
                "x".into(),
                "-o+".into(),
                "-inul".into(),
                src.display().to_string(),
                format!("{}/", dst.display()),
            ],
        ),
        (
            "7z",
            vec![
                "x".into(),
                "-y".into(),
                format!("-o{}", dst.display()),
                src.display().to_string(),
            ],
        ),
        (
            "bsdtar",
            vec![
                "-xf".into(),
                src.display().to_string(),
                "-C".into(),
                dst.display().to_string(),
            ],
        ),
    ];
    let mut tried = Vec::new();
    for (tool, args) in attempts {
        match Command::new(tool)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(st) if st.success() => return Ok(()),
            Ok(st) => tried.push(format!("{tool} exited with {st}")),
            Err(_) => tried.push(format!("{tool} not found")),
        }
    }
    Err(Error::Rar(format!(
        "{}: no RAR extractor worked ({}). Install unrar/7z or repack as .zip",
        src.display(),
        tried.join(", ")
    )))
}

/// The folder content actually starts in: an archive holding one folder
/// (`Naruto/…`) starts inside it.
pub(crate) fn content_root(dir: &Path) -> PathBuf {
    let mut cur = dir.to_path_buf();
    loop {
        let entries: Vec<PathBuf> = match fs::read_dir(&cur) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| !is_junk(p))
                .collect(),
            Err(_) => return cur,
        };
        if entries.len() == 1 && entries[0].is_dir() {
            cur = entries[0].clone();
        } else {
            return cur;
        }
    }
}

/// Files never worth keeping (OS droppings, old optimiser output).
pub(crate) fn is_junk(p: &Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        name.as_str(),
        "thumbs.db" | ".ds_store" | "desktop.ini" | ".rugen-opt" | "__macosx"
    ) || name.starts_with("._")
}

/// Every file under `dir` (relative paths with `/`, sorted), junk skipped.
pub(crate) fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if is_junk(&p) {
                continue;
            }
            // never follow links out of the folder
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file()
                && let Ok(rel) = p.strip_prefix(dir)
            {
                out.push((rel.to_string_lossy().replace('\\', "/"), p));
            }
        }
    }
    out.sort();
    out
}

/// Main def: `<id>.def`, else the first .def with a `[Files] sprite=` (char)
/// or `[StageInfo]` / `[BGdef]` (stage). Relative to `dir`.
pub(crate) fn find_main_def(dir: &Path, id: &str, stage: bool) -> Option<String> {
    let defs: Vec<(String, PathBuf)> = files_under(dir)
        .into_iter()
        .filter(|(_, p)| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("def")))
        .collect();
    if let Some((rel, _)) = defs.iter().find(|(_, p)| {
        p.file_stem()
            .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(id))
    }) {
        return Some(rel.clone());
    }
    // shallowest first, then by name
    let mut defs = defs;
    defs.sort_by_key(|(rel, _)| (rel.matches('/').count(), rel.to_ascii_lowercase()));
    for (rel, p) in &defs {
        let Ok(d) = rugen_formats::def::Def::open(p) else {
            continue;
        };
        let ok = if stage {
            d.section("stageinfo").is_some() || d.section("bgdef").is_some()
        } else {
            d.section("files").and_then(|f| f.raw("sprite")).is_some()
        };
        if ok {
            return Some(rel.clone());
        }
    }
    None
}
