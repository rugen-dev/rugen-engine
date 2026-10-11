//! rugen binary.
//!
//! ```text
//! rugen view <char.def | file.sff> [action] [--shot out.png]   sprite / animation viewer
//! ```

#![forbid(unsafe_code)]

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
        _ => {
            eprintln!(
                "rugen {}\n\nusage:\n  rugen view <char.def|file.sff> [action] [--shot out.png]",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
