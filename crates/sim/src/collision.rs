//! Per-tick collision: player push, then Clsn1-vs-Clsn2 hit detection with
//! MUGEN's hit result rules (guard, hit pause, get-hit variables, targets).

use rugen_cns::state::{MoveType, StateType};

use crate::hit::{
    self, HitDef, HitDefSpec, HitType, MoveContact, Reaction, Trade, attr, boxes_overlap, hf,
};
use crate::player::Player;
use crate::world::{Event, World};

/// A HitDef's `palfx.*` parameters (MUGEN: applied to the target on a hit),
/// `None` without a positive `palfx.time`.
fn hit_palfx(f: &dyn Fn(&str, usize) -> Option<f32>) -> Option<crate::fx::PalFx> {
    let time = f("palfx.time", 0)? as i32;
    if time <= 0 {
        return None;
    }
    let mut pf = crate::fx::PalFx {
        time,
        ..Default::default()
    };
    let rgb = |key: &str, def: i32| [0, 1, 2].map(|k| f(key, k).map_or(def, |v| v as i32));
    pf.add = rgb("palfx.add", 0);
    pf.mul = rgb("palfx.mul", 256);
    if f("palfx.sinadd", 0).is_some() {
        let period = f("palfx.sinadd", 3).map_or(0, |v| v as i32);
        let side = if period < 0 { -1 } else { 1 };
        pf.cycletime[0] = period.abs();
        pf.sinadd = rgb("palfx.sinadd", 0).map(|v| v * side);
    }
    if let Some(c) = f("palfx.color", 0) {
        pf.color = c / 256.0;
    }
    pf.invertall = f("palfx.invertall", 0).is_some_and(|v| v != 0.0);
    Some(pf)
}

/// Evaluate a HitDef controller into concrete values with MUGEN defaults.
pub fn eval_hitdef(w: &mut World, i: usize, spec: &HitDefSpec) -> HitDef {
    let data = w.data[w.players[i].data].clone();
    let ls = data.localcoord[0] / 320.0;
    // evaluate every numeric parameter up front (left to right, like MUGEN)
    let mut vals: Vec<(String, Vec<f32>)> = Vec::with_capacity(spec.exprs.len());
    for (k, (_, codes)) in &spec.exprs {
        let v: Vec<f32> = codes.iter().map(|c| w.eval(i, c).to_f()).collect();
        vals.push((k.clone(), v));
    }
    let f = |key: &str, idx: usize| -> Option<f32> {
        vals.iter().find(|(k, _)| k == key)?.1.get(idx).copied()
    };
    let mut hd = HitDef {
        attr: spec.attr,
        reversal_attr: spec.reversal_attr,
        hitflag: spec.hitflag,
        guardflag: spec.guardflag,
        affectteam: spec.affectteam,
        animtype: spec.animtype,
        air_animtype: spec.air_animtype.unwrap_or(spec.animtype),
        fall_animtype: Reaction::Back,
        ground_type: spec.ground_type,
        air_type: spec.air_type.unwrap_or(spec.ground_type),
        priority: f("priority", 0).map_or(4, |v| v as i32),
        trade: spec.trade,
        damage: f("damage", 0).map_or(0, |v| v as i32),
        guard_damage: f("damage", 1).map_or(0, |v| v as i32),
        pausetime: [
            f("pausetime", 0).map_or(0, |v| v as i32),
            f("pausetime", 1).map_or(0, |v| v as i32),
        ],
        guard_pausetime: [0, 0],
        sparkno: f("sparkno", 0).map_or(data.konst("data.sparkno").unwrap_or(2.0) as i32, |v| {
            v as i32
        }),
        sparkno_own: spec
            .exprs
            .get("sparkno")
            .is_some_and(|(p, _)| *p == Some('s')),
        guard_sparkno: f("guard.sparkno", 0).map_or(
            data.konst("data.guard.sparkno").unwrap_or(40.0) as i32,
            |v| v as i32,
        ),
        guard_sparkno_own: spec
            .exprs
            .get("guard.sparkno")
            .is_some_and(|(p, _)| *p == Some('s')),
        sparkxy: [
            f("sparkxy", 0).unwrap_or(0.0),
            f("sparkxy", 1).unwrap_or(0.0),
        ],
        hitsound: [
            f("hitsound", 0).map_or(-1, |v| v as i32),
            f("hitsound", 1).map_or(0, |v| v as i32),
        ],
        hitsound_own: spec
            .exprs
            .get("hitsound")
            .is_some_and(|(p, _)| *p == Some('s')),
        guardsound: [
            f("guardsound", 0).map_or(-1, |v| v as i32),
            f("guardsound", 1).map_or(0, |v| v as i32),
        ],
        guardsound_own: spec
            .exprs
            .get("guardsound")
            .is_some_and(|(p, _)| *p == Some('s')),
        ground_slidetime: f("ground.slidetime", 0).map_or(0, |v| v as i32),
        guard_slidetime: 0,
        ground_hittime: f("ground.hittime", 0).map_or(0, |v| v as i32),
        guard_hittime: 0,
        air_hittime: f("air.hittime", 0).map_or(20, |v| v as i32),
        down_hittime: f("down.hittime", 0).map_or(20, |v| v as i32),
        guard_ctrltime: 0,
        airguard_ctrltime: 0,
        guard_dist: f("guard.dist", 0).unwrap_or(data.konst("size.attack.dist").unwrap_or(160.0)),
        yaccel: f("yaccel", 0).unwrap_or(0.35 * ls),
        ground_velocity: [
            f("ground.velocity", 0).unwrap_or(0.0),
            f("ground.velocity", 1).unwrap_or(0.0),
        ],
        guard_velocity: 0.0,
        air_velocity: [
            f("air.velocity", 0).unwrap_or(0.0),
            f("air.velocity", 1).unwrap_or(0.0),
        ],
        airguard_velocity: [0.0, 0.0],
        ground_cornerpush_veloff: 0.0,
        air_cornerpush_veloff: 0.0,
        down_cornerpush_veloff: 0.0,
        guard_cornerpush_veloff: 0.0,
        airguard_cornerpush_veloff: 0.0,
        air_juggle: f("air.juggle", 0).map_or(0, |v| v as i32),
        mindist: [f("mindist", 0), f("mindist", 1)],
        maxdist: [f("maxdist", 0), f("maxdist", 1)],
        p1sprpriority: f("p1sprpriority", 0).map_or(1, |v| v as i32),
        p2sprpriority: f("p2sprpriority", 0).map_or(0, |v| v as i32),
        p1facing: f("p1facing", 0).map_or(0, |v| v as i32),
        p1getp2facing: f("p1getp2facing", 0).map_or(0, |v| v as i32),
        p2facing: f("p2facing", 0).map_or(0, |v| v as i32),
        p1stateno: f("p1stateno", 0).map_or(-1, |v| v as i32),
        p2stateno: f("p2stateno", 0).map_or(-1, |v| v as i32),
        p2getp1state: true,
        forcestand: false,
        ground_fall: f("fall", 0).is_some_and(|v| v != 0.0),
        air_fall: false,
        forcenofall: f("forcenofall", 0).is_some_and(|v| v != 0.0),
        fall_xvelocity: f("fall.xvelocity", 0),
        fall_yvelocity: f("fall.yvelocity", 0).unwrap_or(-4.5 * ls),
        fall_recover: f("fall.recover", 0).is_none_or(|v| v != 0.0),
        fall_recovertime: f("fall.recovertime", 0).map_or(4, |v| v as i32),
        fall_damage: f("fall.damage", 0).map_or(0, |v| v as i32),
        down_velocity: [0.0, 0.0],
        down_bounce: f("down.bounce", 0).is_some_and(|v| v != 0.0),
        id: f("id", 0).map_or(0, |v| (v as i32).max(0)),
        chainid: f("chainid", 0).map_or(-1, |v| v as i32),
        nochainid: [
            f("nochainid", 0).map_or(-1, |v| v as i32),
            f("nochainid", 1).map_or(-1, |v| v as i32),
        ],
        hitonce: f("hitonce", 0).map_or(-1, |v| (v != 0.0) as i32),
        kill: f("kill", 0).is_none_or(|v| v != 0.0),
        guard_kill: f("guard.kill", 0).is_none_or(|v| v != 0.0),
        fall_kill: f("fall.kill", 0).is_none_or(|v| v != 0.0),
        numhits: f("numhits", 0).map_or(1, |v| v as i32),
        hit_getpower: i32::MIN,
        guard_getpower: i32::MIN,
        hit_givepower: i32::MIN,
        guard_givepower: i32::MIN,
        envshake_time: f("envshake.time", 0).map_or(0, |v| v as i32),
        envshake_freq: f("envshake.freq", 0).unwrap_or(60.0),
        envshake_ampl: f("envshake.ampl", 0).map_or(-4, |v| v as i32),
        envshake_phase: f("envshake.phase", 0),
        fall_envshake_time: f("fall.envshake.time", 0).map_or(0, |v| v as i32),
        fall_envshake_freq: f("fall.envshake.freq", 0).unwrap_or(60.0),
        fall_envshake_ampl: f("fall.envshake.ampl", 0).map_or(-4, |v| v as i32),
        fall_envshake_phase: f("fall.envshake.phase", 0),
        palfx: hit_palfx(&f),
        active: true,
        owner_id: w.players[i].id,
    };
    // snap: both min and max
    if let Some(x) = f("snap", 0) {
        hd.mindist[0] = Some(x);
        hd.maxdist[0] = Some(x);
    }
    if let Some(y) = f("snap", 1) {
        hd.mindist[1] = Some(y);
        hd.maxdist[1] = Some(y);
    }
    if let Some(v) = f("p2getp1state", 0) {
        hd.p2getp1state = v != 0.0;
    }
    // defaults that depend on other params (finalizeParams)
    if hd.attr & !attr::ST_MASK == 0 {
        hd.attr = 0;
    }
    if hd.hitonce < 0 {
        hd.hitonce = (hd.attr & attr::AT_AT != 0) as i32;
    }
    hd.guard_pausetime = [
        f("guard.pausetime", 0).map_or(hd.pausetime[0], |v| v as i32),
        f("guard.pausetime", 1).map_or(hd.pausetime[1], |v| v as i32),
    ];
    hd.guard_hittime = f("guard.hittime", 0).map_or(hd.ground_slidetime, |v| v as i32);
    hd.guard_slidetime = f("guard.slidetime", 0).map_or(hd.guard_hittime, |v| v as i32);
    hd.guard_ctrltime = f("guard.ctrltime", 0).map_or(hd.guard_slidetime, |v| v as i32);
    hd.airguard_ctrltime = f("airguard.ctrltime", 0).map_or(hd.guard_ctrltime, |v| v as i32);
    hd.guard_velocity = f("guard.velocity", 0).unwrap_or(hd.ground_velocity[0]);
    hd.airguard_velocity = [
        f("airguard.velocity", 0).unwrap_or(hd.air_velocity[0] * 1.5),
        f("airguard.velocity", 1).unwrap_or(hd.air_velocity[1] * 0.5),
    ];
    hd.down_velocity = [
        f("down.velocity", 0).unwrap_or(hd.air_velocity[0]),
        f("down.velocity", 1).unwrap_or(hd.air_velocity[1]),
    ];
    hd.fall_animtype =
        spec.fall_animtype
            .unwrap_or(if hd.air_animtype as i32 >= Reaction::Up as i32 {
                hd.air_animtype
            } else {
                Reaction::Back
            });
    hd.forcestand = f("forcestand", 0).map_or(hd.ground_velocity[1] != 0.0, |v| v != 0.0);
    hd.air_fall = f("air.fall", 0).map_or(hd.ground_fall, |v| v != 0.0);
    // legacy corner push defaults (MUGEN chars)
    hd.ground_cornerpush_veloff =
        f("ground.cornerpush.veloff", 0).unwrap_or(if hd.attr & attr::ST_A != 0 {
            0.0
        } else {
            hd.guard_velocity * 1.3
        });
    hd.air_cornerpush_veloff = f("air.cornerpush.veloff", 0).unwrap_or(hd.ground_cornerpush_veloff);
    hd.down_cornerpush_veloff =
        f("down.cornerpush.veloff", 0).unwrap_or(hd.ground_cornerpush_veloff);
    hd.guard_cornerpush_veloff =
        f("guard.cornerpush.veloff", 0).unwrap_or(hd.ground_cornerpush_veloff);
    hd.airguard_cornerpush_veloff =
        f("airguard.cornerpush.veloff", 0).unwrap_or(hd.guard_cornerpush_veloff);
    let (atk_mul, gh_mul) = if hd.attr & attr::AT_AH != 0 {
        (0.0, 0.6)
    } else {
        (0.7, 0.6)
    };
    hd.hit_getpower = f("getpower", 0).map_or((atk_mul * hd.damage as f32) as i32, |v| v as i32);
    hd.guard_getpower =
        f("getpower", 1).map_or((hd.hit_getpower as f32 * 0.5) as i32, |v| v as i32);
    hd.hit_givepower = f("givepower", 0).map_or((gh_mul * hd.damage as f32) as i32, |v| v as i32);
    hd.guard_givepower =
        f("givepower", 1).map_or((hd.hit_givepower as f32 * 0.5) as i32, |v| v as i32);
    hd
}

// ------------------------------------------------------------------ boxes

/// Size box `[back, top, front, bottom]` in the player's local space (facing-relative).
pub fn size_box(w: &World, i: usize) -> [f32; 4] {
    let (front, back) = w.player_width(i);
    let height = w.players[i].size.height;
    [-back, -height, front, 0.0]
}

/// Clsn boxes of the current frame, scaled to stage space. group 1 = attack, 2 = hurt, 3 = size.
pub fn clsn_world(w: &World, i: usize, group: u8) -> Vec<[f32; 4]> {
    let p = &w.players[i];
    let scale = w.data_scale(p.data);
    let mut out = Vec::new();
    if group == 3 {
        let b = size_box(w, i);
        out.push([b[0] * scale, b[1] * scale, b[2] * scale, b[3] * scale]);
        return out;
    }
    // a helper without an animation has no boxes (Ikemen: nil anim)
    if p.anim_none {
        return out;
    }
    let d = &w.data[p.anim_owner];
    let Some(frame) = d.air.get(p.anim.action).and_then(|a| p.anim.frame(a)) else {
        return out;
    };
    let anim_scale = w.data_scale(p.anim_owner);
    // Clsn scale is the animation owner's [Size] xscale / yscale constants, even
    // for a helper with its own size.* (MUGEN; Ikemen `clsnScaleLocal`)
    let sx = anim_scale * d.konst("size.xscale").unwrap_or(1.0);
    let sy = anim_scale * d.konst("size.yscale").unwrap_or(1.0);
    let boxes = if group == 1 {
        &frame.clsn1
    } else {
        &frame.clsn2
    };
    for b in boxes {
        out.push([
            b[0] as f32 * sx,
            b[1] as f32 * sy,
            b[2] as f32 * sx,
            b[3] as f32 * sy,
        ]);
    }
    out
}

fn stage_pos(w: &World, i: usize) -> [f32; 2] {
    let p = &w.players[i];
    let scale = w.data_scale(p.data);
    [p.pos[0] * scale, p.pos[1] * scale]
}

fn local_scale(w: &World, i: usize) -> f32 {
    w.local_scale(i)
}

// ------------------------------------------------------------------ push

pub fn push_detection(w: &mut World) {
    let n = w.players.len();
    for a in 0..n {
        for b in (a + 1)..n {
            let (pa_, pb_) = (&w.players[a], &w.players[b]);
            if pa_.removed || pb_.removed || !pa_.player_type || !pb_.player_type {
                continue;
            }
            if !pa_.player_push || !pb_.player_push {
                continue;
            }
            if w.players[a].team == w.players[b].team {
                continue;
            }
            let (ba, bb) = (clsn_world(w, a, 3), clsn_world(w, b, 3));
            let (pa, pb) = (stage_pos(w, a), stage_pos(w, b));
            let (fa, fb) = (w.players[a].facing, w.players[b].facing);
            let Some(overlap) = boxes_overlap(&ba, pa, fa, &bb, pb, fb) else {
                continue;
            };
            if overlap <= 0.0 {
                continue;
            }
            // require hurtbox contact too (ASF_sizepushonly off)
            let (ha, hb) = (clsn_world(w, a, 2), clsn_world(w, b, 2));
            if boxes_overlap(&ha, pa, fa, &hb, pb, fb).is_none() {
                continue;
            }
            w.players[a].pushed = true;
            w.players[b].pushed = true;
            // direction: who is on the right
            let mut dir = pb[0] - pa[0];
            if dir == 0.0 {
                let (ma, mb) = (w.players[a].move_type, w.players[b].move_type);
                // the one being hit (or not attacking, or higher up) yields
                let a_yields = (ma == MoveType::H && mb != MoveType::H)
                    || (ma != MoveType::A
                        && mb == MoveType::A
                        && !(ma == MoveType::H || mb == MoveType::H))
                    || (ma == mb && pa[1] >= pb[1]);
                dir = if a_yields { -fa } else { fb };
            }
            let half = overlap * 0.5;
            let (sa, sb) = (local_scale(w, a), local_scale(w, b));
            let (old_a, old_b) = (w.players[a].pos[0], w.players[b].pos[0]);
            if dir > 0.0 {
                w.players[b].pos[0] += half / sb;
                w.players[a].pos[0] -= half / sa;
            } else {
                w.players[b].pos[0] -= half / sb;
                w.players[a].pos[0] += half / sa;
            }
            w.screen_bound(a);
            w.screen_bound(b);
            // the cornered one can't move: give the rest to the other
            let moved_a = (w.players[a].pos[0] - old_a).abs() * sa;
            let moved_b = (w.players[b].pos[0] - old_b).abs() * sb;
            let eps = 0.0001;
            if moved_a + eps < half && moved_b + eps >= half {
                let extra = overlap - moved_a - moved_b;
                if extra > 0.0 {
                    let d = if w.players[b].pos[0] > old_b {
                        1.0
                    } else {
                        -1.0
                    };
                    w.players[b].pos[0] += d * extra / sb;
                    w.screen_bound(b);
                }
            } else if moved_b + eps < half && moved_a + eps >= half {
                let extra = overlap - moved_a - moved_b;
                if extra > 0.0 {
                    let d = if w.players[a].pos[0] > old_a {
                        1.0
                    } else {
                        -1.0
                    };
                    w.players[a].pos[0] += d * extra / sa;
                    w.screen_bound(a);
                }
            }
            for i in [a, b] {
                let p = &mut w.players[i];
                p.old_pos[0] = p.pos[0];
            }
        }
    }
}

// ------------------------------------------------------------------ hits

fn attr_check(w: &World, getter: usize, attacker: usize, hd: &HitDef) -> bool {
    let g = &w.players[getter];
    if g.unhittable_time > 0 || g.standby || hd.attr == 0 {
        return false;
    }
    if hd.chainid >= 0 && g.ghv.hitid != hd.chainid {
        return false;
    }
    let last_by = g
        .ghv
        .targeted_by
        .last()
        .is_some_and(|t| t.0 == w.players[attacker].id);
    if last_by || g.ghv.hitshaketime > 0 {
        for nci in hd.nochainid {
            if nci >= 0 && g.ghv.hitid == nci && g.ghv.playerid == w.players[attacker].id {
                return false;
            }
        }
    }
    if !hit::hitflag_allows(hd.hitflag, g.state_type, g.hittmp, g.in_guard_state()) {
        return false;
    }
    // HitBy / NotHitBy slots: the attack's attr must be allowed by every active slot
    let attacker_st = attr::state_bits(w.players[attacker].state_type);
    let test_attr = (hd.attr & !attr::ST_MASK) | attacker_st;
    for slot in &g.hitby {
        if slot.time == 0 {
            continue;
        }
        let matches = test_attr & slot.attr & attr::ST_MASK != 0
            && test_attr & slot.attr & !attr::ST_MASK != 0;
        if slot.not == matches {
            return false;
        }
    }
    true
}

/// Priority / trade resolution (`hittableByChar`), simplified to the no-reversal case.
fn hittable(w: &mut World, getter: usize, attacker: usize, hd: &HitDef) -> bool {
    if !attr_check(w, getter, attacker, hd) {
        return false;
    }
    let g = &w.players[getter];
    if g.hitdef_targets.contains(&w.players[attacker].id) {
        return true;
    }
    let Some(ghd) = g.hitdef.clone() else {
        return true;
    };
    if g.move_type == MoveType::I || ghd.attr == 0 || g.state_type == StateType::L {
        return true;
    }
    // can the getter hit the attacker back this same tick?
    let counter = {
        let a = &w.players[attacker];
        (a.move_type != MoveType::I || !g.targets.contains(&a.id))
            && !a.hitdef_targets.contains(&g.id)
            && attr_check(w, attacker, getter, &ghd)
            && clsn_contact(w, getter, attacker)
    };
    if ghd.priority < hd.priority {
        return true;
    }
    if ghd.priority > hd.priority {
        return !counter;
    }
    match (ghd.trade, hd.trade) {
        (Trade::Dodge, _) | (_, Trade::Dodge) | (_, Trade::Miss) => !counter,
        _ => true,
    }
}

/// Attacker's Clsn1 overlaps getter's Clsn2.
fn clsn_contact(w: &World, attacker: usize, getter: usize) -> bool {
    let a = clsn_world(w, attacker, 1);
    let b = clsn_world(w, getter, 2);
    boxes_overlap(
        &a,
        stage_pos(w, attacker),
        w.players[attacker].facing,
        &b,
        stage_pos(w, getter),
        w.players[getter].facing,
    )
    .is_some()
}

fn compute_damage(
    w: &World,
    getter: usize,
    attacker: usize,
    damage: i32,
    kill: bool,
    bounds: bool,
) -> i32 {
    if damage == 0 {
        return 0;
    }
    let a = &w.players[attacker];
    let g = &w.players[getter];
    let atk = a.attack_mul * w.data[a.data].konst("data.attack").unwrap_or(100.0) / 100.0;
    let def = w.data[g.data].konst("data.defence").unwrap_or(100.0) / 100.0
        * g.defence_mul
        * g.fall_defence_mul;
    let mut d = damage as f32 * atk / def.max(0.0001);
    if d > 0.0 && d < 1.0 {
        d = 1.0;
    }
    if bounds && d > g.life as f32 {
        d = g.life as f32;
    }
    if !kill && d >= g.life as f32 {
        d = (g.life - 1) as f32;
    }
    libm::roundf(d) as i32
}

pub fn hit_detection(w: &mut World) {
    let n = w.players.len();
    for p in &mut w.players {
        p.gethit = false;
        p.hover_idx = -1;
    }
    reversal_detection(w);
    // attackers with a HitDef first, by id
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| {
        (
            w.players[i].hitdef.as_ref().is_none_or(|h| h.attr == 0),
            w.players[i].id,
        )
    });
    for &c in &order {
        let p = &w.players[c];
        if p.removed || p.standby || p.move_type == MoveType::I {
            continue;
        }
        let Some(hd) = p.hitdef.clone() else { continue };
        if hd.attr == 0 {
            continue;
        }
        let mut candidates: Vec<usize> = Vec::new();
        for g in 0..n {
            if g == c {
                continue;
            }
            let team_ok = hd.affectteam == 0
                || ((w.players[g].team != w.players[c].team) == (hd.affectteam > 0));
            if !team_ok {
                continue;
            }
            // guard distance
            if !w.players[g].inguarddist && w.players[c].move_type == MoveType::A {
                let dx = w.rel_pos(c, g)[0] * w.players[c].facing;
                if dx < hd.guard_dist && dx > -hd.guard_dist {
                    w.players[g].inguarddist = true;
                }
            }
            let airjuggle = w.data[w.players[c].data]
                .konst("data.airjuggle")
                .unwrap_or(15.0) as i32;
            let can_juggle = w.players[c].juggle
                <= w.players[g].ghv.juggle_for(w.players[c].id, airjuggle)
                || !w.players[c].targets.contains(&w.players[g].id);
            if !can_juggle
                || hd.hitonce < 0
                || w.players[c].hitdef_targets.contains(&w.players[g].id)
            {
                continue;
            }
            if !hittable(w, g, c, &hd) {
                continue;
            }
            candidates.push(g);
        }
        candidates.sort_by(|&x, &y| {
            let dx = w.rel_pos(c, x)[0].abs();
            let dy = w.rel_pos(c, y)[0].abs();
            dx.partial_cmp(&dy).unwrap_or(std::cmp::Ordering::Equal)
        });
        for g in candidates {
            if w.players[c].hitdef.as_ref().is_none_or(|h| h.hitonce < 0) {
                break;
            }
            if !clsn_contact(w, c, g) {
                continue;
            }
            let result = hit_result(w, c, g, &hd, HitSrc::Body);
            if result == 0 {
                continue;
            }
            let hd = w.players[c].hitdef.clone().unwrap();
            let gst = w.players[g].state_type;
            if result.abs() == 1 {
                w.players[c].mctype = MoveContact::Hit;
                w.players[c].mctime = -1;
                let no_reaction = (gst == StateType::A && hd.air_type == HitType::None)
                    || (gst != StateType::A && hd.ground_type == HitType::None);
                if !w.players[c].gethit && !no_reaction {
                    w.players[c].hit_pause_time = (hd.pausetime[0] + 1).max(1);
                }
                w.players[c].uniq_hit_count += 1;
            } else {
                w.players[c].mctype = MoveContact::Guarded;
                w.players[c].mctime = -1;
                if !w.players[c].gethit {
                    w.players[c].hit_pause_time = (hd.guard_pausetime[0] + 1).max(1);
                }
            }
            if hd.hitonce > 0
                && let Some(h) = w.players[c].hitdef.as_mut()
            {
                h.hitonce = -1;
            }
            w.players[c].mhv.frame = true;
            w.players[c].mhv.playerid = w.players[g].id;
        }
    }
}

/// ReversalDef: a player whose HitDef has `reversal.attr` catches any matching
/// attack whose Clsn1 touches its own Clsn1; the reversal HitDef then hits the attacker.
fn reversal_detection(w: &mut World) {
    let n = w.players.len();
    for g in 0..n {
        let Some(rhd) = w.players[g].hitdef.clone() else {
            continue;
        };
        if rhd.reversal_attr == 0 || w.players[g].removed {
            continue;
        }
        for c in 0..n {
            if c == g || w.players[c].removed || w.players[c].move_type != MoveType::A {
                continue;
            }
            let Some(hd) = w.players[c].hitdef.clone() else {
                continue;
            };
            if hd.attr == 0
                || hd.attr & rhd.reversal_attr & attr::ST_MASK == 0
                || hd.attr & rhd.reversal_attr & !attr::ST_MASK == 0
                || w.players[g].hitdef_targets.contains(&w.players[c].id)
            {
                continue;
            }
            let team_ok = rhd.affectteam == 0
                || ((w.players[c].team != w.players[g].team) == (rhd.affectteam > 0));
            if !team_ok {
                continue;
            }
            let a = clsn_world(w, c, 1);
            let b = clsn_world(w, g, 1);
            if boxes_overlap(
                &a,
                stage_pos(w, c),
                w.players[c].facing,
                &b,
                stage_pos(w, g),
                w.players[g].facing,
            )
            .is_none()
            {
                continue;
            }
            let result = hit_result(w, g, c, &rhd, HitSrc::Body);
            if result == 0 {
                continue;
            }
            // the reversed attack is spent
            if let Some(h) = w.players[c].hitdef.as_mut() {
                h.attr = 0;
            }
            w.players[c].hittmp = -1;
            let cid = w.players[c].id;
            let p = &mut w.players[g];
            p.mctype = MoveContact::Hit;
            p.mctime = -1;
            if !p.gethit {
                p.hit_pause_time = (rhd.pausetime[0] + 1).max(1);
            }
            p.mhv.frame = true;
            p.mhv.playerid = cid;
            p.uniq_hit_count += 1;
        }
    }
}

/// Apply `attacker`'s HitDef to `getter`. Returns 1 hit, 2 guarded, negative = no reaction, 0 = no contact.
/// What delivers a hit: the attacker's body, one of its projectiles, or an
/// arena item it threw (stage position and facing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HitSrc {
    Body,
    Proj(usize),
    Item { pos: [f32; 2], facing: f32 },
}

pub(crate) fn hit_result(w: &mut World, c: usize, g: usize, hd: &HitDef, src: HitSrc) -> i32 {
    let hd = hd.clone();
    let is_proj = src != HitSrc::Body;
    {
        let gp = &w.players[g];
        if gp.pending_state.is_some() && gp.state_owner != w.players[c].data {
            if gp.gethit {
                if hd.p2stateno >= 0 {
                    return 0;
                }
            } else if gp.hit_pause_time == 0 {
                return 0;
            }
        }
        if !is_proj && hd.p1stateno >= 0 && w.players[c].gethit {
            return 0;
        }
        if gp.gethit && gp.ghv.attr & attr::AT_AT != 0 {
            return 0;
        }
    }
    let getter_in_combo =
        (w.players[g].move_type == MoveType::H || w.players[g].gethit) && !w.players[g].ghv.guarded;
    let can_guard = w.players[g].guarding && (!w.players[g].gethit || w.players[g].ghv.guarded);
    let mut result: i32 = 1;
    w.players[g].ghv.kill = hd.kill;
    if can_guard {
        let gst = w.players[g].state_type;
        if (hd.guardflag & hf::H != 0 && gst == StateType::S)
            || (hd.guardflag & hf::L != 0 && gst == StateType::C)
            || (hd.guardflag & hf::A != 0 && gst == StateType::A)
        {
            w.players[g].ghv.kill = hd.guard_kill;
            let dmg = compute_damage(w, g, c, hd.guard_damage, hd.guard_kill, true);
            if w.players[g].life > dmg {
                result = 2;
            } else {
                w.players[g].ghv.guardko = true;
            }
        }
    }
    let gst = w.players[g].state_type;
    if (gst == StateType::A && hd.air_type == HitType::None)
        || (gst != StateType::A && hd.ground_type == HitType::None)
    {
        result = -result;
    }
    if w.players[g].ghv.damage >= w.players[g].life {
        w.players[g].ghv.kill = true;
    }

    // HitOverride slots on the getter
    if w.players[g].pending_state.is_none() || !w.players[g].gethit {
        let attacker_st = attr::state_bits(w.players[c].state_type);
        let mut chosen = -1;
        for (k, ho) in w.players[g].hover.iter().enumerate() {
            if ho.time == 0 || ho.attr & hd.attr & !attr::ST_MASK == 0 {
                continue;
            }
            let st_ok = if is_proj {
                ho.attr & hd.attr & attr::ST_MASK != 0
            } else {
                ho.attr & attacker_st != 0
            };
            if !st_ok {
                continue;
            }
            // p1stateno / p2stateno together with HitOverride: the attack misses
            if !is_proj && (hd.p1stateno >= 0 || hd.p2stateno >= 0) {
                return 0;
            }
            chosen = k as i32;
            break;
        }
        if chosen >= 0 {
            let ho = w.players[g].hover[chosen as usize];
            w.players[g].hover_idx = chosen;
            if ho.forceair {
                if (result > 0 && hd.air_type == HitType::None)
                    || (result < 0
                        && hd.ground_type == HitType::None
                        && hd.air_type != HitType::None)
                {
                    result = -result;
                }
                if result.abs() == 1 {
                    w.players[g].state_type = StateType::A;
                }
            }
            if result > 0 {
                result = if ho.forceguard { 2 } else { 1 };
            }
        }
    }

    // custom state
    let mut p2s = false;
    if (w.players[g].pending_state.is_none() || !w.players[g].gethit)
        && result.abs() == 1
        && hd.p2stateno >= 0
    {
        let owner = if hd.p2getp1state {
            w.players[c].data
        } else {
            w.players[g].data
        };
        w.players[g].state_owner = owner;
        w.change_state(g, hd.p2stateno, None, Some(false));
        w.players[g].move_type = MoveType::H;
        p2s = true;
    }
    let gid = w.players[g].id;
    if !is_proj {
        w.players[c].hitdef_targets.push(gid);
    }

    let ghvset = !w.players[g].gethit || w.players[g].pending_state.is_none() || p2s;
    if ghvset {
        w.players[g].ghv.hitid = hd.id;
        w.players[g].ghv.playerid = w.players[c].id;
        w.players[g].ghv.groundtype = hd.ground_type;
        w.players[g].ghv.airtype = hd.air_type;
        w.players[g].ghv.hit_type = if gst == StateType::A {
            hd.air_type
        } else {
            hd.ground_type
        };
        if !is_proj {
            w.players[c].spr_priority = hd.p1sprpriority;
        }
        w.players[g].spr_priority = hd.p2sprpriority;
    }
    let mut byf = match src {
        HitSrc::Proj(k) => w.projs[k].facing,
        HitSrc::Item { facing, .. } => facing,
        HitSrc::Body => w.players[c].facing,
    };
    if !is_proj && result == 1 {
        if hd.p1getp2facing != 0 {
            byf = w.players[g].facing;
            if hd.p1getp2facing < 0 {
                byf = -byf;
            }
        } else if hd.p1facing < 0 {
            byf = -byf;
        }
    }
    let scale = local_scale(w, c) / local_scale(w, g);

    if result > 0 {
        if ghvset {
            let data_g = w.data[w.players[g].data].clone();
            let g_pos_y = w.players[g].pos[1];
            let cid = w.players[c].id;
            let mut hit_palfx_now = None;
            let ghv = &mut w.players[g].ghv;
            ghv.selective_reset();
            ghv.attr = hd.attr;
            ghv.guardflag = hd.guardflag;
            ghv.hitid = hd.id;
            ghv.playerid = cid;
            ghv.yaccel = hd.yaccel * scale;
            ghv.groundtype = hd.ground_type;
            ghv.airtype = hd.air_type;
            if hd.forcenofall {
                ghv.fallflag = false;
            }
            ghv.hit_type = if gst == StateType::A {
                hd.air_type
            } else {
                hd.ground_type
            };
            if result == 2 {
                ghv.guarded = true;
                ghv.hitshaketime = hd.guard_pausetime[1].max(0);
                ghv.hittime = hd.guard_hittime.max(0);
                ghv.slidetime = hd.guard_slidetime;
                if gst == StateType::A {
                    ghv.ctrltime = hd.airguard_ctrltime;
                    ghv.xvel = hd.airguard_velocity[0] * scale * -byf;
                    ghv.yvel = hd.airguard_velocity[1] * scale;
                } else {
                    ghv.ctrltime = hd.guard_ctrltime;
                    ghv.xvel = hd.guard_velocity * scale * -byf;
                    ghv.yvel = 0.0;
                }
                ghv.fallflag = false;
                ghv.guardcount += 1;
            } else {
                ghv.guarded = false;
                ghv.hitshaketime = hd.pausetime[1].max(0);
                ghv.slidetime = hd.ground_slidetime;
                ghv.p2getp1state = hd.p2getp1state;
                ghv.forcestand = hd.forcestand;
                ghv.fall_xvelocity = hd.fall_xvelocity.map(|v| v * scale);
                ghv.fall_yvelocity = hd.fall_yvelocity * scale;
                ghv.fall_recover = hd.fall_recover;
                ghv.fall_recovertime = hd.fall_recovertime;
                ghv.fall_damage = hd.fall_damage;
                ghv.fall_kill = hd.fall_kill;
                ghv.fall_envshake_time = hd.fall_envshake_time;
                ghv.fall_envshake_freq = hd.fall_envshake_freq;
                ghv.fall_envshake_ampl = (hd.fall_envshake_ampl as f32 * scale) as i32;
                ghv.fall_envshake_phase = hd.fall_envshake_phase;
                hit_palfx_now = hd.palfx.clone();
                if hd.envshake_time > 0 {
                    w.envshake = crate::fx::EnvShake {
                        time: hd.envshake_time,
                        freq: hd.envshake_freq,
                        ampl: (hd.envshake_ampl as f32 * scale) as i32 as f32,
                        phase: hd.envshake_phase,
                        ..Default::default()
                    };
                    w.envshake.start();
                }
                match gst {
                    StateType::A => {
                        ghv.hittime = hd.air_hittime;
                        ghv.ctrltime = hd.air_hittime;
                        ghv.xvel = hd.air_velocity[0] * scale * -byf;
                        ghv.yvel = hd.air_velocity[1] * scale;
                        ghv.fallflag = ghv.fallflag || hd.air_fall;
                    }
                    StateType::L => {
                        ghv.hittime = hd.down_hittime;
                        ghv.ctrltime = hd.down_hittime;
                        ghv.fallflag = ghv.fallflag || hd.ground_fall;
                        if g_pos_y == 0.0 {
                            ghv.xvel = hd.down_velocity[0] * scale * -byf;
                            ghv.yvel = hd.down_velocity[1] * scale;
                            if !hd.down_bounce && ghv.yvel != 0.0 {
                                ghv.fall_xvelocity = None;
                                ghv.fall_yvelocity = 0.0;
                            }
                        } else {
                            ghv.xvel = hd.air_velocity[0] * scale * -byf;
                            ghv.yvel = hd.air_velocity[1] * scale;
                        }
                    }
                    _ => {
                        ghv.ctrltime = hd.ground_hittime;
                        ghv.xvel = hd.ground_velocity[0] * scale * -byf;
                        ghv.yvel = hd.ground_velocity[1] * scale;
                        ghv.fallflag = ghv.fallflag || hd.ground_fall;
                        if ghv.fallflag && ghv.yvel == 0.0 {
                            ghv.yvel = -0.001 * scale;
                        }
                        ghv.hittime = if ghv.yvel != 0.0 {
                            hd.air_hittime
                        } else {
                            hd.ground_hittime
                        };
                    }
                }
                if ghv.hittime < 0 {
                    ghv.hittime = 0;
                }
                if ghv.hittime > 0 {
                    ghv.hittime += 1; // MUGEN chars
                }
                if getter_in_combo {
                    ghv.hitcount += 1;
                } else {
                    ghv.hitcount = 1;
                }
                let liedown = data_g.konst("data.liedown.time").unwrap_or(60.0) as i32;
                if ghv.down_recovertime > liedown || gst != StateType::L {
                    ghv.down_recovertime = liedown;
                }
            }
            ghv.ground_animtype = hd.animtype;
            ghv.air_animtype = hd.air_animtype;
            ghv.fall_animtype = hd.fall_animtype;
            ghv.animtype = hit::gethit_animtype(ghv, gst);
            if let Some(pf) = hit_palfx_now {
                w.players[g].palfx = pf;
            }
            // mindist / maxdist snapping
            let by_x = match src {
                HitSrc::Proj(k) => w.projs[k].pos[0],
                HitSrc::Item { pos, .. } => pos[0],
                HitSrc::Body => w.players[c].pos[0] * local_scale(w, c),
            };
            let g_x = w.players[g].pos[0] * local_scale(w, g);
            let mut snap: Option<f32> = None;
            if let Some(md) = hd.mindist[0] {
                let md = md * local_scale(w, c);
                if byf < 0.0 && g_x > by_x - md {
                    snap = Some(by_x - md);
                } else if byf > 0.0 && g_x < by_x + md {
                    snap = Some(by_x + md);
                }
            }
            if let Some(md) = hd.maxdist[0] {
                let md = md * local_scale(w, c);
                if byf < 0.0 && g_x < by_x - md {
                    snap = Some(by_x - md);
                } else if byf > 0.0 && g_x > by_x + md {
                    snap = Some(by_x + md);
                }
            }
            if let Some(sx) = snap {
                let gs = local_scale(w, g);
                w.players[g].ghv.xoff = sx / gs - w.players[g].pos[0];
            }
            if result == 1 || gst == StateType::A {
                let by_y = match src {
                    HitSrc::Proj(k) => w.projs[k].pos[1] / local_scale(w, c),
                    HitSrc::Item { pos, .. } => pos[1] / local_scale(w, c),
                    HitSrc::Body => w.players[c].pos[1],
                };
                let g_y = w.players[g].pos[1];
                let mut sy: Option<f32> = None;
                if let Some(md) = hd.mindist[1]
                    && g_y < by_y + md
                {
                    sy = Some(by_y + md);
                }
                if let Some(md) = hd.maxdist[1]
                    && g_y > by_y + md
                {
                    sy = Some(by_y + md);
                }
                if let Some(v) = sy {
                    w.players[g].ghv.yoff = v - w.players[g].pos[1];
                }
            }
            w.players[g].ghv.priority = hd.priority;
        }
        if !p2s && !w.players[g].gethit {
            w.players[g].pending_state = None;
        }
        w.players[g].gethit = true;
        w.players[g].ghv.frame = true;
        if result == 1 {
            let kill = w.players[g].ghv.kill;
            let d = compute_damage(w, g, c, hd.damage, kill, true);
            w.players[g].ghv.damage += d;
        } else {
            let kill = w.players[g].ghv.kill;
            let d = compute_damage(w, g, c, hd.guard_damage, kill, true);
            w.players[g].ghv.damage += d;
        }
        if ghvset && w.players[g].ghv.damage >= w.players[g].life {
            if w.players[g].ghv.kill || !w.players[g].alive() {
                let ghv = &mut w.players[g].ghv;
                ghv.fallflag = true;
                ghv.animtype = hit::gethit_animtype(ghv, gst);
            } else {
                w.players[g].ghv.damage = w.players[g].life - 1;
            }
        }
    }

    if result > 0 {
        let (gp, gv) = if result == 1 {
            (hd.hit_getpower, hd.hit_givepower)
        } else {
            (hd.guard_getpower, hd.guard_givepower)
        };
        w.players[c].mhv.power = w.players[c].mhv.power.saturating_add(gp);
        w.players[g].ghv.power = w.players[g].ghv.power.saturating_add(gv);
    }
    if result == 1 {
        if !is_proj {
            w.players[c].counter_hit = w.players[g].move_type == MoveType::A;
        }
        let no_reaction = (hd.air_type == HitType::None && gst == StateType::A)
            || (gst != StateType::A && hd.ground_type == HitType::None);
        if (ghvset || w.players[g].gethit) && !no_reaction {
            w.players[g].received_hits += hd.numhits;
        }
    }
    // sparks and sounds
    {
        let (own_spark, spark, own_snd, snd) = if result.abs() == 1 {
            (hd.sparkno_own, hd.sparkno, hd.hitsound_own, hd.hitsound)
        } else {
            (
                hd.guard_sparkno_own,
                hd.guard_sparkno,
                hd.guardsound_own,
                hd.guardsound,
            )
        };
        let (gf, _) = w.player_width(g);
        let getter_edge = if w.players[c].facing != w.players[g].facing {
            gf
        } else {
            -w.player_width(g).1
        };
        let gs = local_scale(w, g);
        let cs = local_scale(w, c);
        let x = w.players[g].pos[0] * gs + getter_edge * gs * w.players[g].facing
            - hd.sparkxy[0] * w.players[c].facing * cs;
        let y = (w.players[c].pos[1] + hd.sparkxy[1]) * cs;
        if spark >= 0 {
            let f = w.players[c].facing;
            w.spawn_spark(c, own_spark, spark, [x, y], f);
        }
        if snd[0] >= 0 && snd[1] >= 0 {
            w.events.push(Event::Sound {
                player: c,
                own: own_snd,
                group: snd[0],
                number: snd[1],
                channel: -1,
                volume: 100,
            });
        }
    }
    if !is_proj && hd.hitonce > 0 {
        let keep = w.players[g].id;
        w.players[c].targets.retain(|&t| t == keep);
    }
    if !w.players[c].targets.contains(&gid) {
        w.players[c].targets.push(gid);
    }
    let airjuggle = w.data[w.players[c].data]
        .konst("data.airjuggle")
        .unwrap_or(15.0) as i32;
    let cid = w.players[c].id;
    w.players[g].ghv.add_id(cid, airjuggle);

    if !ghvset {
        return result;
    }
    w.players[g].p1facing = 0.0;
    if result.abs() == 1 {
        if !is_proj && (hd.p1getp2facing != 0 || hd.p1facing < 0) && w.players[c].facing != byf {
            w.players[c].p1facing = byf;
        }
        if hd.p2facing < 0 {
            w.players[g].p1facing = byf;
        } else if hd.p2facing > 0 {
            w.players[g].p1facing = -byf;
        }
        if w.players[g].p1facing == w.players[g].facing {
            w.players[g].p1facing = 0.0;
        }
        w.players[g].ghv.facing = hd.p2facing;
        if !is_proj && hd.p1stateno >= 0 {
            w.change_state(c, hd.p1stateno, None, Some(false));
        }
        if w.players[g].prev_fallflag || w.players[g].ghv.fallflag {
            let jug = if is_proj {
                hd.air_juggle
            } else {
                w.players[c].juggle
            };
            if let Some(t) = w.players[g].ghv.targeted_by.last_mut() {
                t.1 -= jug;
            }
            if !is_proj {
                w.players[c].juggle = 0;
            }
        }
        if !is_proj && result > 0 {
            w.players[c].mhv.cornerpush_veloff = match gst {
                StateType::A => hd.air_cornerpush_veloff,
                StateType::L => hd.down_cornerpush_veloff,
                _ => hd.ground_cornerpush_veloff,
            } * w.players[c].facing;
        }
    }
    if !is_proj && result == 2 {
        w.players[c].mhv.cornerpush_veloff = if gst == StateType::A {
            hd.airguard_cornerpush_veloff
        } else {
            hd.guard_cornerpush_veloff
        } * w.players[c].facing;
    }
    // hit from behind: high <-> low
    let by = if w.players[c].p1facing != 0.0 {
        w.players[c].p1facing
    } else {
        w.players[c].facing
    };
    if (w.players[g].facing < 0.0) == (by < 0.0) {
        let ghv = &mut w.players[g].ghv;
        let flip = |t: HitType| match t {
            HitType::High => HitType::Low,
            HitType::Low => HitType::High,
            o => o,
        };
        ghv.groundtype = flip(ghv.groundtype);
        ghv.airtype = flip(ghv.airtype);
    }
    result
}

/// Does `p` currently count as guarding (`SCF_guard`)?
pub fn compute_guard_flag(p: &Player) -> bool {
    let movetype_ok =
        ((p.ctrl || p.state_no == 52) && p.move_type == MoveType::I) || p.in_guard_state();
    movetype_ok
        && p.input.bb > 0
        && match p.state_type {
            StateType::S => !p.asserted(crate::player::assert::NOSTANDGUARD),
            StateType::C => !p.asserted(crate::player::assert::NOCROUCHGUARD),
            StateType::A => !p.asserted(crate::player::assert::NOAIRGUARD),
            _ => false,
        }
}

/// Projectile Clsn1 boxes in stage space.
fn proj_clsn(w: &World, k: usize, group: u8) -> Vec<[f32; 4]> {
    let p = &w.projs[k];
    let d = &w.data[w.players[p.owner].data];
    let Some(frame) = d.air.get(p.anim_no).and_then(|a| p.anim.frame(a)) else {
        return Vec::new();
    };
    let s = w.data_scale(w.players[p.owner].data);
    let boxes = if group == 1 {
        &frame.clsn1
    } else {
        &frame.clsn2
    };
    boxes
        .iter()
        .map(|b| {
            [
                b[0] as f32 * s * p.scale[0],
                b[1] as f32 * s * p.scale[1],
                b[2] as f32 * s * p.scale[0],
                b[3] as f32 * s * p.scale[1],
            ]
        })
        .collect()
}

pub fn projectile_hit_detection(w: &mut World) {
    let n = w.players.len();
    for g in 0..n {
        if w.players[g].removed {
            continue;
        }
        for k in 0..w.projs.len() {
            let p = w.projs[k].clone();
            if p.removed || p.hits <= 0 || !p.is_active() {
                continue;
            }
            let c = p.owner;
            if w.players[c].removed {
                continue;
            }
            // a root player's own projectile never hits it
            if g == w.players[c].root
                && w.players[g].helper_index == 0
                && w.players[g].team == w.players[c].team
            {
                continue;
            }
            let hd = &p.hitdef;
            if hd.attr == 0 {
                continue;
            }
            let team_ok = hd.affectteam == 0
                || ((w.players[g].team != w.players[c].team) == (hd.affectteam > 0));
            if !team_ok {
                continue;
            }
            let gs = local_scale(w, g);
            let dist_x = (w.players[g].pos[0] * gs - p.pos[0]) * p.facing;
            if !w.players[g].inguarddist
                && dist_x < hd.guard_dist * local_scale(w, c)
                && dist_x > -hd.guard_dist * local_scale(w, c)
            {
                w.players[g].inguarddist = true;
            }
            let airjuggle = w.data[w.players[c].data]
                .konst("data.airjuggle")
                .unwrap_or(15.0) as i32;
            let can_juggle =
                hd.air_juggle <= w.players[g].ghv.juggle_for(w.players[c].id, airjuggle);
            let getter_busy = w.players[g].pending_state.is_some()
                && (w.players[g].gethit || w.players[g].hit_pause_time == 0);
            if !can_juggle
                || getter_busy
                || !(p.hitpause <= 0 || p.contactflag)
                || p.curmisstime > 0
                || hd.hitonce < 0
            {
                continue;
            }
            if !attr_check(w, g, c, hd) {
                continue;
            }
            let a = proj_clsn(w, k, 1);
            let b = clsn_world(w, g, 2);
            if boxes_overlap(
                &a,
                p.pos,
                p.facing,
                &b,
                stage_pos(w, g),
                w.players[g].facing,
            )
            .is_none()
            {
                continue;
            }
            let hd = hd.clone();
            let result = hit_result(w, c, g, &hd, HitSrc::Proj(k));
            if result == 0 {
                continue;
            }
            let pr = &mut w.projs[k];
            pr.contactflag = true;
            let owner = &mut w.players[c];
            if result.abs() == 1 {
                owner.pctype = crate::projectile::ProjContact::Hit;
                pr.hitpause = hd.pausetime[0].max(0);
            } else {
                owner.pctype = crate::projectile::ProjContact::Guarded;
                pr.hitpause = hd.guard_pausetime[0].max(0);
            }
            owner.pctime = 0;
            owner.pcid = pr.id;
        }
    }
}
