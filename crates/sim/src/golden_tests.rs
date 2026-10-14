//! 1v1 matches must stay bit-identical across engine changes that do not
//! mean to change them (team modes, for one): recorded replays play back
//! the same and `rugen-server` re-simulates ranked matches. These checksums
//! were taken before team modes touched the simulation.

use std::path::{Path, PathBuf};

use crate::headless::{MatchSpec, build_world, replay};
use crate::world::Movement;

fn reference() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1")
}

/// Deterministic button noise: a held direction and a few presses, changing
/// every 6..20 ticks, for both players (4 bytes per frame, as replays).
fn noise(frames: u32, seed: u64) -> Vec<u8> {
    let mut s = seed;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut out = Vec::with_capacity(frames as usize * 4);
    let mut held = [0u16; 2];
    let mut left = [0u32; 2];
    for _ in 0..frames {
        for k in 0..2 {
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
            out.extend_from_slice(&held[k].to_le_bytes());
        }
    }
    out
}

struct Run<'a> {
    p1: &'a Path,
    p2: &'a Path,
    stage: Option<&'a Path>,
    movement: Movement,
    ai: [i32; 2],
    arena_ai: bool,
    frames: u32,
    seed: u64,
}

fn run(r: &Run) -> (u64, u32, [u8; 2]) {
    let root = reference();
    let spec = MatchSpec {
        p1: r.p1,
        p2: r.p2,
        stage: r.stage,
        data_dir: &root.join("data"),
        motif: &root.join("data/system.def"),
        zoom: crate::headless::ONLINE_ZOOM,
        movement: r.movement,
        round_time: None,
        ai: r.ai,
        fighter_size: Default::default(),
        seed: crate::headless::LEGACY_SEED,
    };
    let mut w = build_world(&spec).expect("world");
    w.arena_ai = r.arena_ai;
    let out = replay(&mut w, &noise(r.frames, r.seed), None);
    (out.checksum, out.frames, w.round.wins)
}

#[test]
fn kfm_matches_are_bit_identical() {
    let root = reference();
    let kfm = root.join("chars/kfm/kfm.def");
    let stage = root.join("stages/kfm.def");
    if !kfm.exists() || !stage.exists() {
        return;
    }
    let arena = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/arena/arena.def");
    let base = Run {
        p1: &kfm,
        p2: &kfm,
        stage: Some(&stage),
        movement: Movement::default(),
        ai: [0, 0],
        arena_ai: false,
        frames: 12000,
        seed: 0x5eed_1234,
    };
    let got = [
        run(&base),
        run(&Run {
            ai: [8, 3],
            frames: 20000,
            ..base
        }),
        run(&Run {
            movement: Movement {
                air_jumps: 1,
                air_dash: true,
                fast_fall: true,
            },
            seed: 77,
            ..base
        }),
        run(&Run {
            stage: Some(&arena),
            ai: [0, 6],
            arena_ai: true,
            frames: 4000,
            ..base
        }),
    ];
    for g in &got {
        eprintln!("({:#018x}, {}, {:?}),", g.0, g.1, g.2);
    }
    assert_eq!(got, GOLDEN_KFM);
}

const GOLDEN_KFM: [(u64, u32, [u8; 2]); 4] = [
    (0x4d93d67f889e5e19, 12000, [1, 0]),
    // the attacker's animation steps in the tick its hit starts a
    // hitpause (Ikemen acttmp), since the compat fix for Goku's endless
    // B air combo: AI matches with hits differ from then on
    (0x112ef8d316dc78d9, 20000, [2, 1]),
    (0xb4935f252c715b6e, 12000, [1, 0]),
    // examples/arena has items and hazards (moving / dropping platforms,
    // wind) since those features, on purpose
    (0xc540f0b530e524f5, 4000, [0, 0]),
];

/// The same with real community fighters (Naruto, Sasuke and Goku have
/// helpers, projectiles, supers, their own tag code). Not in the repo:
/// `RUGEN_GOLDEN_CHARS=<dir with G6_Naruto/ G6_Sasuke_Rinnegan/ Goku/>
/// cargo test -p rugen-sim golden -- --ignored`.
#[test]
#[ignore]
fn community_matches_are_bit_identical() {
    let Ok(dir) = std::env::var("RUGEN_GOLDEN_CHARS") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let naruto = dir.join("G6_Naruto/G6_Naruto.def");
    let sasuke = dir.join("G6_Sasuke_Rinnegan/G6_Sasuke_Rinnegan.def");
    let goku = dir.join("Goku/Goku.def");
    let stage = reference().join("stages/kfm.def");
    let base = Run {
        p1: &naruto,
        p2: &sasuke,
        stage: Some(&stage),
        movement: Movement::default(),
        ai: [0, 0],
        arena_ai: false,
        frames: 12000,
        seed: 0xabcdef,
    };
    let got = [
        run(&base),
        run(&Run {
            ai: [6, 6],
            frames: 20000,
            ..base
        }),
        run(&Run {
            p1: &goku,
            ai: [0, 8],
            seed: 3,
            ..base
        }),
    ];
    for g in &got {
        eprintln!("({:#018x}, {}, {:?}),", g.0, g.1, g.2);
    }
    assert_eq!(got, GOLDEN_COMMUNITY);
}

const GOLDEN_COMMUNITY: [(u64, u32, [u8; 2]); 3] = [
    // re-recorded with the hitpause animation step (see GOLDEN_KFM)
    (0xe86442fa2ca1cddd, 12000, [1, 0]),
    (0xd8a3ac02e4f7d455, 20000, [2, 0]),
    (0x03e8581d59d82711, 12000, [0, 1]),
];

/// The fighter debugger's trace records what the state machine did, never
/// changes it: a traced match ends bit-identical to an untraced one, and
/// the trace names fired controllers with the trigger group that held.
#[test]
fn tracing_never_changes_the_match() {
    use crate::world::CtrlOutcome;
    let root = reference();
    let kfm = root.join("chars/kfm/kfm.def");
    let stage = root.join("stages/kfm.def");
    if !kfm.exists() || !stage.exists() {
        return;
    }
    let spec = MatchSpec {
        p1: &kfm,
        p2: &kfm,
        stage: Some(&stage),
        data_dir: &root.join("data"),
        motif: &root.join("data/system.def"),
        zoom: crate::headless::ONLINE_ZOOM,
        movement: Movement::default(),
        round_time: None,
        ai: [0, 4],
        fighter_size: Default::default(),
        seed: crate::headless::LEGACY_SEED,
    };
    let inputs = noise(1500, 9);
    let plain = {
        let mut w = build_world(&spec).expect("world");
        replay(&mut w, &inputs, None).checksum
    };
    let mut w = build_world(&spec).expect("world");
    w.trace = Some(Vec::new());
    let (mut fired, mut failed) = (0, 0);
    for f in inputs.chunks_exact(4) {
        let b = |k: usize| crate::Buttons::from_bits(u16::from_le_bytes([f[k], f[k + 1]]));
        w.step(&[b(0), b(2)]);
        for t in w.trace.as_ref().unwrap() {
            let state = &w.data[t.data].states[&t.state];
            assert!((t.index as usize) < state.ctrls.len());
            match t.outcome {
                CtrlOutcome::Fired { group } => {
                    fired += 1;
                    assert!((group as usize) < state.ctrls[t.index as usize].triggers.len());
                }
                _ => failed += 1,
            }
        }
    }
    assert!(
        fired > 100 && failed > 100,
        "fired {fired}, failed {failed}"
    );
    let traced = replay(&mut w, &[], None).checksum;
    assert_eq!(traced, plain);
}
