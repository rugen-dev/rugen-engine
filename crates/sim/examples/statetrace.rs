//! Frame-by-frame trace of a headless match, for chasing a fighter's
//! behaviour without a window.
//!
//! `cargo run --release -p rugen-sim --example statetrace -- <p1.def> <p2.def> <data_dir>
//!  [--frames N] [--from F] [--force P:STATE@F]... [--keys1 SCRIPT] [--keys2 SCRIPT]
//!  [--power] [--size none|normal|...] [--only P]`
//!
//! Prints, from frame `--from` on, one line per player (helpers too): index,
//! id, parent, state, prev state, anim / elem / time, pos, vel, ctrl,
//! movetype, assert flags, trans and palfx. `--force` puts root player P
//! (1 or 2) in STATE at frame F with full power; `--keys` scripts are the
//! `rugen play --script` syntax (`R*40,.*20,D,DR,R,x`), starting at frame 0.

use std::path::PathBuf;

use rugen_sim::{Buttons, CharData, Stage, World};

fn script(s: &str) -> Vec<Buttons> {
    let mut out = Vec::new();
    for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let (keys, n) = match tok.split_once('*') {
            Some((k, n)) => (k, n.trim().parse().unwrap_or(1)),
            None => (tok, 1),
        };
        let mut b = Buttons::default();
        for ch in keys.chars() {
            match ch {
                'U' => b.u = true,
                'D' => b.d = true,
                'L' => b.l = true,
                'R' => b.r = true,
                'a' => b.a = true,
                'b' => b.b = true,
                'c' => b.c = true,
                'x' => b.x = true,
                'y' => b.y = true,
                'z' => b.z = true,
                's' => b.s = true,
                _ => {}
            }
        }
        out.extend(std::iter::repeat_n(b, n));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: statetrace <p1.def> <p2.def> <data_dir> [options]");
        std::process::exit(2);
    }
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let frames: u32 = opt("--frames").and_then(|v| v.parse().ok()).unwrap_or(600);
    let from: u32 = opt("--from").and_then(|v| v.parse().ok()).unwrap_or(0);
    let only: Option<usize> = opt("--only").and_then(|v| v.parse().ok());
    let power = args.iter().any(|a| a == "--power");
    let forces: Vec<(usize, i32, u32)> = args
        .windows(2)
        .filter(|w| w[0] == "--force")
        .filter_map(|w| {
            let (p, rest) = w[1].split_once(':')?;
            let (st, at) = rest.split_once('@')?;
            Some((p.parse().ok()?, st.parse().ok()?, at.parse().ok()?))
        })
        .collect();
    let keys = [
        opt("--keys1").map(|s| script(&s)).unwrap_or_default(),
        opt("--keys2").map(|s| script(&s)).unwrap_or_default(),
    ];
    let data_dir = PathBuf::from(&args[3]);
    let d1 = CharData::load_with_data(&PathBuf::from(&args[1]), &data_dir).expect("p1 loads");
    let d2 = CharData::load_with_data(&PathBuf::from(&args[2]), &data_dir).expect("p2 loads");
    let mut w = World::new(Stage::default(), 7);
    if let Some(s) = opt("--size") {
        use rugen_sim::scale::FighterSize as F;
        w.fighter_size = match s.as_str() {
            "none" => F::None,
            "chibi" => F::Chibi,
            "small" => F::Small,
            _ => F::Normal,
        };
    }
    w.add_player(d1, 0, 1);
    w.add_player(d2, 1, 2);
    w.start_round();
    for f in 0..frames {
        for &(p, st, at) in &forces {
            if at == w.frame
                && let Some(i) = w.side_leader(p.saturating_sub(1).min(1))
            {
                w.players[i].power = w.players[i].power_max;
                w.change_state(i, st, None, Some(false));
            }
        }
        if power {
            for p in w.players.iter_mut().filter(|p| p.helper_index == 0) {
                p.power = p.power_max;
            }
        }
        let b = |k: usize| keys[k].get(f as usize).copied().unwrap_or_default();
        w.step(&[b(0), b(1)]);
        if w.frame < from {
            continue;
        }
        for (i, p) in w.players.iter().enumerate() {
            if p.removed || only.is_some_and(|o| o != i) {
                continue;
            }
            println!(
                "f{} P{i} fac {} id {} par {} st {} prev {} t {} anim {} el {} at {} pos {:.1},{:.1} vel {:.2},{:.2} ctrl {} mt {:?} flags {:#x} trans {:?} pfx {:?}",
                w.frame,
                p.facing,
                p.id,
                p.parent_id,
                p.state_no,
                p.prev_state_no,
                p.state_time,
                p.anim.action,
                p.anim.elem,
                p.anim.elem_time,
                p.pos[0],
                p.pos[1],
                p.vel[0],
                p.vel[1],
                p.ctrl,
                p.move_type,
                p.assert_flags,
                p.trans,
                p.palfx.sample(),
            );
        }
    }
}
