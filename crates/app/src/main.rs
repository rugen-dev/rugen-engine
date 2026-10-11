//! rugen binary.
//!
//! ```text
//! rugen [--root dir]                                            full game: title, select, fight
//! rugen view <char.def | file.sff> [action] [--shot out.png]   sprite / animation viewer
//! rugen play <char> [char2] [--stage s] [--root dir]           local match (keyboard / gamepads)
//!   --net ip:port [--port n] [--p2]                             online rollback match against a peer
//!   --synctest                                                  local match that verifies determinism
//!   --ai n                                                      P2 computer level (default 4, 0 = human)
//!   --watch                                                     both sides computer controlled
//! rugen list [--root dir]                                       list chars/ and stages/
//! rugen check <char> [--root dir]                               report unsupported controllers
//! rugen replay <file.json>                                      play back a recorded match
//!   (quick play records to --record file.json, menu matches to replays/)
//!
//! `<char>` is an id from chars/ (file stem of a .zip/.rar/folder) or a path.
//! ```

#![forbid(unsafe_code)]

use anyhow::Context;

mod config;
mod game;
mod gpu;
mod input;
mod lifebar;
mod play;
mod replay;
mod select;
mod sprites;
mod stage;
mod system;
mod title;
mod versus;
mod viewer;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"),
    )
    .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // a leading option (`rugen --frames 30`) means the full game
    let first = args.first().map(String::as_str);
    match first.filter(|a| !a.starts_with("--")) {
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
            let script = opt("--script");
            let net = opt("--net");
            let port = opt("--port");
            let frames = opt("--frames");
            let ai = opt("--ai");
            let record = opt("--record");
            let taken = [
                stage.clone(),
                shot.clone(),
                root.clone(),
                script.clone(),
                net.clone(),
                port.clone(),
                frames.clone(),
                ai.clone(),
                record.clone(),
            ];
            let p2_side = args.iter().any(|a| a == "--p2");
            let watch = args.iter().any(|a| a == "--watch");
            let synctest = args.iter().any(|a| a == "--synctest");
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| !taken.iter().any(|t| t.as_ref() == Some(*a)))
                .collect();
            let p1 = rest.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "usage: rugen play <char> [char2] [--stage s] [--root dir] [--ai 0..8] [--net ip:port [--port n] [--p2]] [--synctest] [--shot out.png] [--script keys]"
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
            let stage = stage.or_else(|| {
                lib.stages().first().map(|e| e.id.clone()).or_else(|| {
                    let p = lib.root.join("reference/mugen-1.1b1/stages/stage0.def");
                    p.exists().then(|| p.display().to_string())
                })
            });
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
            let config = config::Config::load(&lib.root);
            let net_mode = if let Some(remote) = net {
                let remote: std::net::SocketAddr = remote
                    .parse()
                    .with_context(|| format!("--net expects ip:port, got {remote:?}"))?;
                let port = match port {
                    Some(p) => p.parse().context("--port expects a number")?,
                    None => config.net.port,
                };
                Some(play::NetMode::P2P {
                    port,
                    remote,
                    side: usize::from(p2_side),
                })
            } else if synctest {
                Some(play::NetMode::SyncTest)
            } else {
                None
            };
            let ai = ai
                .map(|a| a.parse::<i32>().context("--ai expects 0..8"))
                .transpose()?
                .unwrap_or(if synctest || net_mode.is_some() { 0 } else { 4 });
            let setup = play::FightSetup {
                p1: p1.clone(),
                p2: p2.unwrap_or(p1),
                stage,
                ai: [if watch { ai } else { 0 }, ai],
                net: net_mode,
            };
            let harness = game::Harness {
                shot: shot.map(std::path::PathBuf::from),
                frames: frames
                    .map(|f| f.parse::<u32>().context("--frames expects a number"))
                    .transpose()?,
                script: script.as_deref().map(play::parse_script),
                record: record.map(std::path::PathBuf::from),
            };
            game::Shell::new(
                config,
                lib,
                game::Transition::Fight(setup),
                true,
                harness,
                None,
            )
            .run()
        }
        Some("replay") => {
            let path = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("usage: rugen replay <file.json> [--shot out.png] [--frames n]")
            })?;
            let r = replay::Replay::load(std::path::Path::new(path))
                .with_context(|| path.to_string())?;
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let lib = rugen_content::Library::discover(
                opt("--root").as_deref().map(std::path::Path::new),
            );
            let config = config::Config::load(&lib.root);
            let harness = game::Harness {
                shot: opt("--shot").map(std::path::PathBuf::from),
                frames: opt("--frames")
                    .map(|f| f.parse::<u32>().context("--frames expects a number"))
                    .transpose()?,
                script: None,
                record: None,
            };
            let setup = r.setup();
            game::Shell::new(
                config,
                lib,
                game::Transition::Fight(setup),
                true,
                harness,
                Some(r),
            )
            .run()
        }
        Some("check") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let root = opt("--root");
            let name = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .find(|a| Some(*a) != root.as_ref())
                .ok_or_else(|| anyhow::anyhow!("usage: rugen check <char> [--root dir]"))?;
            let lib = rugen_content::Library::discover(root.as_deref().map(std::path::Path::new));
            let entry = lib
                .find_char(name)
                .ok_or_else(|| anyhow::anyhow!("character {name:?} not found"))?;
            let (dir, m) = lib.prepare_char(&entry)?;
            let t = std::time::Instant::now();
            let data = rugen_sim::CharData::load_with_data(&dir.join(&m.def), &lib.data_dir())?;
            println!(
                "{} by {} — {} states, {} sprites, loaded in {:.2?}",
                data.display_name,
                data.author,
                data.states.len(),
                data.sff.sprites.len(),
                t.elapsed()
            );
            let mut unsupported: std::collections::BTreeMap<String, usize> = Default::default();
            let mut total = 0usize;
            for st in data.states.values() {
                for c in &st.ctrls {
                    total += 1;
                    if let rugen_sim::sctrl::Sctrl::Unsupported(k) = &c.op {
                        *unsupported.entry(k.clone()).or_default() += 1;
                    }
                }
            }
            println!(
                "{total} controllers, {} unsupported kinds:",
                unsupported.len()
            );
            for (k, n) in &unsupported {
                println!("  {k:<24} x{n}");
            }
            println!("{} warnings:", data.warnings.len());
            for w in &data.warnings {
                println!("  {w}");
            }
            Ok(())
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
        None | Some("game") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let root = opt("--root");
            let lib = rugen_content::Library::discover(root.as_deref().map(std::path::Path::new));
            let config = config::Config::load(&lib.root);
            let harness = game::Harness {
                shot: opt("--shot").map(std::path::PathBuf::from),
                frames: opt("--frames")
                    .map(|f| f.parse::<u32>().context("--frames expects a number"))
                    .transpose()?,
                script: opt("--script").as_deref().map(play::parse_script),
                record: None,
            };
            game::Shell::new(config, lib, game::Transition::Title, false, harness, None).run()
        }
        _ => {
            eprintln!(
                "rugen {}\n\nusage:\n  rugen view <char.def|file.sff> [action] [--shot out.png]\n  rugen play <char> [char2] [--stage s] [--root dir] [--shot out.png]\n  rugen list [--root dir]\n  rugen check <char> [--root dir]",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
