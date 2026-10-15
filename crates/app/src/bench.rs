//! `rugen bench`: the engine's performance numbers in one headless run, as
//! a markdown table (the 0.7.0 before / after method, see docs/performance).
//!
//! ```text
//! rugen bench [--root dir] [--chars A,B[,C,D]] [--frames n] [--out file.md]
//!             [--convert <zip|folder>] [--no-render] [--no-teams] [--backend gl|vulkan|...]
//! ```
//!
//! Sections:
//! - **sim**: a 1v1 of the first two fighters, computer level 4 on both
//!   sides with button noise (busy matches: supers, helpers, explods):
//!   `World::step` mean / p99 / max, a snapshot (`clone`), the fast checksum,
//!   and a real 8-frame rollback (restore a snapshot, re-simulate 8 frames)
//!   every 8 frames; the deep checksum at the end (so two builds can be
//!   compared for identical play).
//! - **teams**: the same for a Simul 4v4 (fighters repeated when fewer are
//!   given) and **arena**: `examples/arena/arena.def` (items, moving and
//!   dropping platforms, wind) with the arena AI.
//! - **load**: `CharData::load_with_data` per fighter and the process peak
//!   RSS (Linux `VmHWM`).
//! - **render**: a child `rugen play … --frames 900` with `RUGEN_FRAMESTATS=1`
//!   (needs a display): update / build / submit CPU ms, draw calls, live
//!   textures at the end, startup and first-frame timings.
//! - **convert**: a child `rugen convert` of `--convert` (its own process, so
//!   the peak RSS belongs to it).

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use rugen_sim::headless::{MatchSpec, TeamSpec, build_team_world, build_world};
use rugen_sim::team::TeamMode;
use rugen_sim::world::Movement;
use rugen_sim::{Buttons, CharData, World};

/// Deterministic button noise, as the golden tests: a held direction and a
/// few presses per side, changing every 6..20 ticks.
fn noise(frames: usize, sides: usize, seed: u64) -> Vec<Vec<Buttons>> {
    let mut s = seed;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut held = vec![0u16; sides];
    let mut left = vec![0u32; sides];
    (0..frames)
        .map(|_| {
            (0..sides)
                .map(|k| {
                    if left[k] == 0 {
                        let r = next();
                        let dir = [0u16, 1, 2, 4, 8, 1 | 8, 2 | 8, 1 | 4, 2 | 4][(r % 9) as usize];
                        let btn = ((r >> 8) as u16 & 0x7f) << 4;
                        let btn = if (r >> 20) % 3 == 0 {
                            btn & (btn >> 1)
                        } else {
                            0
                        };
                        held[k] = dir | btn;
                        left[k] = 6 + ((r >> 32) % 15) as u32;
                    }
                    left[k] -= 1;
                    Buttons::from_bits(held[k])
                })
                .collect()
        })
        .collect()
}

#[derive(Default)]
struct Series(Vec<f64>);

impl Series {
    fn push(&mut self, t: Instant) {
        self.0.push(t.elapsed().as_secs_f64() * 1e3);
    }
    fn mean(&self) -> f64 {
        self.0.iter().sum::<f64>() / self.0.len().max(1) as f64
    }
    fn pct(&self, p: usize) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        let mut v = self.0.clone();
        v.sort_by(f64::total_cmp);
        v[(v.len() * p / 100).min(v.len() - 1)]
    }
    fn max(&self) -> f64 {
        self.0.iter().copied().fold(0.0, f64::max)
    }
    fn cell(&self) -> String {
        format!(
            "{:.3} / {:.3} / {:.3}",
            self.mean(),
            self.pct(99),
            self.max()
        )
    }
}

struct SimResult {
    step: Series,
    save: Series,
    checksum: Series,
    rollback8: Series,
    peak_explods: usize,
    peak_players: usize,
    deep: u64,
    fast: u64,
}

/// Play `inputs` through `world`, timing every part of a rollback.
fn drive(mut world: World, inputs: &[Vec<Buttons>]) -> SimResult {
    let mut r = SimResult {
        step: Series::default(),
        save: Series::default(),
        checksum: Series::default(),
        rollback8: Series::default(),
        peak_explods: 0,
        peak_players: 0,
        deep: 0,
        fast: 0,
    };
    let mut snapshot = world.clone();
    let mut since_snapshot: Vec<&Vec<Buttons>> = Vec::with_capacity(8);
    for (f, input) in inputs.iter().enumerate() {
        let t = Instant::now();
        world.step(input);
        r.step.push(t);
        let t = Instant::now();
        let _ = world.checksum();
        r.checksum.push(t);
        r.peak_explods = r.peak_explods.max(world.explods.len());
        r.peak_players = r.peak_players.max(world.players.len());
        since_snapshot.push(input);
        if f % 8 == 7 {
            // a rollback: back to the snapshot, re-simulate the 8 frames,
            // and confirm the state is the one we had (bit-identical)
            let before = world.checksum();
            let t = Instant::now();
            let mut again = snapshot.clone();
            for i in &since_snapshot {
                again.step(i);
            }
            r.rollback8.push(t);
            assert_eq!(again.checksum(), before, "rollback re-simulation diverged");
            let t = Instant::now();
            snapshot.clone_from(&world);
            r.save.push(t);
            since_snapshot.clear();
        }
    }
    r.fast = world.checksum();
    r.deep = world.deep_checksum();
    r
}

fn vm_hwm_mb() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let l = s.lines().find(|l| l.starts_with("VmHWM"))?;
    let kb: f64 = l.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

struct Table(Vec<(String, String, String)>);

impl Table {
    fn row(&mut self, section: &str, metric: &str, value: impl std::fmt::Display) {
        self.0
            .push((section.into(), metric.into(), value.to_string()));
    }
    fn markdown(&self) -> String {
        let mut s = String::from("| section | metric | value |\n|---|---|---|\n");
        for (a, b, c) in &self.0 {
            s.push_str(&format!("| {a} | {b} | {c} |\n"));
        }
        s
    }
}

/// `rugen bench …` (see the module doc).
pub fn run(args: &[String]) -> Result<()> {
    if let Some(i) = args.iter().position(|a| a == "--convert-child") {
        return convert_child(&args[i + 1..]);
    }
    let root = opt(args, "--root");
    let lib = rugen_content::Library::discover(root.as_deref().map(Path::new));
    let frames: usize = opt(args, "--frames")
        .map(|f| f.parse().context("--frames expects a number"))
        .transpose()?
        .unwrap_or(3600);
    let want: Vec<String> = match opt(args, "--chars") {
        Some(c) => c.split(',').map(|s| s.trim().to_string()).collect(),
        None => {
            let all = lib.chars();
            let pick = |id: &str| all.iter().find(|e| e.id.eq_ignore_ascii_case(id));
            let mut v: Vec<String> = ["Goku", "G6_Sasuke_Rinnegan"]
                .iter()
                .filter_map(|id| pick(id).map(|e| e.id.clone()))
                .collect();
            for e in &all {
                if v.len() >= 2 {
                    break;
                }
                if !v.iter().any(|x| x == &e.id) {
                    v.push(e.id.clone());
                }
            }
            v
        }
    };
    anyhow::ensure!(
        want.len() >= 2,
        "bench needs two fighters: `--chars A,B` or a root with two in chars/"
    );
    let mut table = Table(Vec::new());
    table.row("build", "rugen", env!("CARGO_PKG_VERSION"));
    table.row("build", "frames", frames);

    let mut defs: Vec<PathBuf> = Vec::new();
    for id in &want {
        let entry = lib
            .find_char(id)
            .ok_or_else(|| anyhow::anyhow!("character {id:?} not found"))?;
        let (dir, m) = lib.prepare_char(&entry)?;
        defs.push(m.def_path(&dir));
    }
    let data_dir = lib.data_dir();
    let motif = lib.motif_dir().join("system.def");
    let stage = lib.default_stage();
    fn spec<'a>(
        p1: &'a Path,
        p2: &'a Path,
        stage: Option<&'a Path>,
        seed: u64,
        data_dir: &'a Path,
        motif: &'a Path,
    ) -> MatchSpec<'a> {
        MatchSpec {
            p1,
            p2,
            stage,
            data_dir,
            motif,
            zoom: rugen_sim::headless::ONLINE_ZOOM,
            movement: Movement::default(),
            round_time: None,
            ai: [4, 4],
            fighter_size: Default::default(),
            seed,
            life: Some(5000),
        }
    }

    // load
    for (id, def) in want.iter().zip(&defs) {
        let t = Instant::now();
        let d = CharData::load_with_data(def, &data_dir)?;
        table.row(
            "load",
            &format!("{id} ms"),
            format!("{:.0}", t.elapsed().as_secs_f64() * 1e3),
        );
        drop(d);
    }
    if let Some(mb) = vm_hwm_mb() {
        table.row("load", "peak RSS MB after loads", format!("{mb:.0}"));
    }

    // sim 1v1
    let title = format!("{} vs {}", want[0], want[1]);
    let w = build_world(&spec(
        &defs[0],
        &defs[1],
        stage.as_deref(),
        7,
        &data_dir,
        &motif,
    ))?;
    let r = drive(w, &noise(frames, 2, 0x9E37_79B9));
    sim_rows(&mut table, "sim", &title, &r);

    // teams: Simul 4v4
    if !args.iter().any(|a| a == "--no-teams") {
        let pick = |i: usize| defs[i % defs.len()].as_path();
        let teams = TeamSpec {
            mode: [TeamMode::Simul, TeamMode::Simul],
            partners: [
                vec![pick(2), pick(3), pick(4)],
                vec![pick(3), pick(4), pick(5)],
            ],
            partner_ai: [4, 4],
        };
        let w = build_team_world(
            &spec(&defs[0], &defs[1], stage.as_deref(), 11, &data_dir, &motif),
            &teams,
        )?;
        let r = drive(w, &noise(frames, 2, 0x1234_5678));
        sim_rows(&mut table, "teams", "Simul 4v4", &r);
    }

    // arena
    let arena = [
        lib.root.join("examples/arena/arena.def"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/arena/arena.def"),
    ]
    .into_iter()
    .find(|p| p.exists());
    if let Some(arena) = arena {
        let mut w = build_world(&spec(
            &defs[0],
            &defs[1],
            Some(&arena),
            5,
            &data_dir,
            &motif,
        ))?;
        w.arena_ai = true;
        let r = drive(w, &noise(frames, 2, 0xfeed));
        sim_rows(&mut table, "arena", "items + hazards", &r);
    }
    if let Some(mb) = vm_hwm_mb() {
        table.row("sim", "peak RSS MB", format!("{mb:.0}"));
    }

    // render (child process with a window)
    if !args.iter().any(|a| a == "--no-render") {
        match render_child(
            &lib.root,
            &want[0],
            &want[1],
            opt(args, "--backend").as_deref(),
        ) {
            Ok(rows) => {
                for (k, v) in rows {
                    table.row("render", &k, v);
                }
            }
            Err(e) => table.row("render", "error", e),
        }
    }

    // convert (child process)
    if let Some(src) = opt(args, "--convert") {
        let out = std::env::temp_dir().join(format!("rugen-bench-{}.rgc", std::process::id()));
        let t = Instant::now();
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "bench",
                "--convert-child",
                &src,
                &out.to_string_lossy(),
                &data_dir.to_string_lossy(),
            ])
            .output()?;
        let wall = t.elapsed().as_secs_f64();
        let text = String::from_utf8_lossy(&output.stdout);
        table.row("convert", "source", &src);
        table.row("convert", "wall s", format!("{wall:.1}"));
        for l in text.lines() {
            if let Some((k, v)) = l.split_once('=') {
                table.row("convert", k, v);
            }
        }
        if !output.status.success() {
            table.row(
                "convert",
                "error",
                String::from_utf8_lossy(&output.stderr).trim(),
            );
        }
        let _ = std::fs::remove_file(&out);
    }

    let md = table.markdown();
    print!("{md}");
    if let Some(out) = opt(args, "--out") {
        std::fs::write(&out, &md).with_context(|| format!("writing {out}"))?;
    }
    Ok(())
}

fn sim_rows(table: &mut Table, section: &str, title: &str, r: &SimResult) {
    table.row(section, "match", title);
    table.row(section, "step ms mean / p99 / max", r.step.cell());
    table.row(
        section,
        "8-frame rollback ms mean / p99 / max",
        r.rollback8.cell(),
    );
    table.row(section, "snapshot ms mean / p99 / max", r.save.cell());
    table.row(section, "checksum ms mean / p99 / max", r.checksum.cell());
    table.row(
        section,
        "peak players / explods",
        format!("{} / {}", r.peak_players, r.peak_explods),
    );
    table.row(
        section,
        "final checksum / deep",
        format!("{:#018x} / {:#018x}", r.fast, r.deep),
    );
}

/// `rugen bench --convert-child <src> <out.rgc> <data_dir>`: convert in this
/// process and print `key=value` lines (its peak RSS is the conversion's).
fn convert_child(args: &[String]) -> Result<()> {
    let [src, out, data_dir, ..] = args else {
        anyhow::bail!("--convert-child <src> <out> <data_dir>");
    };
    let t = Instant::now();
    let report = rugen_content::convert::convert_path(
        Path::new(src),
        Path::new(out),
        rugen_formats::rgc::Kind::Char,
        Path::new(data_dir),
    )?;
    println!("ms={:.0}", t.elapsed().as_secs_f64() * 1e3);
    println!(
        "size MB={:.1}",
        std::fs::metadata(out).map(|m| m.len()).unwrap_or(0) as f64 / 1e6
    );
    if let Some(mb) = vm_hwm_mb() {
        println!("peak RSS MB={mb:.0}");
    }
    if let Some(fp) = report.fingerprint.as_deref() {
        println!("fingerprint={}", rugen_fair::short(fp));
    }
    Ok(())
}

/// A 900-frame harness fight in a child process with frame statistics on.
fn render_child(
    root: &Path,
    a: &str,
    b: &str,
    backend: Option<&str>,
) -> std::result::Result<Vec<(String, String)>, String> {
    let shot = std::env::temp_dir().join(format!("rugen-bench-{}.png", std::process::id()));
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.args([
        "play",
        a,
        b,
        "--root",
        &root.to_string_lossy(),
        "--ai",
        "4",
        "--frames",
        "900",
        "--shot",
        &shot.to_string_lossy(),
    ])
    .env("RUGEN_FRAMESTATS", "1");
    if let Some(b) = backend {
        cmd.env("WGPU_BACKEND", b);
    }
    let output = cmd.output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&shot);
    let text = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(text.lines().last().unwrap_or("harness failed").to_string());
    }
    let mut rows = Vec::new();
    let (mut update, mut build, mut submit) =
        (Series::default(), Series::default(), Series::default());
    let (mut draws, mut textures, mut uploads_kib) = (Vec::new(), 0u64, 0u64);
    let mut backend_used = String::new();
    for l in text.lines() {
        if let Some(rest) = l.split_once("framestats match:").map(|x| x.1) {
            // "63 fps | update 0.40ms (max 0.91) | ... | draws 57 quads 76 | uploads 56 (9728 KiB) textures 71 palettes 70"
            for part in rest.split('|') {
                let w: Vec<&str> = part.split_whitespace().collect();
                let num = |i: usize| -> f64 {
                    w.get(i)
                        .map(|s| {
                            s.trim_end_matches("ms")
                                .trim_matches(|c| c == '(' || c == ')')
                        })
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0.0)
                };
                match w.first().copied() {
                    Some("update") => update.0.push(num(1)),
                    Some("build") => build.0.push(num(1)),
                    Some("submit") => submit.0.push(num(1)),
                    Some("draws") => draws.push(num(1)),
                    Some("uploads") => {
                        uploads_kib += num(2) as u64;
                        textures = num(5) as u64;
                    }
                    _ => {}
                }
            }
        } else if let Some(x) = l.split_once("timing: ").map(|x| x.1) {
            rows.push(("timing".into(), x.to_string()));
        } else if l.contains("render: adapter") || l.contains("backend") {
            backend_used = l.split("] ").last().unwrap_or(l).to_string();
        }
    }
    if !backend_used.is_empty() {
        rows.push(("adapter".into(), backend_used));
    }
    rows.push((
        "update ms mean / max".into(),
        format!("{:.2} / {:.2}", update.mean(), update.max()),
    ));
    rows.push((
        "build ms mean / max".into(),
        format!("{:.2} / {:.2}", build.mean(), build.max()),
    ));
    rows.push((
        "submit ms mean / max".into(),
        format!("{:.2} / {:.2}", submit.mean(), submit.max()),
    ));
    let draws_mean = draws.iter().sum::<f64>() / draws.len().max(1) as f64;
    rows.push(("draw calls per frame".into(), format!("{draws_mean:.0}")));
    rows.push(("textures alive at end".into(), textures.to_string()));
    rows.push(("uploaded KiB".into(), uploads_kib.to_string()));
    Ok(rows)
}
