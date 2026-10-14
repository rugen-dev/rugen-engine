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
    (0x06349344b07c143a, 20000, [2, 1]),
    (0xeccb0277ef3cb214, 12000, [1, 0]),
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
    (0x6766b21d8016fc94, 12000, [1, 0]),
    (0x648e097b8dc77328, 20000, [1, 2]),
    (0x72e835bd3538dacd, 12000, [0, 2]),
];
