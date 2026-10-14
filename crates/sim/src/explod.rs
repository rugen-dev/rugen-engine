//! Explods: owned animations (sparks, effects) with MUGEN's position spaces.

use rugen_formats::air::Air;

use crate::anim::AnimState;
use crate::world::World;

/// Where an explod's animation comes from.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimSrc {
    /// Index into `World::data`.
    Player(usize),
    /// `data/fightfx` (`F` prefix).
    Fx,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PosType {
    #[default]
    P1,
    P2,
    Front,
    Back,
    Left,
    Right,
    None,
}

impl PosType {
    pub fn parse(s: &str) -> Option<PosType> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "p1" => PosType::P1,
            "p2" => PosType::P2,
            "front" => PosType::Front,
            "back" => PosType::Back,
            "left" => PosType::Left,
            "right" => PosType::Right,
            "none" => PosType::None,
            _ => return None,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Space {
    #[default]
    Stage,
    Screen,
    /// Not given: front / back / left / right mean screen, the rest stage
    /// (Ikemen `Space_none`, resolved by [`World::explod_set_pos`]).
    Unset,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Trans {
    #[default]
    Default,
    None,
    Add,
    Add1,
    Sub,
    AddAlpha,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Explod {
    pub id: i32,
    /// Owning player index.
    pub owner: usize,
    pub owner_id: i32,
    pub src: AnimSrc,
    pub anim: AnimState,
    pub time: i32,
    pub postype: PosType,
    pub space: Space,
    /// Player id the position follows while `bindtime != 0`.
    pub bind_id: i32,
    pub bindtime: i32,
    /// Base position (stage units), set from the bind target or absolute.
    /// In [`Space::Screen`] `pos + offset + rel` is a screen position
    /// instead: stage units at camera zoom 1, x from the screen centre, y
    /// from the top (Ikemen draws screen sprites unzoomed, unshaken).
    pub pos: [f32; 2],
    /// Offset from `pos` (already facing-adjusted).
    pub rel: [f32; 2],
    /// Anchor offset: the screen edge for left / right, the bound
    /// player's position for a screen-space p1 / p2 explod.
    pub offset: [f32; 2],
    pub facing: f32,
    pub vfacing: f32,
    pub scale: [f32; 2],
    pub velocity: [f32; 2],
    pub accel: [f32; 2],
    pub friction: [f32; 2],
    pub removetime: i32,
    pub removeongethit: bool,
    pub removeonchangestate: bool,
    pub sprpriority: i32,
    pub ontop: bool,
    pub under: bool,
    pub trans: Trans,
    pub alpha: [i32; 2],
    pub ignorehitpause: bool,
    pub supermovetime: i32,
    pub pausemovetime: i32,
    pub angle: f32,
    pub animfreeze: bool,
    pub removed: bool,
}

impl Explod {
    pub fn new(owner: usize, owner_id: i32, src: AnimSrc, anim_no: i32) -> Self {
        Self {
            id: -1,
            owner,
            owner_id,
            src,
            anim: AnimState::start(anim_no),
            time: 0,
            postype: PosType::P1,
            space: Space::Unset,
            bind_id: -2,
            bindtime: 1,
            pos: [0.0, 0.0],
            rel: [0.0, 0.0],
            offset: [0.0, 0.0],
            facing: 1.0,
            vfacing: 1.0,
            scale: [1.0, 1.0],
            velocity: [0.0, 0.0],
            accel: [0.0, 0.0],
            friction: [1.0, 1.0],
            removetime: -2,
            removeongethit: false,
            removeonchangestate: false,
            sprpriority: 0,
            ontop: false,
            under: false,
            trans: Trans::Default,
            alpha: [-1, 0],
            ignorehitpause: true,
            supermovetime: 0,
            pausemovetime: 0,
            angle: 0.0,
            animfreeze: false,
            removed: false,
        }
    }

    /// Final draw position in stage space.
    pub fn draw_pos(&self) -> [f32; 2] {
        [
            self.pos[0] + self.offset[0] + self.rel[0],
            self.pos[1] + self.offset[1] + self.rel[1],
        ]
    }

    /// The space this explod is drawn in, `Unset` resolved like Ikemen.
    pub fn resolved_space(&self) -> Space {
        match self.space {
            Space::Unset => match self.postype {
                PosType::Front | PosType::Back | PosType::Left | PosType::Right => Space::Screen,
                _ => Space::Stage,
            },
            s => s,
        }
    }

    /// Drawn in screen space (fixed on screen, unzoomed).
    pub fn on_screen(&self) -> bool {
        self.resolved_space() == Space::Screen
    }

    pub fn air<'a>(&self, w: &'a World) -> Option<&'a Air> {
        match self.src {
            AnimSrc::Player(d) => Some(&w.data[d].air),
            AnimSrc::Fx => w.fx.as_ref().map(|f| &f.air),
        }
    }
}

impl World {
    /// Resolve the explod's base position from its postype (Ikemen `Explod.setPos`).
    ///
    /// Screen-space explods keep screen positions (see [`Explod::pos`]): the
    /// left / right edges are half the stage's screen width from the centre
    /// here, and the renderer moves them out to the window's real edges.
    pub(crate) fn explod_set_pos(&self, e: &mut Explod, c: usize) {
        e.space = e.resolved_space();
        let half = self.stage.width / 2.0;
        let p_pos = |e: &mut Explod, i: usize| {
            let p = &self.players[i];
            e.bind_id = p.id;
            e.facing = p.facing;
            e.rel[0] *= p.facing;
            let s = self.local_scale(i);
            if e.space == Space::Screen {
                // Ikemen: the player's stage position, read as a screen one
                e.offset = [p.pos[0] * s, self.stage.zoffset + p.pos[1] * s];
                e.pos = [0.0, 0.0];
            } else {
                e.pos = [p.pos[0] * s, p.pos[1] * s];
            }
        };
        // left / right: the screen edge, or in stage space where that edge
        // is now (the explod then stays there)
        let edge = |e: &mut Explod, right: bool| {
            e.offset = if e.space == Space::Screen {
                [if right { half } else { -half }, 0.0]
            } else {
                e.pos = [0.0, 0.0];
                let z = self.cam_zoom.max(0.05);
                e.rel[0] /= z;
                [
                    if right {
                        self.screen_right()
                    } else {
                        self.screen_left()
                    },
                    0.0,
                ]
            };
        };
        match e.postype {
            PosType::P1 => p_pos(e, c),
            PosType::P2 => {
                if let Some(p2) = self.nearest_enemy(c) {
                    p_pos(e, p2);
                }
            }
            PosType::Front | PosType::Back => {
                let f = self.players[c].facing;
                if e.postype == PosType::Back {
                    e.facing = f;
                }
                let right = (f > 0.0 && e.postype == PosType::Front)
                    || (f < 0.0 && e.postype == PosType::Back);
                if right {
                    if e.postype == PosType::Back {
                        e.rel[0] = -e.rel[0];
                    }
                    e.postype = PosType::Right;
                } else {
                    e.postype = PosType::Left;
                }
                edge(e, right);
            }
            PosType::Left => edge(e, false),
            PosType::Right => edge(e, true),
            PosType::None => {
                if e.space == Space::Screen {
                    e.offset = [-half, 0.0];
                }
            }
        }
    }

    pub(crate) fn explod_paused(&self, e: &Explod) -> bool {
        let paused = if self.supertime > 0 {
            (e.supermovetime >= 0 && e.time >= e.supermovetime) || e.supermovetime < -2
        } else if self.pausetime > 0 {
            (e.pausemovetime >= 0 && e.time >= e.pausemovetime) || e.pausemovetime < -2
        } else {
            false
        };
        if paused {
            return true;
        }
        if !e.ignorehitpause
            && let Some(p) = self.players.iter().find(|p| p.id == e.owner_id)
            && p.hit_pause_time > 0
        {
            return true;
        }
        false
    }

    /// Per-tick explod update (position, timers, animation, removal).
    pub(crate) fn update_explods(&mut self) {
        let n = self.explods.len();
        for i in 0..n {
            if self.explods[i].removed {
                continue;
            }
            let mut e = self.explods[i].clone();
            self.update_explod(&mut e);
            self.explods[i] = e;
        }
        self.explods.retain(|e| !e.removed);
    }

    /// One explod's tick. Reads the animation in place: the character's AIR
    /// is shared data and must not be copied per explod and frame.
    fn update_explod(&self, e: &mut Explod) {
        let Some(action) = e.air(self).and_then(|air| air.get(e.anim.action)) else {
            e.removed = true;
            return;
        };
        if (e.removetime >= 0 && e.time >= e.removetime) || (e.removetime <= -2 && e.anim.loop_end)
        {
            e.removed = true;
            return;
        }
        if e.removeongethit
            && let Some(p) = self.players.iter().find(|p| p.id == e.owner_id)
            && p.gethit
            && !p.in_guard_state()
        {
            e.removed = true;
            return;
        }
        if self.explod_paused(e) {
            return;
        }
        if e.bindtime != 0
            && e.space == Space::Stage
            && let Some(b) = self.players.iter().position(|p| p.id == e.bind_id)
        {
            let s = self.local_scale(b);
            let p = &self.players[b];
            e.pos = [p.pos[0] * s, p.pos[1] * s];
        }
        e.time += 1;
        if e.bindtime > 0 {
            e.bindtime -= 1;
        }
        e.pos[0] += e.velocity[0] * e.facing;
        e.pos[1] += e.velocity[1];
        for k in 0..2 {
            e.velocity[k] *= e.friction[k];
            e.velocity[k] += e.accel[k];
            if e.velocity[k].abs() < 0.1 && e.friction[k].abs() < 1.0 {
                e.velocity[k] = 0.0;
            }
        }
        if !e.animfreeze {
            e.anim.step(action);
        }
    }

    pub fn remove_explods(&mut self, owner_id: i32, id: i32) {
        for e in &mut self.explods {
            if e.owner_id == owner_id && (id < 0 || e.id == id) {
                e.removed = true;
            }
        }
    }

    pub fn num_explod(&self, owner_id: i32, id: i32) -> i32 {
        self.explods
            .iter()
            .filter(|e| !e.removed && e.owner_id == owner_id && (id < 0 || e.id == id))
            .count() as i32
    }
}
