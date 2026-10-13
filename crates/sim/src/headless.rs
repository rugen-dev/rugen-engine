//! One place that builds the [`World`] of a match from files, used by the
//! game and by `rugen-server`'s replay verifier (`--features resim`), so
//! both simulate exactly the same thing.

use std::path::Path;

use crate::world::Movement;
use crate::{CharData, FxData, Stage, World};

/// Everything that shapes a match's simulation.
#[derive(Clone, Debug)]
pub struct MatchSpec<'a> {
    pub p1: &'a Path,
    pub p2: &'a Path,
    pub stage: Option<&'a Path>,
    /// Engine `data/` (common states fallback, fightfx, fight.def).
    pub data_dir: &'a Path,
    pub motif: &'a Path,
    /// Camera limits (`[camera] zoom_out / zoom_in`); `zoom_in` replaces the stage's.
    pub zoom: (f32, f32),
    /// Engine movement options; a stage's `[Arena]` overrides them.
    pub movement: Movement,
    /// Round time override (offline only; online peers keep the motif's).
    pub round_time: Option<i32>,
    pub ai: [i32; 2],
    /// FIGHTER SIZE (a match rule: replays record it, online peers share it).
    pub fighter_size: crate::scale::FighterSize,
}

/// Build the world and start the first round.
pub fn build_world(spec: &MatchSpec) -> crate::Result<World> {
    let mut stage = match spec.stage {
        Some(p) => Stage::from_def(&rugen_formats::def::Def::open(p)?),
        None => Stage::default(),
    };
    stage.zoom_out = stage.zoom_out.min(spec.zoom.0.clamp(0.2, 1.0));
    // the player's zoom-in setting caps every stage (below 1 keeps the view
    // wider than the stage's own screen even when the fighters stand close)
    stage.zoom_in = spec.zoom.1.clamp(stage.zoom_out, 3.0);
    let d1 = CharData::load_with_data(spec.p1, spec.data_dir)?;
    let d2 = if spec.p2 == spec.p1 {
        d1.clone()
    } else {
        CharData::load_with_data(spec.p2, spec.data_dir)?
    };
    let mut world = World::new(stage, 1);
    world.fighter_size = spec.fighter_size;
    match FxData::load_with_motif(spec.motif, spec.data_dir) {
        Ok(fx) => {
            let mut round = fx.round.clone();
            if let Some(t) = spec.round_time {
                round.round_time = t;
            }
            world.round = crate::RoundState::new(round);
            world.fx = Some(fx);
        }
        Err(e) => log::warn!("fightfx not loaded from {}: {e}", spec.data_dir.display()),
    }
    world.add_player(d1, 0, 1);
    world.add_player(d2, 1, 2);
    let a = &world.stage.arena;
    world.movement = Movement {
        air_jumps: a.air_jumps.unwrap_or(spec.movement.air_jumps).clamp(0, 8),
        air_dash: a.air_dash.unwrap_or(spec.movement.air_dash),
        fast_fall: a.fast_fall.unwrap_or(spec.movement.fast_fall),
    };
    world.players[0].ai_level = spec.ai[0].clamp(0, 8);
    world.players[1].ai_level = spec.ai[1].clamp(0, 8);
    world.start_round();
    Ok(world)
}

/// Outcome of replaying a confirmed input stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    pub frames: u32,
    pub checksum: u64,
    /// Winning side (0 / 1) once the match is over; `None` for a draw or unfinished.
    pub winner: Option<usize>,
    pub match_over: bool,
}

/// Step `world` through `inputs` (4 bytes per frame: P1 u16 LE, P2 u16 LE),
/// stopping at `stop_frame` if given.
pub fn replay(world: &mut World, inputs: &[u8], stop_frame: Option<u32>) -> Replayed {
    for f in inputs.chunks_exact(4) {
        if stop_frame.is_some_and(|s| world.frame >= s) {
            break;
        }
        let p1 = u16::from_le_bytes([f[0], f[1]]);
        let p2 = u16::from_le_bytes([f[2], f[3]]);
        world.step(&[crate::Buttons::from_bits(p1), crate::Buttons::from_bits(p2)]);
    }
    let w = world.round.wins;
    Replayed {
        frames: world.frame,
        checksum: world.checksum(),
        winner: (world.round.match_over && w[0] != w[1]).then(|| usize::from(w[1] > w[0])),
        match_over: world.round.match_over,
    }
}
