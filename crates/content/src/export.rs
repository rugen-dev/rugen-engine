//! `rugen export`: an `.rgc` / `.rgs` back into an ordinary MUGEN folder
//! (Fighter Factory and MUGEN open it).
//!
//! Every original file is written at its path: text, palettes and music
//! byte for byte; sprites as an SFF of the same version with the same
//! sprites, palettes and pixels ([`rugen_formats::sff::Sff::write_exact`]);
//! sounds as an SND whose sounds are the decoded Opus streams as 16-bit WAV;
//! images converted to WebP as PNG again. Files the converter copied in
//! from the engine's `data/` (`common1.cns`) are left out, as they were not
//! part of the original folder. Converting the exported folder again gives
//! the same fingerprint.

use std::path::{Path, PathBuf};

use rugen_formats::rgc;

use crate::{Error, Result};

/// What an export wrote.
#[derive(Clone, Debug, Default)]
pub struct Exported {
    pub dir: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub def: String,
}

/// Write the content file `content` as a folder at `out` (created; it must
/// not hold files the export would overwrite unless `overwrite`).
pub fn export(content: &Path, out: &Path, overwrite: bool) -> Result<Exported> {
    let a = rugen_formats::vfs::archive(content).map_err(|e| Error::Other(e.to_string()))?;
    let meta = a.meta().map_err(|e| Error::Other(e.to_string()))?;
    let mut done = Exported {
        dir: out.to_path_buf(),
        def: meta.def.clone(),
        ..Default::default()
    };
    let skip: Vec<String> = meta.engine_files.iter().map(|p| rgc::canon(p)).collect();
    for e in a.files() {
        if skip.contains(&rgc::canon(&e.path)) {
            continue;
        }
        if !rgc::safe_rel(&e.path) {
            return Err(Error::Other(format!("unsafe path {:?}", e.path)));
        }
        let target = out.join(&e.path);
        if target.exists() && !overwrite {
            return Err(Error::Other(format!(
                "{} exists (pass --force to overwrite)",
                target.display()
            )));
        }
        let bytes = rugen_formats::vfs::read(&content.join(&e.path))
            .map_err(|err| Error::Other(format!("{}: {err}", e.path)))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &bytes)?;
        done.files += 1;
        done.bytes += bytes.len() as u64;
    }
    Ok(done)
}
