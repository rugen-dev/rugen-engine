//! Projectiles: owned HitDefs that travel on their own.

use rugen_cns::state::Controller;
use rugen_cns::{Code, Result};

use crate::anim::AnimState;
use crate::explod::PosType;
use crate::hit::{HitDef, HitDefSpec};
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjStatus {
    Active,
    Hit,
    Cancel,
    Rem,
}

/// Owner-side record of the last projectile contact (ProjHitTime & co).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProjContact {
    #[default]
    None,
    Hit,
    Guarded,
    Cancel,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ProjectileSpec {
    pub hitdef: HitDefSpec,
    pub projid: Option<Code>,
    pub projanim: Option<Code>,
    pub projhitanim: Option<Code>,
    pub projremanim: Option<Code>,
    pub projcancelanim: Option<Code>,
    pub projscale: Option<Vec<Code>>,
    pub projremove: Option<Code>,
    pub projremovetime: Option<Code>,
    pub velocity: Option<Vec<Code>>,
    pub remvelocity: Option<Vec<Code>>,
    pub accel: Option<Vec<Code>>,
    pub velmul: Option<Vec<Code>>,
    pub projhits: Option<Code>,
    pub projmisstime: Option<Code>,
    pub projpriority: Option<Code>,
    pub projsprpriority: Option<Code>,
    pub projedgebound: Option<Code>,
    pub projstagebound: Option<Code>,
    pub projheightbound: Option<Vec<Code>>,
    pub offset: Option<Vec<Code>>,
    pub postype: Option<PosType>,
    pub supermovetime: Option<Code>,
    pub pausemovetime: Option<Code>,
}

impl ProjectileSpec {
    pub fn compile(c: &Controller) -> Result<Self> {
        let one = |n: &str| c.expr(n);
        Ok(Self {
            hitdef: HitDefSpec::compile_with_attr_default(c, "S, NP")?,
            projid: one("projid")?,
            projanim: one("projanim")?,
            projhitanim: one("projhitanim")?,
            projremanim: one("projremanim")?,
            projcancelanim: one("projcancelanim")?,
            projscale: c.exprs("projscale")?,
            projremove: one("projremove")?,
            projremovetime: one("projremovetime")?,
            velocity: c.exprs("velocity")?,
            remvelocity: c.exprs("remvelocity")?,
            accel: c.exprs("accel")?,
            velmul: c.exprs("velmul")?,
            projhits: one("projhits")?,
            projmisstime: one("projmisstime")?,
            projpriority: one("projpriority")?,
            projsprpriority: one("projsprpriority")?,
            projedgebound: one("projedgebound")?,
            projstagebound: one("projstagebound")?,
            projheightbound: c.exprs("projheightbound")?,
            offset: c.exprs("offset")?,
            postype: c.param("postype").and_then(PosType::parse),
            supermovetime: one("supermovetime")?,
            pausemovetime: one("pausemovetime")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub owner: usize,
    pub owner_id: i32,
    pub id: i32,
    pub status: ProjStatus,
    pub hitdef: HitDef,
    pub anim: AnimState,
    pub anim_no: i32,
    pub hitanim: i32,
    /// `i32::MIN` = not set (keep the current anim)
    pub remanim: i32,
    pub cancelanim: i32,
    pub scale: [f32; 2],
    pub remove: bool,
    pub removetime: i32,
    pub velocity: [f32; 2],
    pub remvelocity: [f32; 2],
    pub accel: [f32; 2],
    pub velmul: [f32; 2],
    pub hits: i32,
    pub misstime: i32,
    pub priority: i32,
    pub sprpriority: i32,
    pub edgebound: f32,
    pub stagebound: f32,
    pub heightbound: [f32; 2],
    /// Stage coordinates (stage units).
    pub pos: [f32; 2],
    pub facing: f32,
    pub removefacing: f32,
    pub supermovetime: i32,
    pub pausemovetime: i32,
    pub curmisstime: i32,
    pub hitpause: i32,
    pub contactflag: bool,
    pub freezeflag: bool,
    pub time: i32,
    pub remove_done: bool,
    pub removed: bool,
}

impl Projectile {
    pub fn is_active(&self) -> bool {
        self.status == ProjStatus::Active
    }
}

impl World {
    fn proj_paused(&self, p: &Projectile) -> bool {
        if self.supertime > 0 {
            p.supermovetime == 0 || p.supermovetime < -1
        } else if self.pausetime > 0 {
            p.pausemovetime == 0 || p.pausemovetime < -1
        } else {
            false
        }
    }

    /// Movement, bounds and removal animation (Ikemen `Projectile.update`).
    pub(crate) fn update_projectiles(&mut self) {
        let (xmin, xmax) = self.screen_bounds();
        let (sl, sr) = (self.stage.screen_left, self.stage.screen_right);
        let (cam_min, cam_max) = (self.stage.bound_left, self.stage.bound_right);
        for k in 0..self.projs.len() {
            let mut p = self.projs[k].clone();
            if p.removed {
                continue;
            }
            let paused = self.proj_paused(&p);
            let data = self.data[self.players[p.owner].data].clone();
            if !paused && p.hitpause == 0 {
                if p.status == ProjStatus::Active {
                    // bounds are stage units, the projectile its owner's units
                    let s = self.local_scale(p.owner);
                    let x = p.pos[0] * s;
                    let out = p.removetime == 0
                        || (p.removetime <= -2 && p.anim.loop_end)
                        || x < xmin - sl - p.edgebound * s
                        || x > xmax + sr + p.edgebound * s
                        || (p.velocity[0] * p.facing < 0.0 && x < cam_min - p.stagebound * s)
                        || (p.velocity[0] * p.facing > 0.0 && x > cam_max + p.stagebound * s)
                        || (p.velocity[1] > 0.0 && p.pos[1] > p.heightbound[1])
                        || (p.velocity[1] < 0.0 && p.pos[1] < p.heightbound[0]);
                    if out {
                        p.status = ProjStatus::Rem;
                    }
                }
                if !p.remove_done && p.status != ProjStatus::Active {
                    let next = match p.status {
                        ProjStatus::Hit => p.hitanim,
                        ProjStatus::Cancel => p.cancelanim,
                        ProjStatus::Rem => p.remanim,
                        ProjStatus::Active => i32::MIN,
                    };
                    if next != i32::MIN && next != p.anim_no {
                        if next < 0 || data.air.get(next).is_none() {
                            p.anim_no = -1;
                        } else {
                            p.anim_no = next;
                            p.anim = AnimState::start(next);
                        }
                    }
                    p.remove_done = true;
                    p.velocity = p.remvelocity;
                    p.accel = [0.0, 0.0];
                    p.velmul = [1.0, 1.0];
                    if p.facing != p.removefacing {
                        p.velocity[0] = -p.velocity[0];
                    }
                }
                if p.status != ProjStatus::Active {
                    let done = match data.air.get(p.anim_no) {
                        None => true,
                        Some(a) => a.total_time <= 0 || p.anim.anim_time(a) == 0,
                    };
                    if done {
                        p.anim_no = -1;
                        p.removed = true;
                    }
                }
            }
            if !(paused || p.hitpause > 0 || p.freezeflag) {
                p.pos[0] += p.velocity[0] * p.facing;
                p.pos[1] += p.velocity[1];
                for i in 0..2 {
                    p.velocity[i] += p.accel[i];
                    p.velocity[i] *= p.velmul[i];
                }
                if p.velocity[0] < 0.0 && p.anim_no != -1 {
                    p.facing = -p.facing;
                    p.velocity[0] = -p.velocity[0];
                    p.accel[0] = -p.accel[0];
                }
            }
            self.projs[k] = p;
        }
    }

    /// Timers and animation (Ikemen `Projectile.tick`).
    pub(crate) fn tick_projectiles(&mut self) {
        for k in 0..self.projs.len() {
            let mut p = self.projs[k].clone();
            if p.removed {
                continue;
            }
            if p.contactflag {
                p.contactflag = false;
                p.curmisstime = p.misstime.max(0);
                if p.hits >= 0 {
                    p.hits -= 1;
                    if p.hits <= 0 && p.remove {
                        p.status = ProjStatus::Hit;
                    }
                }
                p.hitdef.air_juggle = 0;
            }
            let paused = self.proj_paused(&p);
            if !paused {
                if p.hitpause <= 0 {
                    p.time += 1;
                    if p.removetime > 0 {
                        p.removetime -= 1;
                    }
                    if p.curmisstime > 0 {
                        p.curmisstime -= 1;
                    }
                    if p.supermovetime > 0 {
                        p.supermovetime -= 1;
                    }
                    if p.pausemovetime > 0 {
                        p.pausemovetime -= 1;
                    }
                    p.freezeflag = false;
                    let data = self.data[self.players[p.owner].data].clone();
                    if let Some(a) = data.air.get(p.anim_no) {
                        p.anim.step(a);
                    }
                } else {
                    p.hitpause -= 1;
                    p.freezeflag = true;
                }
            }
            self.projs[k] = p;
        }
        self.projs.retain(|p| !p.removed);
    }

    pub fn num_proj(&self, owner_id: i32, id: i32) -> i32 {
        self.projs
            .iter()
            .filter(|p| {
                !p.removed && p.owner_id == owner_id && p.is_active() && (id < 0 || p.id == id)
            })
            .count() as i32
    }
}
