//! Trace a recorded match (an app replay `.json`) headless and report where
//! a fighter in a get-hit state is stopped by the screen bounds.
//!
//! `cargo run --release -p rugen-sim --example replaybound -- <replay.json>
//!  <root> [--from-root /old/root] [--zoom 0.6,0.85] [--all]`
//!
//! Paths in the replay that start with `--from-root` are moved to `<root>`
//! (a copy of the install). Prints one line per frame where a root fighter
//! with movetype H sits on a bound edge while moving outward (or on every
//! frame with `--all`), with the camera and its limits.

use std::path::{Path, PathBuf};

use rugen_cns::state::MoveType;
use rugen_sim::headless::{MatchSpec, build_world};
use rugen_sim::world::Movement;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let root = PathBuf::from(&args[2]);
    let from = opt("--from-root").unwrap_or_else(|| "/home/axelrevana/RUGEN".into());
    let all = args.iter().any(|a| a == "--all");
    let zoom: Vec<f32> = opt("--zoom")
        .unwrap_or_else(|| "0.6,0.85".into())
        .split(',')
        .filter_map(|v| v.parse().ok())
        .collect();
    let r: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[1]).expect("replay")).expect("json");
    let fix = |v: &serde_json::Value| -> PathBuf {
        let s = v.as_str().unwrap_or_default();
        PathBuf::from(s.replacen(&from, &root.to_string_lossy(), 1))
    };
    let (p1, p2) = (fix(&r["p1"]), fix(&r["p2"]));
    let stage = r["stage"].as_str().map(|_| fix(&r["stage"]));
    let size = match r["fighter_size"].as_str().unwrap_or("none") {
        "normal" => rugen_sim::scale::FighterSize::Normal,
        "chibi" => rugen_sim::scale::FighterSize::Chibi,
        "small" => rugen_sim::scale::FighterSize::Small,
        _ => rugen_sim::scale::FighterSize::None,
    };
    let data = root.join("data");
    let motif = root.join("motif/themes/ink");
    let ai = [
        r["ai"][0].as_i64().unwrap_or(0) as i32,
        r["ai"][1].as_i64().unwrap_or(0) as i32,
    ];
    let spec = MatchSpec {
        p1: &p1,
        p2: &p2,
        stage: stage.as_deref(),
        data_dir: &data,
        motif: Path::new(&motif),
        zoom: (zoom[0], zoom[1]),
        movement: Movement::default(),
        round_time: r["round_time"].as_i64().map(|v| v as i32),
        ai,
        fighter_size: size,
        seed: r["seed"].as_u64().unwrap_or(1),
        life: r["life"].as_i64().map(|v| v as i32),
    };
    let mut w = build_world(&spec).expect("world");
    if let Some(run) = r.get("run").filter(|v| v.is_object()) {
        w.round.config.match_wins = run["match_wins"].as_u64().unwrap_or(2).max(1) as u8;
        if w.round.config.match_wins <= 1 {
            w.round.config.max_drawgames = 0;
        }
        if let Some(l) = run["p1_life"].as_i64() {
            let p = &mut w.players[0];
            p.life = (l as i32).clamp(1, p.life_max.max(1));
        }
    }
    w.arena_ai = r["arena_ai"].as_bool().unwrap_or(false);
    let st = w.stage.clone();
    println!(
        "stage {:?} width {} bounds cam {}..{} player {}..{} screen {} {} zoom {}..{} tension {}",
        st.name,
        st.width,
        st.bound_left,
        st.bound_right,
        st.left_bound,
        st.right_bound,
        st.screen_left,
        st.screen_right,
        st.zoom_out,
        st.zoom_in,
        st.tension
    );
    let inputs = r["inputs"].as_array().cloned().unwrap_or_default();
    let mut walls = 0;
    let mut invisible = 0;
    for f in &inputs {
        let b = |k: usize| rugen_sim::Buttons::from_bits(f[k].as_u64().unwrap_or(0) as u16);
        w.step(&[b(0), b(1)]);
        let (xmin, xmax) = w.play_bounds();
        for i in 0..2 {
            let Some(i) = w.side_leader(i) else { continue };
            let p = &w.players[i];
            let s = w.local_scale(i);
            let x = p.pos[0] * s;
            let at_edge = x <= xmin + 25.0 || x >= xmax - 25.0;
            let outward = (x <= xmin + 25.0 && p.vel[0] * p.facing < 0.0)
                || (x >= xmax - 25.0 && p.vel[0] * p.facing > 0.0);
            let hit = p.move_type == MoveType::H;
            if all || (hit && at_edge && outward) {
                if hit && at_edge && outward {
                    walls += 1;
                    // held inside the visible screen: an invisible wall
                    let (sl, sr) = (w.screen_left(), w.screen_right());
                    let margin = 25.0 + w.stage.screen_left.max(w.stage.screen_right);
                    if x > sl + margin && x < sr - margin {
                        invisible += 1;
                    }
                }
                println!(
                    "f{} P{} st {} x {:.1} vel {:.2} y {:.1} cam {:.1} zoom {:.3} play {:.1}..{:.1} screen {:.1}..{:.1}",
                    w.frame,
                    i,
                    p.state_no,
                    x,
                    p.vel[0] * p.facing,
                    p.pos[1] * s,
                    w.cam_x,
                    w.cam_zoom,
                    xmin,
                    xmax,
                    w.screen_left(),
                    w.screen_right()
                );
            }
        }
    }
    println!(
        "frames {} wall frames {walls} invisible {invisible} checksum {:#x}",
        w.frame,
        w.checksum()
    );
}
