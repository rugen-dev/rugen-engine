//! `rugen-gym`: one real-time match between two outside players over JSON
//! lines.
//!
//! ```text
//! rugen-gym --p1 <a.def> --p2 <b.def> [--stage <s.def>] [--root dir]
//!           [--data dir] [--motif dir] [--seed n] [--round-time s]
//!           [--rounds n] [--life n] [--step-ticks n] [--max-plan n]
//!           [--cpu1 level] [--cpu2 level] [--no-lab]
//!           [--speed x] [--replay out.json] [--live out.jsonl]
//!           [--names A,B]
//! ```
//!
//! The match runs at 60 ticks a second (`--speed 2` twice as fast, for
//! bots) and never waits. stdout: a `hello` line, then an `obs` line for a
//! side (`obs.side`) each time its previous answer has come in, and a
//! `result` line at the end. stdin, whenever a player is ready, its brain
//! (see `rugen_gym::brain::Book`): `{"side":0,"rules":[{"when":
//! ["he_attacks","distance<40"],"do":["block"]},{"do":["approach"]}],
//! "say":"why"}`, read every tick from then on; or a plan: `{"side":0,"steps":[{"card":"approach",
//! "until":"distance<45"},{"card":"stand_light_punch","hit":1}],
//! "rules":[{"if":"i_am_hit","goto":1}],"say":"why"}`. The match runs the
//! plan by itself while the player thinks about the next one. `--cpu2 6`
//! gives side 2 to the game's own computer player (level 1 to 8), which is
//! never asked. `--root` is
//! the game folder (default: the current one): `data/` and
//! `motif/themes/ink` are looked for there.
//!
//! `--live file` writes the match as it happens, for the game to follow
//! (`rugen replay --live file`): a first line with the replay's header,
//! then a line per tick (`{"i":[p1,p2]}`) and per event (a side asked, a
//! plan, a step that ended), and `{"end":true}`.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use rugen_gym::{Config, Env};

fn data_dir(root: &Path) -> PathBuf {
    [root.join("data"), root.join("reference/mugen-1.1b1/data")]
        .into_iter()
        .find(|d| d.join("common1.cns").exists())
        .unwrap_or_else(|| root.join("data"))
}

/// A fighter or stage as given: a `.def`, or a packed `.rgc` / `.rgs` (or
/// a folder), whose `.def` is then looked for inside (the one named like
/// it first, else the only one).
fn def_of(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("def")) {
        return p;
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase().replace(' ', "_"))
        .unwrap_or_default();
    // every .def of the pack, wherever it sits (or of the folder's top)
    let files: Vec<String> = match rugen_formats::vfs::archive(&p) {
        Ok(a) => a.files().map(|e| e.path.clone()).collect(),
        Err(_) => rugen_formats::vfs::read_dir(&p)
            .into_iter()
            .filter(|(_, dir)| !dir)
            .map(|(name, _)| name)
            .collect(),
    };
    let defs: Vec<String> = files
        .into_iter()
        .filter(|f| f.to_lowercase().ends_with(".def"))
        .collect();
    let base = |d: &String| {
        let name = d.rsplit('/').next().unwrap_or(d).to_lowercase();
        name.trim_end_matches(".def").replace(' ', "_")
    };
    // the one named like the pack; else not an intro or an ending, the
    // shortest path first
    let mut rest: Vec<&String> = defs
        .iter()
        .filter(|d| !["intro", "ending"].contains(&base(d).as_str()))
        .collect();
    rest.sort_by_key(|d| d.len());
    let named = defs
        .iter()
        .find(|d| base(d) == stem || stem.ends_with(&format!("_{}", base(d))));
    match named.or(rest.first().copied()) {
        Some(d) => p.join(d),
        None => p,
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "usage: rugen-gym --p1 <a.def> --p2 <b.def> [--stage <s.def>] [--root dir] \
             [--data dir] [--motif dir] [--seed n] [--round-time s] [--rounds n] [--life n] \
             [--step-ticks n] [--max-plan n] [--speed x] [--replay out.json] [--live out.jsonl] \
             [--names A,B]"
        );
        std::process::exit(2);
    }
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let num = |name: &str| -> Result<Option<i64>> {
        opt(name)
            .map(|v| {
                v.parse::<i64>()
                    .with_context(|| format!("{name} expects a number"))
            })
            .transpose()
    };
    let root = PathBuf::from(opt("--root").unwrap_or("."));
    let p1 = def_of(opt("--p1").context("--p1 <fighter.def | fighter.rgc> is required")?);
    let p2 = def_of(opt("--p2").context("--p2 <fighter.def | fighter.rgc> is required")?);
    let data = opt("--data").map_or_else(|| data_dir(&root), PathBuf::from);
    let motif = opt("--motif").map_or_else(
        || {
            let ink = root.join("motif/themes/ink");
            if ink.is_dir() { ink } else { data.clone() }
        },
        PathBuf::from,
    );
    let mut cfg = Config::new(p1, p2, data, motif);
    cfg.stage = opt("--stage").map(def_of);
    if let Some(s) = opt("--seed") {
        cfg.seed = s.parse().context("--seed expects a number")?;
    }
    cfg.round_time = num("--round-time")?.map(|v| v as i32);
    cfg.life = num("--life")?.map(|v| v as i32);
    cfg.match_wins = num("--rounds")?.map(|v| v.clamp(1, 9) as u8);
    if let Some(v) = num("--step-ticks")? {
        cfg.step_ticks = v.clamp(1, 30) as u32;
    }
    if let Some(v) = num("--max-plan")? {
        cfg.max_plan = v.clamp(1, 600) as u32;
    }
    // a side played by the game's own computer player, at a level 1 to 8
    for (side, flag) in ["--cpu1", "--cpu2"].into_iter().enumerate() {
        if let Some(v) = num(flag)? {
            cfg.cpu[side] = v.clamp(1, 8) as i32;
        }
    }
    // every move of both fighters is measured first, unless told not to
    cfg.lab = !args.iter().any(|a| a == "--no-lab");
    let speed: f64 = match opt("--speed") {
        Some(s) => s.parse().context("--speed expects a number")?,
        None => 1.0,
    };
    anyhow::ensure!(
        speed > 0.0 && speed <= 1000.0,
        "--speed is between 0 and 1000"
    );
    let tick = Duration::from_secs_f64(1.0 / (60.0 * speed));
    let names = opt("--names").unwrap_or("p1,p2");
    let (n1, n2) = names.split_once(',').unwrap_or((names, "p2"));

    let mut env = Env::new(cfg)?;
    // answers come in on their own thread: the match never blocks on stdin
    let (tx, answers) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // the match as it happens, for a viewer (the header first: no inputs yet)
    let mut live = match opt("--live") {
        Some(path) => {
            let path = Path::new(path);
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = std::io::BufWriter::new(
                std::fs::File::create(path).with_context(|| path.display().to_string())?,
            );
            writeln!(f, "{}", env.replay([n1, n2], &root))?;
            f.flush()?;
            Some(f)
        }
        None => None,
    };
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", env.hello())?;
    out.flush()?;
    let mut next = Instant::now();
    while !env.over() {
        loop {
            match answers.try_recv() {
                Ok(line) => {
                    let v: serde_json::Value = serde_json::from_str(&line).context(
                        "a JSON line like {\"side\":0,\"steps\":[{\"card\":\"approach\"}]}",
                    )?;
                    let side = v["side"]
                        .as_u64()
                        .filter(|s| *s < 2)
                        .context("side is 0 or 1")?;
                    if let Some(e) = env.submit(side as usize, &v) {
                        eprintln!("frame {} p{}: {e}", env.world.frame, side + 1);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    bail!("stdin closed at frame {}", env.world.frame)
                }
            }
        }
        for side in 0..2 {
            if env.wants(side) {
                writeln!(out, "{}", env.observation(side))?;
                out.flush()?;
            }
        }
        env.tick();
        if let Some(f) = live.as_mut() {
            for e in env.take_feed() {
                writeln!(f, "{e}")?;
            }
            if let Some(i) = env.inputs.last() {
                writeln!(f, "{{\"i\":[{},{}]}}", i[0], i[1])?;
            }
            f.flush()?;
        }
        // a fixed timeline: a slow tick is caught up, never drifts
        next += tick;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    let played = env.inputs.len();
    env.finish();
    if let Some(f) = live.as_mut() {
        for e in env.take_feed() {
            writeln!(f, "{e}")?;
        }
        for i in &env.inputs[played..] {
            writeln!(f, "{{\"i\":[{},{}]}}", i[0], i[1])?;
        }
        writeln!(f, "{{\"end\":true}}")?;
        f.flush()?;
    }
    let mut result = env.result();
    if let Some(path) = opt("--replay") {
        let path = Path::new(path);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec(&env.replay([n1, n2], &root))?)
            .with_context(|| path.display().to_string())?;
        result["replay"] = path.display().to_string().into();
    }
    writeln!(out, "{result}")?;
    Ok(())
}
