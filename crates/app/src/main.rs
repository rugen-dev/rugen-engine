//! rugen binary.
//!
//! ```text
//! rugen view <char.def | file.sff> [action] [--shot out.png]   sprite / animation viewer
//! rugen play <char> [char2] [--stage s] [--root dir]           local match, keyboard P1
//! rugen list [--root dir]                                       list chars/ and stages/
//!
//! `<char>` is an id from chars/ (file stem of a .zip/.rar/folder) or a path.
//! ```

#![forbid(unsafe_code)]

mod play;
mod viewer;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"),
    )
    .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("view") => {
            let shot = args
                .iter()
                .position(|a| a == "--shot")
                .and_then(|i| args.get(i + 1).cloned());
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| Some(*a) != shot.as_ref())
                .collect();
            let path = rest.first().ok_or_else(|| {
                anyhow::anyhow!("usage: rugen view <char.def|file.sff> [action] [--shot out.png]")
            })?;
            let action = rest.get(1).map(|s| s.parse::<i32>()).transpose()?;
            viewer::run(
                std::path::Path::new(path),
                action,
                shot.map(std::path::PathBuf::from),
            )
        }
        Some("play") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let stage = opt("--stage");
            let shot = opt("--shot");
            let root = opt("--root");
            let taken = [stage.clone(), shot.clone(), root.clone()];
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| !taken.iter().any(|t| t.as_ref() == Some(*a)))
                .collect();
            let p1 = rest.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "usage: rugen play <char> [char2] [--stage s] [--root dir] [--shot out.png]"
                )
            })?;
            let lib = rugen_content::Library::discover(root.as_deref().map(std::path::Path::new));
            let resolve = |name: &str| -> anyhow::Result<std::path::PathBuf> {
                let entry = lib.find_char(name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "character {name:?} not found in {}",
                        lib.chars_dir().display()
                    )
                })?;
                let (dir, m) = lib.prepare_char(&entry)?;
                Ok(dir.join(m.def))
            };
            let p1 = resolve(p1)?;
            let p2 = rest.get(1).map(|s| resolve(s)).transpose()?;
            let stage = stage
                .map(|s| -> anyhow::Result<std::path::PathBuf> {
                    let p = std::path::Path::new(&s);
                    if p.exists() && p.extension().is_some_and(|e| e == "def") {
                        return Ok(p.to_path_buf());
                    }
                    let entry = lib
                        .stages()
                        .into_iter()
                        .find(|e| e.id.eq_ignore_ascii_case(&s))
                        .ok_or_else(|| anyhow::anyhow!("stage {s:?} not found"))?;
                    let (dir, m) = lib.prepare_stage(&entry)?;
                    Ok(dir.join(m.def))
                })
                .transpose()?;
            play::run(
                &p1,
                p2.as_deref(),
                stage.as_deref(),
                shot.map(std::path::PathBuf::from),
                &lib.data_dir(),
            )
        }
        Some("list") => {
            let root = args
                .iter()
                .position(|a| a == "--root")
                .and_then(|i| args.get(i + 1).cloned());
            let lib = rugen_content::Library::discover(root.as_deref().map(std::path::Path::new));
            println!("root: {}", lib.root.display());
            println!("chars:");
            for c in lib.chars() {
                println!("  {:<24} {:?}  {}", c.id, c.kind, c.source.display());
            }
            println!("stages:");
            for s in lib.stages() {
                println!("  {:<24} {:?}  {}", s.id, s.kind, s.source.display());
            }
            Ok(())
        }
        _ => {
            eprintln!(
                "rugen {}\n\nusage:\n  rugen view <char.def|file.sff> [action] [--shot out.png]\n  rugen play <char> [char2] [--stage s] [--root dir] [--shot out.png]\n  rugen list [--root dir]",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
