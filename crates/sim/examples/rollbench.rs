//! Cost of one rollback, by part: snapshot (save), restore (load), one
//! re-simulated frame, and the checksum ggrs stores with each snapshot.
//!
//! `cargo run --release -p rugen-sim --example rollbench -- <p1.def> <p2.def> <data_dir> [frames]`
//!
//! Plays `frames` (default 3600) frames of pseudo-random inputs with both
//! sides on AI level 4 (busy matches: hits, projectiles, explods), then
//! prints mean / p99 microseconds per part and the deepest rollback that
//! still fits in one 16.67 ms frame next to rendering (`--render-ms`,
//! default 4).

use std::path::PathBuf;
use std::time::Instant;

use rugen_sim::{Buttons, CharData, Stage, World};

fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    let p99 = v[(v.len() * 99 / 100).min(v.len() - 1)];
    (mean, p99, v[v.len() - 1])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: rollbench <p1.def> <p2.def> <data_dir> [frames] [--render-ms N]");
        std::process::exit(2);
    }
    let data_dir = PathBuf::from(&args[3]);
    let frames: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3600);
    let render_ms: f64 = args
        .iter()
        .position(|a| a == "--render-ms")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(4.0);
    let d1 = CharData::load_with_data(&PathBuf::from(&args[1]), &data_dir).expect("p1 loads");
    let d2 = if args[2] == args[1] {
        d1.clone()
    } else {
        CharData::load_with_data(&PathBuf::from(&args[2]), &data_dir).expect("p2 loads")
    };
    let mut world = World::new(Stage::default(), 7);
    world.add_player(d1, 0, 1);
    world.add_player(d2, 1, 2);
    world.players[0].ai_level = 4;
    world.players[1].ai_level = 4;
    world.start_round();

    let mut seed: u32 = 0x9E37_79B9;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let spikes = args.iter().any(|a| a == "--spikes");
    let (mut save, mut load, mut step, mut sum) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut ring: Vec<World> = (0..10).map(|_| world.clone()).collect();
    let mut max_entities = (0usize, 0usize);
    for f in 0..frames {
        let input = |r: u32| {
            Buttons::from_bits(if r % 4 == 0 {
                (r >> 8) as u16 & 0x7ff
            } else {
                0
            })
        };
        let inputs = [input(rnd()), input(rnd())];
        let t = Instant::now();
        world.step(&inputs);
        let us = t.elapsed().as_secs_f64() * 1e6;
        step.push(us);
        if spikes && us > 1500.0 {
            let helpers = world.players.iter().filter(|p| p.helper_index > 0).count();
            let states: Vec<String> = world
                .players
                .iter()
                .filter(|p| !p.removed)
                .map(|p| format!("{}:{}", p.helper_index, p.state_no))
                .collect();
            eprintln!(
                "spike frame {} {us:.0} us: {} players ({helpers} helpers), {} explods, {} warnings, states {:?}",
                world.frame,
                world.players.len(),
                world.explods.len(),
                world.warnings.len(),
                states
            );
        }
        world.warnings.clear();
        let t = Instant::now();
        let c = world.checksum();
        sum.push(t.elapsed().as_secs_f64() * 1e6);
        std::hint::black_box(c);
        // what ggrs does: `cell.save(world.clone())` into a ring of
        // max_prediction + 2 cells (the evicted snapshot is dropped) ...
        let t = Instant::now();
        let s = world.clone();
        let n = ring.len();
        ring[f % n] = s;
        save.push(t.elapsed().as_secs_f64() * 1e6);
        if f % 7 == 0 {
            // ... and `*world = cell.load()` (a clone of the stored state)
            let t = Instant::now();
            let w = ring[(f + 3) % ring.len()].clone();
            world = w;
            load.push(t.elapsed().as_secs_f64() * 1e6);
            world.clone_from(&ring[f % ring.len()]);
        }
        max_entities.0 = max_entities.0.max(world.explods.len());
        max_entities.1 = max_entities.1.max(world.projs.len());
        if world.round.match_over {
            world.start_round();
        }
    }
    let p = |name: &str, v: Vec<f64>| {
        let (m, p99, max) = stats(v);
        println!("{name:<10} mean {m:8.1} us   p99 {p99:8.1} us   max {max:8.1} us");
        (m, p99)
    };
    println!(
        "{} vs {}: {frames} frames, peak {} explods / {} projectiles, final checksum {:016x}",
        args[1],
        args[2],
        max_entities.0,
        max_entities.1,
        world.checksum()
    );
    let (sv, sv99) = p("save", save);
    let (ld, _) = p("load", load);
    let (st, st99) = p("step", step);
    let (cs, _) = p("checksum", sum);
    // a rollback of N frames: one load, then N x (step + checksum + save)
    let per = st + cs + sv;
    let per99 = st99 + cs + sv99;
    let budget = 16_667.0 - render_ms * 1000.0;
    println!(
        "per resimulated frame {per:.1} us (p99 {per99:.1}); one 16.67 ms frame with {render_ms} ms \
         of rendering fits a {:.0}-frame rollback (p99 frames: {:.0})",
        ((budget - ld) / per).floor(),
        ((budget - ld) / per99).floor()
    );
}
