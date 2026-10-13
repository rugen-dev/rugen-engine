//! rugen binary.
//!
//! ```text
//! rugen [--root dir]                                            full game: title, select, fight
//! rugen view <char.def | file.sff> [action] [--shot out.png]   sprite / animation viewer
//! rugen stage-gen [--out motif/stages]                    built-in stages (training, ink_dojo, ink_arena)
//! rugen play <char> [char2] [--stage s] [--root dir]           local match (keyboard / gamepads)
//!   --net ip:port [--port n] [--p2]                             LAN / direct-IP rollback match against a peer
//!   --server url --room CODE|new [--spectate] [--guest]         online through rugen-server (direct peer
//!                                                               to peer, else TURN / relayed; [net] direct
//!                                                               = false never shares the IP; `new` creates a room):
//!                                                               brings <char>, shows the matchup, confirms
//!                                                               unless BLOCKED, syncs files, fights, quits;
//!                                                               with --shot: picture of the matchup screen
//!   --synctest                                                  local match that verifies determinism
//!   --ai n                                                      P2 computer level (default 4, 0 = human)
//!   --watch                                                     both sides computer controlled
//!   --shot out.png --frames n --script keys --script2 keys     harness: P1 / P2 scripted inputs
//!   --keys 90:Enter,120:k                                       harness: raw key presses at ticks
//! rugen list [--root dir]                                       list chars/ and stages/
//! rugen convert <folder|archive> [out.rgc] [--stage]            write a shareable .rgc / .rgs
//! rugen fontgen --roles [--motif motif] [--out dir]             regenerate the motif's role fonts
//! rugen export <file.rgc|.rgs> [out_dir] [--force]              back to a MUGEN folder (Fighter Factory)
//! rugen check <char> [--root dir]                               report unsupported controllers
//! rugen verify <char> | --all [--root dir]                      load + headless soak, verdict stored in the cache
//!   (play refuses `broken` characters unless --force)
//! rugen replay <file.json>                                      play back a recorded match
//!   (quick play records to --record file.json, menu matches to replays/)
//!
//! `<char>` is an id from chars/ (file stem of a .rgc / .zip / .rar / folder) or a path.
//! ```

#![forbid(unsafe_code)]
// the ONLINE screen's data context is one large json! literal
#![recursion_limit = "256"]

use anyhow::Context;

mod config;
mod fontgen;
mod game;
mod gpu;
mod inkgen;
mod inkscreens;
mod input;
mod kit;
mod latency;
mod lifebar;
mod matchup;
mod official;
mod online;
mod options;
mod pixelize;
mod play;
mod replay;
mod roster;
mod screeninfo;
mod screenpack;
mod select;
mod settings;
mod settings_gen;
mod smooth;
mod sprites;
mod stage;
mod stagegen;
mod system;
mod title;
mod tutorial;
mod uigen;
mod uiscreen;
mod verify;
mod versus;
mod viewer;

/// The content library with the `[content]` settings of its `rugen.toml`.
fn library(root: Option<&str>) -> rugen_content::Library {
    let mut lib = rugen_content::Library::discover(root.map(std::path::Path::new));
    lib.keep_originals = config::Config::load(&lib.root).content.keep_originals;
    lib
}

/// A recorded def path, or the same character / stage found again by id when
/// the path is gone (`cache/chars/<id>-<hash>/x.def` of older versions).
fn relocate(
    lib: &rugen_content::Library,
    def: &std::path::Path,
    stage: bool,
) -> std::path::PathBuf {
    if rugen_formats::vfs::exists(def) {
        return def.to_path_buf();
    }
    let id = def
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .map(|n| n.rsplit_once('-').map_or(n.clone(), |(a, _)| a.to_string()))
        .unwrap_or_default();
    let found = if stage {
        lib.stages()
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(&id))
            .and_then(|e| lib.prepare_stage(&e).ok())
    } else {
        lib.find_char(&id).and_then(|e| lib.prepare_char(&e).ok())
    };
    found.map_or_else(|| def.to_path_buf(), |(d, m)| m.def_path(&d))
}

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
            let script2 = opt("--script2");
            let net = opt("--net");
            let port = opt("--port");
            let server = opt("--server");
            let room = opt("--room");
            let frames = opt("--frames");
            let ai = opt("--ai");
            let record = opt("--record");
            let keys = opt("--keys");
            let taken = [
                keys.clone(),
                stage.clone(),
                shot.clone(),
                root.clone(),
                script.clone(),
                script2.clone(),
                net.clone(),
                port.clone(),
                server.clone(),
                room.clone(),
                frames.clone(),
                ai.clone(),
                record.clone(),
            ];
            let p2_side = args.iter().any(|a| a == "--p2");
            let watch = args.iter().any(|a| a == "--watch");
            let force = args.iter().any(|a| a == "--force");
            let synctest = args.iter().any(|a| a == "--synctest");
            let spectate = args.iter().any(|a| a == "--spectate");
            let guest = args.iter().any(|a| a == "--guest");
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| !taken.iter().any(|t| t.as_ref() == Some(*a)))
                .collect();
            let p1 = rest.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "usage: rugen play <char> [char2] [--stage s] [--root dir] [--ai 0..8] [--net ip:port [--port n] [--p2]] [--server url --room CODE|new [--spectate] [--guest]] [--synctest] [--shot out.png] [--script keys] [--script2 keys]"
                )
            })?;
            let lib = library(root.as_deref());
            let resolve = |name: &str| -> anyhow::Result<std::path::PathBuf> {
                let entry = lib.find_char(name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "character {name:?} not found in {}",
                        lib.chars_dir().display()
                    )
                })?;
                let (dir, m) = verify::prepare_verified(&lib, &entry)?;
                if m.verify.as_ref().is_some_and(|v| v.is_broken()) && !force {
                    anyhow::bail!(
                        "{name} failed verification ({}); run `rugen verify {name}` for details or pass --force",
                        m.verify
                            .as_ref()
                            .map(|v| v.issues.join("; "))
                            .unwrap_or_default()
                    );
                }
                Ok(m.def_path(&dir))
            };
            let config = config::Config::load(&lib.root);
            // online: the ONLINE screen drives the room and the matchup
            if let Some(room) = &room {
                let server = server
                    .clone()
                    .or_else(|| Some(config.net.server.clone()).filter(|s| !s.trim().is_empty()))
                    .context("--room needs --server URL (or [net] server in rugen.toml)")?;
                let entry = lib.find_char(p1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "character {p1:?} not found in {}",
                        lib.chars_dir().display()
                    )
                })?;
                let auto = online::OnlineAuto {
                    server,
                    room: room.clone(),
                    char_id: entry.id,
                    stage: stage.clone(),
                    spectate,
                    guest,
                };
                let harness = game::Harness {
                    shot: shot.map(std::path::PathBuf::from),
                    frames: frames
                        .as_deref()
                        .map(|f| f.parse::<u32>().context("--frames expects a number"))
                        .transpose()?,
                    script: script.as_deref().map(play::parse_script),
                    script2: script2.as_deref().map(play::parse_script),
                    in_match: true,
                    ..Default::default()
                };
                return game::Shell::new(
                    config,
                    lib,
                    game::Transition::Online,
                    true,
                    harness,
                    None,
                )
                .with_online_auto(auto)
                .run();
            }
            let names: (String, Option<String>) =
                (p1.to_string(), rest.get(1).map(|s| s.to_string()));
            let p1 = resolve(&names.0)?;
            let p2 = names.1.as_deref().map(resolve).transpose()?;
            let stage = stage.or_else(|| lib.default_stage().map(|p| p.display().to_string()));
            let stage = stage
                .map(|s| -> anyhow::Result<std::path::PathBuf> {
                    let p = std::path::Path::new(&s);
                    if rugen_formats::vfs::exists(p) && p.extension().is_some_and(|e| e == "def") {
                        return Ok(p.to_path_buf());
                    }
                    let entry = lib
                        .stages()
                        .into_iter()
                        .find(|e| e.id.eq_ignore_ascii_case(&s))
                        .ok_or_else(|| anyhow::anyhow!("stage {s:?} not found"))?;
                    let (dir, m) = lib.prepare_stage(&entry)?;
                    Ok(m.def_path(&dir))
                })
                .transpose()?;
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
                tutorial: false,
                ids: [String::new(), String::new()],
                mode: None,
            };
            let harness = game::Harness {
                shot: shot.map(std::path::PathBuf::from),
                frames: frames
                    .map(|f| f.parse::<u32>().context("--frames expects a number"))
                    .transpose()?,
                script: script.as_deref().map(play::parse_script),
                script2: script2.as_deref().map(play::parse_script),
                record: record.map(std::path::PathBuf::from),
                keys: keys.as_deref().map(game::parse_keys).unwrap_or_default(),
                in_match: false,
            };
            let shell = game::Shell::new(
                config,
                lib,
                game::Transition::Fight(setup),
                true,
                harness,
                None,
            );
            shell.run()
        }
        Some("screenpack-gen") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let ttf = opt("--font")
                .map(std::path::PathBuf::from)
                .or_else(fontgen::default_ttf)
                .ok_or_else(|| anyhow::anyhow!("no bold TTF found; pass --font file.ttf"))?;
            let bytes = std::fs::read(&ttf).with_context(|| ttf.display().to_string())?;
            let motif = opt("--motif").unwrap_or_else(|| "motif".into());
            let out = opt("--out").unwrap_or_else(|| "motif/themes/dojo".into());
            screenpack::generate(
                &bytes,
                std::path::Path::new(&motif),
                std::path::Path::new(&out),
            )?;
            println!(
                "wrote the rugen screenpack to {out}/ (font {})",
                ttf.display()
            );
            Ok(())
        }
        Some("ui-gen") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let theme = opt("--theme");
            let motif = opt("--motif").unwrap_or_else(|| "motif".into());
            let out = opt("--out").unwrap_or_else(|| match &theme {
                Some(t) => format!("motif/themes/{t}"),
                None => "motif/themes/dojo".into(),
            });
            let ttf = opt("--font")
                .map(std::path::PathBuf::from)
                .or_else(fontgen::default_ttf);
            let bytes = match &ttf {
                Some(p) => Some(std::fs::read(p).with_context(|| p.display().to_string())?),
                None => None,
            };
            match theme.as_deref() {
                Some("ink") => {
                    inkgen::generate(
                        std::path::Path::new(&motif),
                        std::path::Path::new(&out),
                        bytes.as_deref(),
                    )?;
                    println!("wrote the Shonen Ink theme to {out}/");
                }
                Some("dojo") | None => {
                    uigen::generate(
                        bytes.as_deref(),
                        std::path::Path::new(&motif),
                        std::path::Path::new(&out),
                    )?;
                    println!("wrote the Neon Dojo theme to {out}/ (fonts from {motif}/font/src)");
                }
                Some(other) => {
                    anyhow::bail!("unknown theme {other:?}: ui-gen knows `ink` and `dojo`")
                }
            }
            Ok(())
        }
        Some("kit-import") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let motif = opt("--motif").unwrap_or_else(|| "motif".into());
            let out = opt("--out").unwrap_or_else(|| "motif/themes/dojo".into());
            let n = kit::import(std::path::Path::new(&motif), std::path::Path::new(&out))?;
            println!("wrote {n} kit pieces to {out}/ui/kit/");
            Ok(())
        }
        Some("pixelize") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let values: Vec<String> = ["--width", "--palette"]
                .iter()
                .filter_map(|f| opt(f))
                .collect();
            let positional: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--"))
                .filter(|a| !values.contains(a))
                .collect();
            let (Some(input), Some(output)) = (positional.first(), positional.get(1)) else {
                anyhow::bail!(
                    "usage: rugen pixelize <in.png|webp> <out.webp> [--width N] [--palette p.gpl] [--dither] [--outline]"
                );
            };
            let palette = match opt("--palette") {
                Some(p) => pixelize::load_gpl(std::path::Path::new(&p))?,
                None => pixelize::full_palette(),
            };
            let width = opt("--width")
                .map(|w| w.parse::<u32>().context("--width expects a number"))
                .transpose()?;
            let (w, h) = pixelize::pixelize_file(
                std::path::Path::new(input),
                std::path::Path::new(output),
                width,
                &palette,
                args.iter().any(|a| a == "--dither"),
                args.iter()
                    .any(|a| a == "--outline")
                    .then_some(pixelize::BLACK),
            )?;
            println!("wrote {output} ({w}x{h}, {} colours)", palette.len());
            Ok(())
        }
        Some("stage-gen") => {
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| "motif/stages".into());
            stagegen::generate(std::path::Path::new(&out))?;
            println!("wrote the built-in stages to {out}/");
            Ok(())
        }
        Some("fontgen") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            // the motif's role fonts only (menu, small, title, numbers, code)
            if args.iter().any(|a| a == "--roles") {
                let motif = opt("--motif").unwrap_or_else(|| "motif".into());
                let out = opt("--out").unwrap_or_else(|| format!("{motif}/font"));
                fontgen::generate_roles_into(
                    std::path::Path::new(&motif),
                    std::path::Path::new(&out),
                    None,
                )?;
                println!("wrote the role fonts to {out}/");
                return Ok(());
            }
            let ttf = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .map(std::path::PathBuf::from)
                .or_else(fontgen::default_ttf)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "usage: rugen fontgen [font.ttf] --size N [--italic rows] [--condense f] [--rim] --out font.def [--preview x.png]"
                    )
                })?;
            let bytes = std::fs::read(&ttf).with_context(|| ttf.display().to_string())?;
            let style = fontgen::Style {
                size: opt("--size").and_then(|v| v.parse().ok()).unwrap_or(16),
                italic: opt("--italic").and_then(|v| v.parse().ok()).unwrap_or(3),
                condense: opt("--condense")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.9),
                rim: args.iter().any(|a| a == "--rim"),
                highlight: true,
            };
            let generator = fontgen::Generator::new(&bytes, style)?;
            let out = opt("--out").unwrap_or_else(|| "font.def".into());
            let out = std::path::Path::new(&out);
            let name = out
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            fontgen::write_font(&generator, out, &name)?;
            println!(
                "wrote {} (space {}, height {})",
                out.display(),
                generator.space_width(),
                generator.height()
            );
            if let Some(p) = opt("--preview") {
                let (w, h, rgba) = fontgen::preview(&generator, 0);
                play::write_png(std::path::Path::new(&p), w, h, &rgba)?;
                println!("preview {p}");
            }
            Ok(())
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
                script2: None,
                record: None,
                keys: Vec::new(),
                in_match: false,
            };
            let mut setup = r.setup();
            // recordings made before content files point into old cache folders
            setup.p1 = relocate(&lib, &setup.p1, false);
            setup.p2 = relocate(&lib, &setup.p2, false);
            setup.stage = setup.stage.map(|s| relocate(&lib, &s, true));
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
        Some("verify") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let root = opt("--root");
            let all = args.iter().any(|a| a == "--all");
            let lib = library(root.as_deref());
            let targets: Vec<rugen_content::Entry> = if all {
                lib.chars()
            } else {
                let name = args[1..]
                    .iter()
                    .filter(|a| !a.starts_with("--"))
                    .find(|a| Some(*a) != root.as_ref())
                    .ok_or_else(|| {
                        anyhow::anyhow!("usage: rugen verify <char> | --all [--root dir]")
                    })?;
                vec![
                    lib.find_char(name)
                        .ok_or_else(|| anyhow::anyhow!("character {name:?} not found"))?,
                ]
            };
            let sparring = verify::sparring_partner(&lib);
            let mut broken = 0;
            for entry in targets {
                let (dir, mut m) = lib.prepare_char(&entry)?;
                let report =
                    verify::verify(&m.def_path(&dir), &lib.data_dir(), sparring.as_deref());
                report.print();
                if report.verdict == verify::Verdict::Broken {
                    broken += 1;
                }
                m.verify = Some(report.info());
                rugen_content::Library::save_manifest(&dir, &m)?;
            }
            if broken > 0 {
                std::process::exit(2);
            }
            Ok(())
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
            let lib = library(root.as_deref());
            let entry = lib
                .find_char(name)
                .ok_or_else(|| anyhow::anyhow!("character {name:?} not found"))?;
            let (dir, m) = lib.prepare_char(&entry)?;
            let t = std::time::Instant::now();
            let data = rugen_sim::CharData::load_with_data(&m.def_path(&dir), &lib.data_dir())?;
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
        Some("export") => {
            let force = args.iter().any(|a| a == "--force");
            let rest: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
            let Some(file) = rest.first() else {
                anyhow::bail!("usage: rugen export <file.rgc|file.rgs> [out_dir] [--force]");
            };
            let file = std::path::Path::new(file.as_str());
            let out = rest
                .get(1)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(
                        file.file_stem()
                            .map(|s| s.to_os_string())
                            .unwrap_or_default(),
                    )
                });
            let t = std::time::Instant::now();
            let done = rugen_content::export::export(file, &out, force)?;
            println!(
                "wrote {} files ({:.1} MB) to {} in {:.1?}; main def: {}",
                done.files,
                done.bytes as f64 / 1e6,
                out.display(),
                t.elapsed(),
                done.def
            );
            Ok(())
        }
        Some("convert") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1).cloned())
            };
            let root = opt("--root");
            let stage = args.iter().any(|a| a == "--stage");
            let rest: Vec<&String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--") && Some(*a) != root.as_ref())
                .collect();
            let Some(src) = rest.first() else {
                anyhow::bail!(
                    "usage: rugen convert <folder|archive.zip|archive.rar> [out.rgc|out.rgs] [--stage] [--root dir]"
                );
            };
            let lib = rugen_content::Library::discover(root.as_deref().map(std::path::Path::new));
            let src = std::path::Path::new(src.as_str());
            let kind = if stage {
                rugen_formats::rgc::Kind::Stage
            } else {
                rugen_formats::rgc::Kind::Char
            };
            let id = src
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let out = rest
                .get(1)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(format!("{id}.{}", kind.ext())));
            let report = rugen_content::convert::convert_path(src, &out, kind, &lib.data_dir())?;
            println!(
                "{} → {}: {}{}",
                src.display(),
                out.display(),
                report.summary(),
                report
                    .fingerprint
                    .as_deref()
                    .map(|f| format!(", fingerprint {}", rugen_fair::short(f)))
                    .unwrap_or_else(|| ", no fingerprint".into())
            );
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
                script2: opt("--script2").as_deref().map(play::parse_script),
                record: None,
                keys: opt("--keys")
                    .as_deref()
                    .map(game::parse_keys)
                    .unwrap_or_default(),
                in_match: false,
            };
            game::Shell::new(config, lib, game::Transition::Title, false, harness, None).run()
        }
        _ => {
            eprintln!(
                "rugen {}\n\nusage:\n  rugen view <char.def|file.sff> [action] [--shot out.png]\n  rugen play <char> [char2] [--stage s] [--root dir] [--shot out.png]\n  rugen list [--root dir]\n  rugen check <char> [--root dir]\n  rugen convert <folder|archive> [out.rgc] [--stage]\n  rugen export <file.rgc|file.rgs> [out_dir] [--force]",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
