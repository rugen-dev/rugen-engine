//! rugen binary.
//!
//! ```text
//! rugen view <char.def | file.sff> [action] [--shot out.png]   sprite / animation viewer
//! rugen play <p1.def> [p2.def] [--stage stage.def]              local match, keyboard P1
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
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| Some(*a) != stage.as_ref() && Some(*a) != shot.as_ref())
                .collect();
            let p1 = rest.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "usage: rugen play <p1.def> [p2.def] [--stage stage.def] [--shot out.png]"
                )
            })?;
            play::run(
                std::path::Path::new(p1),
                rest.get(1).map(|s| std::path::Path::new(s.as_str())),
                stage.as_deref().map(std::path::Path::new),
                shot.map(std::path::PathBuf::from),
            )
        }
        _ => {
            eprintln!(
                "rugen {}\n\nusage:\n  rugen view <char.def|file.sff> [action] [--shot out.png]\n  rugen play <p1.def> [p2.def] [--stage stage.def] [--shot out.png]",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
