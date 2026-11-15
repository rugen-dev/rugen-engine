//! `rugen-update-key`: the release side of the updater's signature
//! (`rugen_update::sign`).
//!
//! ```text
//! rugen-update-key gen <private-key-file>     make a key pair
//! rugen-update-key public                     the public key of the private key
//! rugen-update-key sign <SHA256SUMS> [out]    write SHA256SUMS.sig
//! rugen-update-key verify <SHA256SUMS> [sig]  check a signature
//! ```
//!
//! The private key is never given on the command line and never printed.
//! `sign` and `public` read it from the file named by
//! `RUGEN_UPDATE_SIGNING_KEY_FILE`, or else from the environment variable
//! `RUGEN_UPDATE_SIGNING_KEY` (64 hex digits). `verify` checks against
//! `RUGEN_UPDATE_KEY` (the public key) from the environment, or else the
//! one this tool was built with.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rugen_update::sign;

const USAGE: &str = "\
rugen-update-key gen <private-key-file>     make a key pair (the file must not exist)
rugen-update-key public                     the public key of the private key
rugen-update-key sign <SHA256SUMS> [out]    write the signature (default: <SHA256SUMS>.sig)
rugen-update-key verify <SHA256SUMS> [sig]  check a signature

The private key is read from the file named by RUGEN_UPDATE_SIGNING_KEY_FILE,
or from the environment variable RUGEN_UPDATE_SIGNING_KEY. It is never taken
from the command line and never printed.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("rugen-update-key: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> anyhow::Result<()> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match (arg(0), arg(1)) {
        (Some("gen"), Some(file)) => generate(Path::new(file)),
        (Some("public"), None) => {
            println!(
                "RUGEN_UPDATE_KEY={}",
                sign::hex(&sign::public_of(&private_key()?)?)
            );
            Ok(())
        }
        (Some("sign"), Some(sums)) => {
            let out = arg(2).map_or_else(|| PathBuf::from(format!("{sums}.sig")), PathBuf::from);
            sign_file(Path::new(sums), &out)
        }
        (Some("verify"), Some(sums)) => {
            let sig = arg(2).map_or_else(|| PathBuf::from(format!("{sums}.sig")), PathBuf::from);
            verify_file(Path::new(sums), &sig)
        }
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

/// Write a new private key to `file` (which must not exist; readable by
/// its owner only) and print the public key.
fn generate(file: &Path) -> anyhow::Result<()> {
    use std::io::Write as _;
    let seed = sign::generate()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(file)
        .with_context(|| format!("{}: not written (it must not exist yet)", file.display()))?;
    writeln!(f, "{}", sign::hex(&seed))?;
    f.sync_all()?;
    println!("private key written to {}", file.display());
    println!("  keep it off this machine's repositories and backups you do not control;");
    println!("  whoever has it can sign a release every player's game will install.");
    println!("public key (not a secret; give it to the builds):");
    println!("RUGEN_UPDATE_KEY={}", sign::hex(&sign::public_of(&seed)?));
    Ok(())
}

/// The private key, from `RUGEN_UPDATE_SIGNING_KEY_FILE` or
/// `RUGEN_UPDATE_SIGNING_KEY`.
fn private_key() -> anyhow::Result<[u8; 32]> {
    let text = match std::env::var_os("RUGEN_UPDATE_SIGNING_KEY_FILE") {
        Some(file) if !file.is_empty() => std::fs::read_to_string(&file)
            .with_context(|| format!("read {}", Path::new(&file).display()))?,
        _ => std::env::var("RUGEN_UPDATE_SIGNING_KEY").map_err(|_| {
            anyhow::anyhow!(
                "no private key: set RUGEN_UPDATE_SIGNING_KEY_FILE (a file) or RUGEN_UPDATE_SIGNING_KEY"
            )
        })?,
    };
    // the text is never put in an error message
    sign::parse_key(&text).ok_or_else(|| anyhow::anyhow!("the private key is not 64 hex digits"))
}

fn sign_file(sums: &Path, out: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(sums).with_context(|| format!("read {}", sums.display()))?;
    anyhow::ensure!(
        !rugen_update::parse_sha256sums(&String::from_utf8_lossy(&bytes)).is_empty(),
        "{}: no checksum lines, not signed",
        sums.display()
    );
    let seed = private_key()?;
    let sig = sign::sign(&seed, &bytes)?;
    let public = sign::public_of(&seed)?;
    // never publish a signature the game would refuse
    sign::verify(&public, &bytes, &sig)?;
    std::fs::write(out, format!("{sig}\n")).with_context(|| format!("write {}", out.display()))?;
    println!("{} signed: {}", sums.display(), out.display());
    println!(
        "signed with the key whose public half is {}",
        sign::hex(&public)
    );
    Ok(())
}

fn verify_file(sums: &Path, sig: &Path) -> anyhow::Result<()> {
    let key = match std::env::var("RUGEN_UPDATE_KEY") {
        Ok(k) if !k.trim().is_empty() => sign::parse_key(&k)
            .ok_or_else(|| anyhow::anyhow!("RUGEN_UPDATE_KEY is not 64 hex digits"))?,
        _ => sign::update_key().ok_or_else(|| {
            anyhow::anyhow!("no public key: set RUGEN_UPDATE_KEY (64 hex digits)")
        })?,
    };
    let bytes = std::fs::read(sums).with_context(|| format!("read {}", sums.display()))?;
    let text = std::fs::read_to_string(sig).with_context(|| format!("read {}", sig.display()))?;
    sign::verify(&key, &bytes, &text)?;
    println!("{}: signed by {}", sums.display(), sign::hex(&key));
    Ok(())
}
