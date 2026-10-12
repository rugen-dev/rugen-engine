//! Safety limits and crash isolation: one character's content must never
//! take the simulation down. Skipped when the reference tree is absent.

use std::path::PathBuf;
use std::sync::Arc;

use crate::world::{Event, Stage, World};
use crate::{Buttons, CharData, Rng};

fn kfm() -> Option<Arc<CharData>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/mugen-1.1b1/chars/kfm/kfm.def");
    if !p.exists() {
        return None;
    }
    Some(CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}")))
}

fn world() -> Option<World> {
    let d = kfm()?;
    let mut w = World::new(Stage::default(), 1);
    w.add_player(d.clone(), 0, 1);
    w.add_player(d, 1, 2);
    w.start_round();
    // skip the intro so states run with control
    let mut guard = 0;
    while w.round.round_state() != 2 && guard < 2000 {
        w.step(&[Buttons::default(), Buttons::default()]);
        guard += 1;
    }
    Some(w)
}

fn run(w: &mut World, n: usize) {
    for _ in 0..n {
        w.step(&[Buttons::default(), Buttons::default()]);
    }
}

fn state_text(no: i32, body: &str) -> String {
    format!("[Statedef {no}]\ntype = S\nmovetype = I\nphysics = S\nanim = 0\nctrl = 1\n{body}\n")
}

#[test]
fn explod_and_projectile_limits_hold() {
    let Some(mut w) = world() else { return };
    let text = state_text(
        7000,
        "[State 7000, e]\ntype = Explod\ntrigger1 = 1\nanim = 0\n\n[State 7000, p]\ntype = Projectile\ntrigger1 = 1\nprojanim = 0\nattr = S, NP\n",
    );
    let d = w.data[0].with_states("limits", &text).unwrap();
    w.data[0] = d;
    w.limits.explods = 20;
    w.limits.projectiles = 5;
    w.change_state(0, 7000, None, None);
    run(&mut w, 60);
    let explods = w.explods.iter().filter(|e| !e.removed).count();
    let projs = w.projs.iter().filter(|p| !p.removed).count();
    assert!(explods <= 20, "explods {explods}");
    assert!(projs <= 5, "projectiles {projs}");
    assert!(w.warnings.iter().any(|m| m.contains("explod limit")));
    assert!(w.warnings.iter().any(|m| m.contains("projectile limit")));
}

#[test]
fn helper_limit_is_per_root() {
    let Some(mut w) = world() else { return };
    let text = state_text(
        7001,
        "[State 7001, h]\ntype = Helper\ntrigger1 = 1\nstateno = 0\n",
    );
    let d = w.data[0].with_states("limits", &text).unwrap();
    w.data[0] = d.clone();
    w.data[1] = d;
    w.limits.helpers = 4;
    w.change_state(0, 7001, None, None);
    run(&mut w, 10);
    let mine = w
        .players
        .iter()
        .filter(|p| !p.removed && p.helper_index > 0 && p.root == 0)
        .count();
    assert_eq!(mine, 4);
    assert!(w.warnings.iter().any(|m| m.contains("helper limit")));
}

#[test]
fn sound_flood_is_capped() {
    let Some(mut w) = world() else { return };
    let mut body = String::new();
    for k in 0..60 {
        body.push_str(&format!(
            "[State 7002, s{k}]\ntype = PlaySnd\ntrigger1 = 1\nvalue = 0, 0\n\n"
        ));
    }
    let text = state_text(7002, &body);
    let d = w.data[0].with_states("limits", &text).unwrap();
    w.data[0] = d;
    w.change_state(0, 7002, None, None);
    run(&mut w, 1);
    let sounds = w
        .events
        .iter()
        .filter(|e| matches!(e, Event::Sound { .. }))
        .count();
    assert!(
        sounds <= w.limits.sounds_per_tick,
        "{sounds} sounds in one tick"
    );
}

#[test]
fn non_finite_positions_are_sanitized() {
    let Some(mut w) = world() else { return };
    // 1e30 * 1e30 overflows f32 to infinity; inf - inf is NaN
    let text = state_text(
        7003,
        "[State 7003, v]\ntype = VelSet\ntrigger1 = 1\nx = 1e30 * 1e30\ny = 1e30 * 1e30 - 1e30 * 1e30\n",
    );
    let d = w.data[0].with_states("limits", &text).unwrap();
    w.data[0] = d;
    w.change_state(0, 7003, None, None);
    run(&mut w, 5);
    let p = &w.players[0];
    assert!(p.pos[0].is_finite() && p.pos[1].is_finite());
    assert!(p.vel[0].is_finite() && p.vel[1].is_finite());
    assert!(w.warnings.iter().any(|m| m.contains("non-finite")));
}

#[test]
fn step_checked_disables_a_crashing_player() {
    let Some(mut w) = world() else { return };
    run(&mut w, 5);
    let (frame, p1_pos) = (w.frame, w.players[0].pos);
    w.panic_on_run = Some(1);
    let blamed = w.step_checked(&[Buttons::default(), Buttons::default()]);
    assert_eq!(blamed, Some(1));
    assert_eq!(w.frame, frame, "world rolled back to the snapshot");
    assert_eq!(w.players[0].pos, p1_pos);
    assert!(w.players[1].broken);
    assert_eq!(w.players[1].life, 0);
    w.panic_on_run = None;
    // the match carries on: P2 is frozen, P1 keeps running
    for _ in 0..30 {
        assert!(
            w.step_checked(&[Buttons::default(), Buttons::default()])
                .is_none()
        );
    }
    assert!(w.players[1].broken);
    assert!(w.warnings.iter().any(|m| m.contains("crashed")));
}

/// Random inputs plus randomly generated (mostly nonsensical) state
/// controllers must never panic the simulation.
#[test]
fn fuzzed_controllers_never_panic() {
    let Some(base) = kfm() else { return };
    let kinds = [
        "ChangeState",
        "SelfState",
        "VelSet",
        "VelAdd",
        "VelMul",
        "PosSet",
        "PosAdd",
        "VarSet",
        "VarAdd",
        "LifeAdd",
        "LifeSet",
        "PowerAdd",
        "HitDef",
        "Explod",
        "Helper",
        "Projectile",
        "PlaySnd",
        "ChangeAnim",
        "ChangeAnim2",
        "AfterImage",
        "PalFX",
        "Pause",
        "SuperPause",
        "TargetState",
        "TargetLifeAdd",
        "TargetBind",
        "DestroySelf",
        "Width",
        "AssertSpecial",
        "Turn",
        "CtrlSet",
        "StateTypeSet",
        "HitBy",
        "NotHitBy",
        "HitOverride",
        "ReversalDef",
        "Gravity",
        "ScreenBound",
        "AngleDraw",
        "EnvShake",
        "BindToParent",
        "ParentVarSet",
        "VarRangeSet",
        "AttackMulSet",
        "DefenceMulSet",
        "HitVelSet",
        "HitFallDamage",
        "PosFreeze",
        "RemoveExplod",
        "ModifyExplod",
        "SprPriority",
        "MakeDust",
        "Offset",
    ];
    let params = [
        "value", "x", "y", "v", "fv", "anim", "stateno", "id", "time", "pos", "damage", "attr",
        "flags", "elem", "first", "last", "postype", "movetime",
    ];
    for seed in 1..=4u64 {
        let mut rng = Rng::new(seed * 7919);
        let mut num = |rng: &mut Rng| -> String {
            match rng.below(6) {
                0 => "0".into(),
                1 => "-1".into(),
                2 => format!("{}", rng.below(100) as i32 - 50),
                3 => format!("{}", rng.below(1_000_000) as i32),
                4 => "1e30 * 1e30".into(),
                _ => format!("{}.5", rng.below(1000) as i32 - 500),
            }
        };
        let mut text = String::new();
        for k in 0..40 {
            let no = 8000 + k;
            let st = ["S", "C", "A", "L"][rng.below(4) as usize];
            let anim = ["0", "200", "5000", "999999", "-5", "41"][rng.below(6) as usize];
            let phys = ["S", "C", "A", "N"][rng.below(4) as usize];
            text.push_str(&format!(
                "[Statedef {no}]\ntype = {st}\nmovetype = A\nphysics = {phys}\nanim = {anim}\nctrl = {}\n\n",
                rng.below(2)
            ));
            for c in 0..8 {
                let kind = kinds[rng.below(kinds.len() as u32) as usize];
                let trig = [
                    "1",
                    "random < 500",
                    "time = 0",
                    "animelem = 2",
                    "vel y > 0",
                    "var(0) > 3",
                ][rng.below(6) as usize];
                text.push_str(&format!(
                    "[State {no}, {c}]\ntype = {kind}\ntrigger1 = {trig}\n"
                ));
                for _ in 0..3 {
                    let pn = params[rng.below(params.len() as u32) as usize];
                    let v = match pn {
                        "attr" => "SCA, NA, SP".to_string(),
                        "flags" => "noautoturn, intro, invisible".to_string(),
                        "postype" => "p2".to_string(),
                        "pos" => format!("{}, {}", num(&mut rng), num(&mut rng)),
                        _ => num(&mut rng),
                    };
                    text.push_str(&format!("{pn} = {v}\n"));
                }
                text.push('\n');
            }
        }
        let d = base.with_states_lenient("fuzz", &text);
        let mut w = World::new(Stage::default(), seed);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        w.start_round();
        for t in 0..600u32 {
            let a = Buttons::from_bits((rng.next_u64() & 0x7ff) as u16);
            let b = Buttons::from_bits((rng.next_u64() & 0x7ff) as u16);
            if t % 25 == 0 {
                for i in 0..2 {
                    if !w.players[i].removed {
                        let no = 8000 + rng.below(40) as i32;
                        w.change_state(i, no, None, None);
                    }
                }
            }
            w.step(&[a, b]);
            assert!(w.players.len() < 400, "player list exploded");
        }
        for p in &w.players {
            assert!(p.pos[0].is_finite() && p.pos[1].is_finite(), "seed {seed}");
        }
    }
}
