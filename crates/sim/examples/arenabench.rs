//! CPU vs CPU matches on an arena stage, with ring-out statistics.
//!
//! `cargo run --release -p rugen-sim --example arenabench -- <p1.def> <p2.def>
//!  <stage.def> <data_dir> <motif> [--matches N] [--ai L] [--seed S]
//!  [--arena-ai 0|1] [--trace] [--dump P,F0,F1,M]`
//!
//! Plays N full matches (seeds S, S+1, ...) and prints, per side: stocks lost
//! by ring-out, self-destructs (ring-outs with no hit taken in the last 2 s),
//! KOs by damage, average stock duration and match wins. `--trace` prints
//! the last states before every ring-out; `--dump` prints player P tick by
//! tick from frame F0 to F1 of match M.
//! Character defs may live inside an `.rgc` (`chars/x.rgc/x.def`).

use std::path::PathBuf;
use std::sync::Arc;

use rugen_cns::state::MoveType;
use rugen_sim::world::Movement;
use rugen_sim::{CharData, FxData, Stage, World};

#[derive(Default, Clone, Copy)]
struct Side {
    ringouts: u32,
    selfko: u32,
    dmg_ko: u32,
    stock_frames: u64,
    stocks_ended: u32,
    wins: u32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pos: Vec<&String> = {
        let mut v = Vec::new();
        let mut k = 1;
        while k < args.len() {
            if args[k].starts_with("--") {
                k += 2;
                continue;
            }
            v.push(&args[k]);
            k += 1;
        }
        v
    };
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|k| args.get(k + 1))
            .cloned()
    };
    if pos.len() < 5 {
        eprintln!(
            "usage: arenabench <p1.def> <p2.def> <stage.def> <data_dir> <motif> [--matches N] [--ai L] [--seed S] [--arena-ai 0|1]"
        );
        std::process::exit(2);
    }
    let (p1, p2, stage, data_dir, motif) = (
        PathBuf::from(pos[0]),
        PathBuf::from(pos[1]),
        PathBuf::from(pos[2]),
        PathBuf::from(pos[3]),
        PathBuf::from(pos[4]),
    );
    let matches: u64 = opt("--matches").and_then(|v| v.parse().ok()).unwrap_or(4);
    let ai: i32 = opt("--ai").and_then(|v| v.parse().ok()).unwrap_or(4);
    let seed0: u64 = opt("--seed").and_then(|v| v.parse().ok()).unwrap_or(1);
    let trace = args.iter().any(|a| a == "--trace");
    // --dump P,F0,F1,M: per-tick state of player P in match M
    let dump: Option<(usize, u32, u32, u64)> = opt("--dump").and_then(|v| {
        let n: Vec<u64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        (n.len() == 4).then(|| (n[0] as usize, n[1] as u32, n[2] as u32, n[3]))
    });
    let arena_ai = opt("--arena-ai").is_none_or(|v| v != "0");
    let d1 = CharData::load_with_data(&p1, &data_dir).expect("p1");
    let d2 = CharData::load_with_data(&p2, &data_dir).expect("p2");
    let fx = FxData::load_with_motif(&motif, &data_dir).ok();
    let def = rugen_formats::def::Def::open(&stage).expect("stage");
    let stage = Stage::from_def(&def);
    let names = [d1.name.clone(), d2.name.clone()];

    let mut sides = [Side::default(); 2];
    let mut draws = 0;
    let max_frames = 60 * 60 * 8;
    for m in 0..matches {
        let mut w = World::new(stage.clone(), seed0 + m);
        if let Some(fx) = &fx {
            let mut round = fx.round.clone();
            round.round_time = 99;
            w.round = rugen_sim::RoundState::new(round);
            w.fx = Some(fx.clone());
        }
        w.add_player(Arc::clone(&d1), 0, 1);
        w.add_player(Arc::clone(&d2), 1, 2);
        let a = &w.stage.arena;
        w.movement = Movement {
            air_jumps: a.air_jumps.unwrap_or(0),
            air_dash: a.air_dash.unwrap_or(false),
            fast_fall: a.fast_fall.unwrap_or(false),
        };
        w.arena_ai = arena_ai;
        w.players[0].ai_level = ai;
        w.players[1].ai_level = ai;
        w.start_round();
        let mut last_hit = [0u32; 2];
        let mut stock_start = [0u32; 2];
        let mut prev_stocks = [w.players[0].stocks, w.players[1].stocks];
        let mut prev_alive = [true; 2];
        let mut prev_round = w.round.round_no;
        let mut prev_life = [w.players[0].life, w.players[1].life];
        let mut hist: [Vec<(i32, i32, i32, bool)>; 2] = [Vec::new(), Vec::new()];
        while !w.round.match_over && w.frame < max_frames {
            w.step(&[]);
            let f = w.frame;
            if let Some((dp, f0, f1, dm)) = dump
                && m == dm
                && f >= f0
                && f <= f1
            {
                let p = &w.players[dp];
                let sc = w.local_scale(dp);
                let b = p.arena.last_buttons();
                eprintln!(
                    "  f{f} st{} t{} ctrl{} x{:.1} y{:.1} vx{:.2} vy{:.2} face{} btn {}{}{}{} ab{}{}",
                    p.state_no,
                    p.state_time,
                    p.ctrl as u8,
                    p.pos[0] * sc,
                    p.pos[1] * sc,
                    p.vel[0] * p.facing * sc,
                    p.vel[1] * sc,
                    p.facing,
                    if b.u { "U" } else { "" },
                    if b.d { "D" } else { "" },
                    if b.l { "L" } else { "" },
                    if b.r { "R" } else { "" },
                    b.a as u8,
                    b.x as u8
                );
            }
            if w.round.round_no != prev_round {
                prev_round = w.round.round_no;
                stock_start = [f; 2];
                last_hit = [0; 2];
                prev_alive = [true; 2];
                for k in 0..2 {
                    prev_stocks[k] = w.players[k].stocks;
                    prev_life[k] = w.players[k].life;
                }
                continue;
            }
            for k in 0..2 {
                let p = &w.players[k];
                let sc = w.local_scale(k);
                let e = (
                    p.state_no,
                    (p.pos[0] * sc) as i32,
                    (p.pos[1] * sc) as i32,
                    p.ctrl,
                );
                if hist[k].last().is_none_or(|l| l.0 != e.0) {
                    hist[k].push(e);
                    if hist[k].len() > 12 {
                        hist[k].remove(0);
                    }
                }
                if p.move_type == MoveType::H || p.life < prev_life[k] {
                    last_hit[k] = f;
                }
                let lost_stock = p.stocks < prev_stocks[k];
                let died = prev_alive[k] && !p.alive();
                if lost_stock || died {
                    // ring-out: the blast zone takes a stock (or the last one)
                    let ringout = lost_stock;
                    let s = &mut sides[k];
                    if ringout && trace {
                        let sc = w.local_scale(k);
                        eprintln!(
                            "  ringout {} f{} hit{} at ({:.0},{:.0}) states {:?} self={}",
                            names[k],
                            f,
                            f.saturating_sub(last_hit[k]),
                            p.pos[0] * sc,
                            p.pos[1] * sc,
                            hist[k],
                            f.saturating_sub(last_hit[k]) > 120 || last_hit[k] == 0
                        );
                    }
                    if ringout {
                        s.ringouts += 1;
                        if f.saturating_sub(last_hit[k]) > 120 || last_hit[k] == 0 {
                            s.selfko += 1;
                        }
                    } else {
                        s.dmg_ko += 1;
                    }
                    s.stock_frames += u64::from(f - stock_start[k]);
                    s.stocks_ended += 1;
                    stock_start[k] = f;
                }
                prev_stocks[k] = p.stocks;
                prev_alive[k] = p.alive();
                prev_life[k] = p.life;
            }
        }
        let wn = w.round.wins;
        if w.round.match_over && wn[0] != wn[1] {
            sides[usize::from(wn[1] > wn[0])].wins += 1;
        } else {
            draws += 1;
        }
        eprintln!(
            "match {m}: wins {:?}, frames {}, over {}, vetoes {:?} cancels {:?} recoveries {:?} navs {:?} ledges {:?}",
            wn,
            w.frame,
            w.round.match_over,
            [w.players[0].arena.vetoes, w.players[1].arena.vetoes],
            [w.players[0].arena.cancels, w.players[1].arena.cancels],
            [w.players[0].arena.recoveries, w.players[1].arena.recoveries],
            [w.players[0].arena.navs, w.players[1].arena.navs],
            [w.players[0].arena.ledges, w.players[1].arena.ledges],
        );
    }
    println!(
        "{} vs {} | ai {ai} | arena_ai {arena_ai} | {matches} matches, {draws} draws",
        names[0], names[1]
    );
    for k in 0..2 {
        let s = sides[k];
        let avg = if s.stocks_ended > 0 {
            s.stock_frames as f64 / s.stocks_ended as f64 / 60.0
        } else {
            0.0
        };
        println!(
            "  P{} {:<20} ringouts {:>3} selfKO {:>3} dmgKO {:>3} avg stock {:>6.1}s wins {}",
            k + 1,
            names[k],
            s.ringouts,
            s.selfko,
            s.dmg_ko,
            avg,
            s.wins
        );
    }
}
