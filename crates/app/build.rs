//! Build facts baked into the binary: the git commit (`RUGEN_COMMIT`,
//! "unknown" without git) and the target triple (`RUGEN_TARGET`), both
//! recorded in replays. Windows: embed the app icon
//! (`motif/brand/rugen.ico`) into rugen.exe.

fn main() {
    println!("cargo:rerun-if-changed=../../motif/brand/rugen.ico");
    println!("cargo:rerun-if-env-changed=RUGEN_COMMIT");
    // a release build may pass its own (a source tarball has no .git)
    let commit = std::env::var("RUGEN_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUGEN_COMMIT={}", commit.trim());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=RUGEN_TARGET={target}");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../motif/brand/rugen.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}

/// `git rev-parse --short HEAD`, re-run when HEAD moves.
fn git_commit() -> Option<String> {
    let dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let git_dir = String::from_utf8(dir.stdout).ok()?;
    let git_dir = std::path::Path::new(git_dir.trim());
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    // a worktree's refs live in the common dir
    if let Ok(o) = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        && o.status.success()
        && let Ok(common) = String::from_utf8(o.stdout)
    {
        let common = std::path::Path::new(common.trim());
        println!("cargo:rerun-if-changed={}", common.join("packed-refs").display());
        println!("cargo:rerun-if-changed={}", common.join("refs/heads").display());
    }
    let o = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8(o.stdout).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}
