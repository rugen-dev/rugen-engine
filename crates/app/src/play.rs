//! The match screen: runs a [`World`] (locally or through rollback netplay),
//! draws players, explods, projectiles, the stage and the fight.def HUD.
//!
//! Player keys come from `rugen.toml` (see [`crate::config`]); gamepads work
//! out of the box. Debug keys: Space pause, `.` step, F2 boxes, F1 debug
//! (TRAINING: F1 hitboxes), +/- zoom.

use rugen_sim::scale::FighterSize;
use rugen_sim::team::TeamMode;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rugen_audio::{Audio, BankId, SoundKey};
use rugen_net::{Netplay, Options as NetOptions};
use rugen_render::{Blend, Camera, Draw, PaletteId, Renderer, TextureId, Tint};
use rugen_sim::explod::{AnimSrc, PosType, Trans};
use rugen_sim::fx::PalFxOut;
use rugen_sim::world::Event;
use rugen_sim::{Buttons, CharData, World};
use winit::keyboard::{Key, NamedKey};

use crate::config::Config;

/// How the match is driven.
#[derive(Clone, Debug)]
pub enum NetMode {
    /// Online: our UDP port, the peer, and which side we play (0 = P1, 1 = P2).
    P2P {
        port: u16,
        remote: SocketAddr,
        side: usize,
    },
    /// Online through a `rugen-server` relay session: every datagram goes
    /// through the server (nobody learns the other player's address).
    /// `server` is the base URL, `ticket` comes from the lobby.
    Relay {
        server: String,
        ticket: rugen_net::online::SessionTicket,
        /// Skip the UDP probe and relay over the WebSocket.
        websocket_only: bool,
    },
    /// Local two-player game that rolls back every frame to prove determinism.
    SyncTest,
    /// Watching an online match from the server's input log (a live match,
    /// even mid-match): simulated locally like the players did, with the
    /// online rules (camera, bundled fight.def, the room's rounds).
    Stream(crate::watch::Feed),
}

/// Rules the LAN host sends in the handshake (`HandshakeRules::Send`): the
/// simulation settings that are not part of the match setup, so two players
/// with different `[game]` options still simulate the same match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LanRules {
    pub fighter_size: FighterSize,
    pub movement: rugen_sim::world::Movement,
    /// The match seed (LAN protocol 3); a protocol 2 host sends none.
    pub seed: Option<u64>,
}

impl LanRules {
    /// `fighter_size=normal;air_jumps=1;air_dash=0;fast_fall=1;seed=123`
    /// (no spaces). Protocol 2 peers skip the `seed` they do not know.
    pub fn encode(&self) -> String {
        let m = &self.movement;
        let mut s = format!(
            "fighter_size={};air_jumps={};air_dash={};fast_fall={}",
            self.fighter_size.as_str(),
            m.air_jumps,
            u8::from(m.air_dash),
            u8::from(m.fast_fall)
        );
        if let Some(seed) = self.seed {
            s.push_str(&format!(";seed={seed}"));
        }
        s
    }

    /// `None` unless every field is there (0.6.1 hosts send the size only).
    pub fn decode(s: &str) -> Option<Self> {
        let (mut size, mut jumps, mut dash, mut fall) = (None, None, None, None);
        let mut seed = None;
        for kv in s.split(';') {
            let (k, v) = kv.split_once('=')?;
            let flag = || match v {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
            match k {
                "fighter_size" => size = FighterSize::parse(v),
                "air_jumps" => jumps = v.parse::<i32>().ok().filter(|n| (0..=9).contains(n)),
                "air_dash" => dash = flag(),
                "fast_fall" => fall = flag(),
                "seed" => seed = v.parse::<u64>().ok(),
                _ => {} // a newer host's extra rule
            }
        }
        Some(Self {
            fighter_size: size?,
            movement: rugen_sim::world::Movement {
                air_jumps: jumps?,
                air_dash: dash?,
                fast_fall: fall?,
            },
            seed,
        })
    }
}

/// A fresh seed for an offline match (so arena item drops and everything
/// else seeded differ from one match to the next); replays record it.
pub fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.write_u128(nanos);
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    // never the legacy seed: a fresh match is told apart from an old replay
    match h.finish() {
        0 | rugen_sim::headless::LEGACY_SEED => 2,
        s => s,
    }
}

/// The seed a LAN match plays with: the host's own when the guest can take
/// it (protocol 3), the one the host sent for the guest, and seed 1 when
/// either side is a protocol 2 peer that cannot agree on one.
pub(crate) fn lan_seed(
    side: usize,
    peer_protocol: Option<u32>,
    mine: u64,
    agreed: Option<&LanRules>,
) -> u64 {
    let legacy = rugen_sim::headless::LEGACY_SEED;
    if side == 0 {
        if peer_protocol.is_some_and(|v| v >= rugen_net::LAN_PROTOCOL_SEED) {
            mine
        } else {
            legacy
        }
    } else {
        agreed.and_then(|r| r.seed).unwrap_or(legacy)
    }
}

/// What a LAN peer agreed to after the handshake: the host keeps its own
/// rules (`None`), the guest takes the host's. A peer older than
/// [`rugen_net::LAN_PROTOCOL_MIN`] (0.6.1 and earlier: it would simulate
/// with its own movement settings) is refused.
pub(crate) fn lan_agreement(
    side: usize,
    peer_protocol: Option<u32>,
    rules: Option<&str>,
) -> anyhow::Result<Option<LanRules>> {
    if peer_protocol.is_none_or(|v| v < rugen_net::LAN_PROTOCOL_MIN) {
        anyhow::bail!("the other player runs an older rugen; both players need the same version");
    }
    if side == 0 {
        return Ok(None);
    }
    match rules.and_then(LanRules::decode) {
        Some(r) => Ok(Some(r)),
        None => anyhow::bail!("the host sent no match rules; both players need the same version"),
    }
}

/// Camera limits of a match: the player's own settings offline (and in a
/// local sync test), [`rugen_sim::headless::ONLINE_ZOOM`] in every online
/// session (LAN netplay, relay rooms, quick match, ranked, spectating), since
/// the camera is part of the simulation and must match the peer's and the
/// server's re-simulation.
pub fn match_zoom(net: Option<&NetMode>, cam: &crate::config::CameraCfg) -> (f32, f32) {
    match net {
        None | Some(NetMode::SyncTest) => (cam.zoom_out, cam.zoom_in),
        Some(NetMode::P2P { .. } | NetMode::Relay { .. } | NetMode::Stream(_)) => {
            rugen_sim::headless::ONLINE_ZOOM
        }
    }
}

/// Everything needed to start a match.
#[derive(Clone, Debug)]
pub struct FightSetup {
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    /// Computer level per side (0 = human).
    pub ai: [i32; 2],
    pub net: Option<NetMode>,
    /// Guided tutorial for P1 (P2 stands still, no clock).
    pub tutorial: bool,
    /// Cache manifest ids of both characters (for per-character stats); may be empty.
    pub ids: [String; 2],
    /// Game mode the match was started from (for "character select" after the result).
    pub mode: Option<crate::game::Mode>,
    /// FIGHTER SIZE to simulate with instead of the config's (a replay's
    /// recorded one); LAN matches take the host's from the handshake.
    pub fighter_size: Option<FighterSize>,
    /// Arena AI plugin for CPU fighters instead of the config's (a replay's
    /// recorded value, or `rugen play --arena-ai`). Never on online.
    pub arena_ai: Option<bool>,
    /// Round clock (seconds, <= 0 = no clock) instead of the options' (a
    /// replay's recorded one).
    pub round_time: Option<i32>,
    /// Team modes (offline only): each side's mode and partners; `None` is
    /// a classic 1v1.
    pub teams: Option<TeamSetup>,
    /// CHALLENGE run rules for this fight (offline 1v1 only): `None` plays
    /// the motif's rounds with full life.
    pub run: Option<RunRules>,
    /// The match seed: `None` draws a fresh one offline ([`fresh_seed`]);
    /// replays carry the recorded one, online matches
    /// [`rugen_sim::headless::online_seed`] of the match id. LAN matches
    /// agree on one in the handshake.
    pub seed: Option<u64>,
}

/// What a CHALLENGE run changes in one of its fights. Applied to the world
/// right after it is built (before the first frame), so a replay that
/// records them plays the fight again exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunRules {
    /// Round wins needed (Survival: 1, a draw ends the fight).
    pub match_wins: u8,
    /// P1's life at the start (Survival carries it from fight to fight);
    /// `None` = full.
    #[serde(default)]
    pub p1_life: Option<i32>,
}

impl RunRules {
    /// Set the rules on a freshly built world.
    pub fn apply(&self, world: &mut World) {
        world.round.config.match_wins = self.match_wins.max(1);
        if self.match_wins <= 1 {
            // one round: a draw ends the fight instead of replaying the round
            world.round.config.max_drawgames = 0;
        }
        if let (Some(life), Some(p)) = (self.p1_life, world.players.first_mut()) {
            p.life = life.clamp(1, p.life_max.max(1));
        }
    }
}

/// The teams of an offline team match: `FightSetup::p1` / `p2` are each
/// side's first fighter, `partners` the others in pick order.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeamSetup {
    #[serde(with = "team_modes")]
    pub mode: [TeamMode; 2],
    pub partners: [Vec<PathBuf>; 2],
    /// Cache manifest ids of the partners (per-character stats; may be empty).
    #[serde(default)]
    pub ids: [Vec<String>; 2],
}

impl TeamSetup {
    /// `TURNS · WITH GOKU, KUNG FU MAN`: a side's mode and partners for the
    /// VS screen (empty for a single fighter).
    pub fn line(&self, side: usize) -> String {
        if self.mode[side] == TeamMode::Single {
            return String::new();
        }
        let names: Vec<String> = self.partners[side]
            .iter()
            .map(|p| match rugen_content::manifest_for(p) {
                Some((_, m)) => m.display_name.to_uppercase(),
                None => p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_uppercase())
                    .unwrap_or_default(),
            })
            .collect();
        format!(
            "{} · WITH {}",
            self.mode[side].as_str().to_uppercase(),
            names.join(", ")
        )
    }

    /// `turns:kfm,goku` (a mode, then the partners after the first fighter).
    pub fn parse_side(s: &str) -> Option<(TeamMode, Vec<String>)> {
        let (mode, rest) = s.split_once(':').unwrap_or((s, ""));
        let mode = TeamMode::parse(mode)?;
        let names = rest
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect();
        Some((mode, names))
    }
}

/// Computer level of each side's Simul partners: a computer side's own,
/// COMPUTER LEVEL for a human side's (its partners are computer players).
pub fn partner_ai(side_ai: [i32; 2], ai_level: i32) -> [i32; 2] {
    side_ai.map(|a| if a > 0 { a } else { ai_level.clamp(1, 8) })
}

/// Team modes as their names in replays (`"turns"`).
mod team_modes {
    use rugen_sim::team::TeamMode;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(m: &[TeamMode; 2], s: S) -> Result<S::Ok, S::Error> {
        [m[0].as_str(), m[1].as_str()].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[TeamMode; 2], D::Error> {
        let [a, b] = <[String; 2]>::deserialize(d)?;
        let parse = |v: &str| {
            TeamMode::parse(v)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown team mode {v:?}")))
        };
        Ok([parse(&a)?, parse(&b)?])
    }
}

/// GPU texture for a sprite plus the SFF palette index it was authored with.
type Loaded = (TextureId, Option<usize>);

struct CharGpu {
    textures: HashMap<(u16, u16), Option<Loaded>>,
    shared: crate::sprites::Shared,
    palettes: Vec<PaletteId>,
}

/// One sprite to draw this frame, in stage coordinates.
struct Item {
    spr: Option<(TextureId, Option<PaletteId>, [f32; 2])>,
    pos: [f32; 2],
    facing: f32,
    vfacing: f32,
    scale: [f32; 2],
    frame: rugen_formats::air::Frame,
    boxes: bool,
    layer: i32,
    prio: i32,
    seq: usize,
    blend: Blend,
    angle: f32,
    tint: Tint,
    /// Screen space: `pos` is a screen position (stage units at zoom 1, x
    /// from the centre, y from the top), drawn unzoomed and unshaken; the
    /// value moves it by that many half-widths of the window's extra width
    /// over the stage's (-1 = left anchored, 1 = right, 0 = centred).
    screen: Option<f32>,
    /// What it is, for the `RUGEN_HARNESS_DRAWLOG` dump: kind (P player,
    /// I afterimage, E explod, J projectile), player / explod index, action.
    who: (char, usize, i32),
}

/// Final blend for a sprite: the `Trans` controller wins over the AIR frame.
fn resolve_blend(frame: &rugen_formats::air::Frame, trans: Trans, alpha: [i32; 2]) -> Blend {
    let a = |v: i32, def: u8| if v < 0 { def } else { v.min(255) as u8 };
    match trans {
        Trans::Default => crate::sprites::air_blend(frame),
        Trans::None => Blend::Alpha,
        Trans::Add => Blend::Add { src: 255, dst: 255 },
        Trans::Add1 => Blend::Add { src: 255, dst: 128 },
        Trans::AddAlpha => Blend::Add {
            src: a(alpha[0], 255),
            dst: a(alpha[1], 0),
        },
        Trans::Sub => Blend::Sub { src: 255, dst: 255 },
    }
}

/// Order of two draws at the same layer and sprpriority, from the draw's
/// kind (`who.0`) and its push index: Ikemen queues projectiles, then
/// players, then explods, and draws what was queued later behind.
fn queue_order(kind: char, seq: usize) -> std::cmp::Reverse<(u8, usize)> {
    let group = match kind {
        'J' => 0,
        'P' | 'I' => 1,
        _ => 2,
    };
    std::cmp::Reverse((group, seq))
}

/// PalFX multiply for a subtractive draw (Ikemen `toShader`, `useNeg`).
fn sub_mul(m: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| (m[(i + 1) % 3] + m[(i + 2) % 3]) / 2.0)
}

fn tint(p: PalFxOut) -> Tint {
    Tint {
        mul: p.mul,
        add: p.add,
        color: p.color,
        invert: p.invert,
    }
}

/// The online match menu (Esc / a pad's Select): RESUME or FLEE, with a
/// confirmation. The simulation keeps running under it (an online match
/// cannot pause); the local fighter gets neutral input while it is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FleeMenu {
    /// Focused row: 0 = RESUME / NO, 1 = FLEE / YES.
    pub sel: u8,
    /// Showing the "flee the match?" question.
    pub confirm: bool,
}

/// What a menu input did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Stay,
    Close,
    Flee,
}

impl FleeMenu {
    /// One tick of newly pressed buttons (the local player's).
    pub fn input(&mut self, p: Buttons) -> MenuAction {
        if p.u || p.d {
            self.sel ^= 1;
        }
        if p.b {
            if self.confirm {
                *self = FleeMenu::default();
                return MenuAction::Stay;
            }
            return MenuAction::Close;
        }
        if p.a || p.c || p.x || p.s {
            return match (self.confirm, self.sel) {
                (false, 0) => MenuAction::Close,
                (false, _) => {
                    // FLEE asks first, focused on NO
                    *self = FleeMenu {
                        sel: 0,
                        confirm: true,
                    };
                    MenuAction::Stay
                }
                (true, 0) => {
                    *self = FleeMenu::default();
                    MenuAction::Stay
                }
                (true, _) => MenuAction::Flee,
            };
        }
        MenuAction::Stay
    }
}

/// An online match that ended before its last round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarlyEnd {
    /// We fled (forfeit): a loss.
    Fled,
    /// The other player fled, quit or lost its connection: a win.
    PeerGone(rugen_net::PeerExit),
}

impl EarlyEnd {
    /// The `ending` a result report carries.
    pub fn ending(self) -> rugen_proto::Ending {
        match self {
            EarlyEnd::PeerGone(rugen_net::PeerExit::Lost) => rugen_proto::Ending::Disconnect,
            _ => rugen_proto::Ending::Flee,
        }
    }

    /// The line the winner reads.
    pub fn message(self) -> &'static str {
        match self {
            EarlyEnd::Fled => "YOU FLED THE MATCH",
            EarlyEnd::PeerGone(rugen_net::PeerExit::Lost) => "OPPONENT DISCONNECTED",
            EarlyEnd::PeerGone(_) => "YOUR OPPONENT FLED",
        }
    }
}

/// Ticks the "opponent fled" victory stays up (skippable after a second).
const EARLY_LINGER: u32 = 300;

/// Sound banks of one fight: each loaded fighter, the common sounds and the
/// announcer own their samples, so nothing cached by an earlier fight (or an
/// earlier version of a hot-reloaded fighter) answers this fight's PlaySnd.
#[derive(Debug)]
pub struct SoundBanks {
    common: BankId,
    announcer: BankId,
    /// One per `world.data` slot.
    chars: Vec<BankId>,
}

impl SoundBanks {
    pub fn new() -> Self {
        Self {
            common: rugen_audio::new_bank(),
            announcer: rugen_audio::new_bank(),
            chars: Vec::new(),
        }
    }

    /// The bank of data slot `d` (fresh on first use).
    fn char_bank(&mut self, d: usize) -> BankId {
        while self.chars.len() <= d {
            self.chars.push(rugen_audio::new_bank());
        }
        self.chars[d]
    }

    /// Slot `d` now holds different data (hot reload).
    fn renew(&mut self, d: usize) {
        self.char_bank(d);
        self.chars[d] = rugen_audio::new_bank();
    }
}

impl Default for SoundBanks {
    fn default() -> Self {
        Self::new()
    }
}

/// Which bank and bytes a sound event refers to: the announcer's fight.snd,
/// the player's own `.snd` (`own`, PlaySnd without `F`), or common.snd.
pub fn resolve_sound<'w>(
    world: &'w World,
    banks: &mut SoundBanks,
    player: usize,
    own: bool,
    announcer: bool,
    group: i32,
    number: i32,
) -> (SoundKey, Option<&'w [u8]>) {
    let (bank, bytes) = if announcer {
        (
            banks.announcer,
            world
                .fx
                .as_ref()
                .and_then(|f| f.fight_snd.as_ref())
                .and_then(|s| s.get(group, number)),
        )
    } else if own {
        let d = world.players[player].data;
        (
            banks.char_bank(d),
            world.data[d]
                .snd
                .as_ref()
                .and_then(|s| s.get(group, number)),
        )
    } else {
        (
            banks.common,
            world
                .fx
                .as_ref()
                .and_then(|f| f.snd.as_ref())
                .and_then(|s| s.get(group, number)),
        )
    };
    (
        SoundKey {
            bank,
            group,
            number,
        },
        bytes,
    )
}

pub struct Match {
    pub world: World,
    /// Who owns which sounds (see [`SoundBanks`]).
    banks: SoundBanks,
    /// Draw the fight without its HUD (the pause menu's frozen backdrop).
    pub hud_hidden: bool,
    /// `rugen debug`: the fighter debugger (history, selection, panel data).
    pub debugger: Option<crate::debugger::Debugger>,
    /// Online menu over the running match (see [`FleeMenu`]).
    pub flee_menu: Option<FleeMenu>,
    /// Online: the match ended early, and ticks since.
    early: Option<(EarlyEnd, u32)>,
    /// Online: the match is ranked (the flee question says what it costs).
    pub ranked: Option<bool>,
    /// One per `world.data`, plus one for fightfx at the end (filled by `init_gpu`).
    chars: Vec<CharGpu>,
    net: Option<Netplay>,
    net_status: String,
    /// Eases rollback corrections on screen (display only).
    smooth: crate::smooth::Smoother,
    paused: bool,
    step: bool,
    zoom: f32,
    pub show_boxes: bool,
    debug: bool,
    /// Announcement text and ticks left to show it (fallback when fight.def is missing).
    banner: Option<(String, i32)>,
    stage_visual: Option<crate::stage::StageVisual>,
    lifebar: Option<crate::lifebar::Lifebar>,
    /// Ticks since the match ended.
    over_ticks: i32,
    /// Start pressed after the match ended.
    pub leave: bool,
    /// Inputs recorded this match (local play only).
    pub record: Option<crate::replay::Replay>,
    /// Inputs to replay instead of live input, with a cursor.
    pub playback: Option<(Vec<[u16; 2]>, usize)>,
    /// Watching from the server's input log: the feed, the next frame, and
    /// whether the picture started (after the start buffer).
    stream: Option<(crate::watch::Feed, usize, bool)>,
    /// Frames simulated without drawing this frame (catching up).
    catching: bool,
    /// F3: state / frame data / input history overlay.
    overlay: bool,
    input_history: [Vec<Buttons>; 2],
    setup: FightSetup,
    data_dir: PathBuf,
    /// Hot reload: data slot and what to watch for it.
    watch: Vec<(usize, Watch)>,
    reload_tick: u32,
    /// Data slots whose textures must be re-uploaded (after a reload).
    gpu_dirty: Vec<usize>,
    /// Stage music file, if any.
    pub bgm: Option<PathBuf>,
    pub tutorial: Option<crate::tutorial::Tutorial>,
    /// What this player presses for START (hints in the fight overlays).
    pub start_label: String,
    /// What each side presses for TAG (Tag teams: the HUD hint).
    pub tag_labels: [String; 2],
    /// Arena hints: what each side presses for X (jump off a ledge) and B
    /// (pick up / throw an item).
    pub arena_caps: [[String; 2]; 2],
    /// Profile stats already written for this match.
    pub stats_recorded: bool,
    /// TRAINING tools (hitboxes, ...): drawing and the dummy's inputs only.
    pub training: Option<crate::training::Training>,
    /// A CHALLENGE run's strip over the fight (left, right), set by the shell.
    pub run_hud: Option<(String, String)>,
}

impl Match {
    pub fn load(
        setup: &FightSetup,
        motif: &Path,
        data_dir: &Path,
        cfg: &Config,
    ) -> anyhow::Result<Self> {
        let visual = setup
            .stage
            .as_ref()
            .and_then(|p| match crate::stage::StageVisual::load(p) {
                Ok(v) => Some(v),
                Err(e) => {
                    log::warn!("stage background not loaded: {e:#}");
                    None
                }
            });
        // a replay's recorded size, else the config's (online: the room's rules)
        let mut fighter_size = setup.fighter_size.unwrap_or(cfg.game.fighter_size);
        let mut movement = rugen_sim::world::Movement {
            air_jumps: cfg.game.air_jumps,
            air_dash: cfg.game.air_dash,
            fast_fall: cfg.game.fast_fall,
        };
        let mut seed = match &setup.net {
            None | Some(NetMode::SyncTest) => setup.seed.unwrap_or_else(fresh_seed),
            // set from the match id; a setup without one plays the legacy seed
            Some(NetMode::Relay { .. } | NetMode::Stream(_)) => {
                setup.seed.unwrap_or(rugen_sim::headless::LEGACY_SEED)
            }
            Some(NetMode::P2P { .. }) => setup.seed.unwrap_or_else(fresh_seed),
        };
        let net = match &setup.net {
            Some(NetMode::P2P { port, remote, side }) => {
                // LAN / explicit IP: wait for the peer, measure the round trip
                let mut socket = rugen_net::Socket::bind(*port)
                    .with_context(|| format!("netplay on port {port}"))?;
                // the host's FIGHTER SIZE and movement ride the handshake:
                // both sides simulate with them
                let mine = LanRules {
                    fighter_size,
                    movement,
                    seed: Some(seed),
                }
                .encode();
                let rules = if *side == 0 {
                    rugen_net::HandshakeRules::Send(&mine)
                } else {
                    rugen_net::HandshakeRules::Receive
                };
                let shake = socket
                    .handshake_with(
                        *remote,
                        std::time::Duration::from_secs(60),
                        rules,
                        &mut |s| log::info!("netplay: {s}"),
                    )
                    .with_context(|| format!("netplay with {remote}"))?;
                let agreed = lan_agreement(*side, shake.peer_protocol, shake.rules.as_deref())
                    .with_context(|| format!("netplay with {remote}"))?;
                seed = lan_seed(*side, shake.peer_protocol, seed, agreed.as_ref());
                if let Some(r) = agreed {
                    fighter_size = r.fighter_size;
                    movement = r.movement;
                }
                let p95 = shake.rtt.map(|r| r.p95).or(shake.rtt_ms);
                let policy = cfg.net.delay_policy();
                let o = NetOptions {
                    input_delay: cfg.net.input_delay.frames(p95, &policy),
                    max_prediction: policy.prediction_window,
                    ..Default::default()
                };
                log::info!(
                    "netplay: port {port}, playing P{}, rtt {:?} ms (p95 {:?}), input delay {}",
                    side + 1,
                    shake.rtt_ms,
                    p95,
                    o.input_delay
                );
                Some(Netplay::p2p_with_socket(socket, *remote, *side, &o)?)
            }
            Some(NetMode::Relay {
                server,
                ticket,
                websocket_only,
            }) => Some(relay_session(server, ticket, *websocket_only, cfg)?.0),
            Some(NetMode::SyncTest) => Some(Netplay::synctest(7, &NetOptions::default())?),
            Some(NetMode::Stream(_)) | None => None,
        };
        // round timings come from fight.def: a user theme's only offline
        let sim_motif = crate::themes::sim_motif(
            motif,
            matches!(
                setup.net,
                Some(NetMode::P2P { .. } | NetMode::Relay { .. } | NetMode::Stream(_))
            ),
        );
        let spec = rugen_sim::headless::MatchSpec {
            p1: &setup.p1,
            p2: &setup.p2,
            stage: setup.stage.as_deref(),
            data_dir,
            motif: &sim_motif,
            zoom: match_zoom(setup.net.as_ref(), &cfg.camera),
            movement,
            // a replay's recorded clock; offline: the options' round time;
            // online rooms: the room's (both sides run `match_config`); LAN
            // peers keep the motif's
            round_time: setup.round_time.or_else(|| {
                (setup.net.is_none() || cfg.game.match_wins.is_some())
                    .then_some(cfg.game.round_time)
            }),
            ai: if setup.net.is_none() {
                setup.ai
            } else {
                [0, 0]
            },
            fighter_size,
            seed,
        };
        // team modes are offline only: online stays 1v1
        let teams = setup.teams.as_ref().filter(|_| setup.net.is_none());
        let mut world = match teams {
            None => rugen_sim::headless::build_world(&spec),
            Some(t) => {
                let ts = rugen_sim::headless::TeamSpec {
                    mode: t.mode,
                    partners: [0, 1].map(|s| t.partners[s].iter().map(PathBuf::as_path).collect()),
                    partner_ai: partner_ai(setup.ai, cfg.game.ai_level),
                };
                rugen_sim::headless::build_team_world(&spec, &ts)
            }
        }
        .with_context(|| format!("{} vs {}", setup.p1.display(), setup.p2.display()))?;
        // the room's ROUNDS (online; the same on both sides and in the server's re-simulation)
        if let Some(w) = cfg.game.match_wins.filter(|_| setup.net.is_some()) {
            world.round.config.match_wins = w;
        }
        // a CHALLENGE run's fight: its rounds and P1's carried life
        if let Some(rules) = setup.run.filter(|_| setup.net.is_none() && teams.is_none()) {
            rules.apply(&mut world);
        }
        // a match rule for CPU sides; online rooms are human vs human
        world.arena_ai = setup.net.is_none() && setup.arena_ai.unwrap_or(cfg.game.arena_ai);
        for d in &world.data {
            for w in &d.warnings {
                log::debug!("{}: {w}", d.name);
            }
            // Opus sounds of .rgc fighters decode in the background, ready before they play
            if let Some(snd) = &d.snd {
                snd.prefetch();
            }
        }
        if setup.tutorial {
            world.round.time_left = -1;
        }
        log::info!("{} vs {}", world.data[0].name, world.data[1].name);

        // stage music: paths are relative to the game root (`sound/x.mp3`), else the stage dir
        let bgm = setup.stage.as_ref().and_then(|p| {
            let def = rugen_formats::def::Def::open(p).ok()?;
            let name = def.section("music")?.raw("bgmusic")?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let root = data_dir.parent().unwrap_or(data_dir);
            let stage_dir = p.parent().unwrap_or(Path::new("."));
            [root.join(&name), stage_dir.join(&name)]
                .into_iter()
                .find(|c| rugen_formats::vfs::exists(c))
        });
        let lifebar = match crate::lifebar::Lifebar::load(motif, data_dir) {
            Ok(lb) => Some(lb),
            Err(e) => {
                log::warn!("fight.def HUD not loaded: {e:#}");
                None
            }
        };
        Ok(Self {
            world,
            banks: SoundBanks::new(),
            flee_menu: None,
            early: None,
            ranked: None,
            chars: Vec::new(),
            net,
            net_status: String::new(),
            smooth: Default::default(),
            paused: false,
            step: false,
            zoom: 1.0,
            show_boxes: false,
            hud_hidden: false,
            debugger: None,
            debug: true,
            banner: None,
            stage_visual: visual,
            lifebar,
            over_ticks: 0,
            leave: false,
            record: None,
            playback: None,
            stream: match &setup.net {
                Some(NetMode::Stream(f)) => Some((f.clone(), 0, false)),
                _ => None,
            },
            catching: false,
            overlay: std::env::var_os("RUGEN_OVERLAY").is_some(),
            input_history: [Vec::new(), Vec::new()],
            watch: [(0usize, &setup.p1), (1, &setup.p2)]
                .into_iter()
                .filter(|(i, _)| *i == 0 || setup.p2 != setup.p1)
                .filter_map(|(i, p)| Watch::of(p).map(|w| (i, w)))
                .collect(),
            setup: setup.clone(),
            data_dir: data_dir.to_path_buf(),
            reload_tick: 0,
            gpu_dirty: Vec::new(),
            bgm,
            tutorial: None,
            start_label: "START".into(),
            tag_labels: ["TAG".into(), "TAG".into()],
            arena_caps: [["X".into(), "B".into()], ["X".into(), "B".into()]],
            stats_recorded: false,
            training: None,
            run_hud: None,
        })
    }

    /// An arena fight (`[Arena]` stage rules or ARENA mode): the HUD shows
    /// fighter cards along the bottom instead of the long life bars.
    pub fn is_arena(&self) -> bool {
        let a = &self.world.stage.arena;
        self.setup.mode == Some(crate::game::Mode::Arena) || a.stocks > 0 || a.blastzone.is_some()
    }

    pub fn is_tutorial(&self) -> bool {
        self.tutorial.is_some()
    }

    pub fn setup_ids(&self) -> [String; 2] {
        self.setup.ids.clone()
    }

    /// Re-upload palettes/textures for data slots touched by a hot reload.
    pub fn refresh_gpu(&mut self, r: &mut Renderer) {
        if self.chars.is_empty() {
            self.init_gpu(r);
            return;
        }
        for i in std::mem::take(&mut self.gpu_dirty) {
            let Some(d) = self.world.data.get(i) else {
                continue;
            };
            let mut palettes: Vec<PaletteId> =
                d.sff.palettes.iter().map(|p| r.add_palette(p)).collect();
            if palettes.is_empty() {
                palettes.push(r.add_palette(&[[0; 4]; 256]));
            }
            if let Some(cg) = self.chars.get_mut(i) {
                cg.textures.clear();
                cg.shared.clear();
                cg.palettes = palettes;
            }
        }
    }

    /// Reload a character whose source files changed on disk (creator workflow).
    fn check_reload(&mut self) {
        self.reload_tick += 1;
        if self.reload_tick % 60 != 0 || self.net.is_some() {
            return;
        }
        for k in 0..self.watch.len() {
            let slot = self.watch[k].0;
            let def = if slot == 0 {
                &self.setup.p1
            } else {
                &self.setup.p2
            };
            let def = def.clone();
            let changed = match &mut self.watch[k].1 {
                // a folder in chars/: its working copy is converted again first
                Watch::WorkingCopy => {
                    match rugen_content::refresh_working_copy(&def, &self.data_dir) {
                        Some(Ok(changed)) => changed,
                        Some(Err(e)) => {
                            log::error!("reload failed: {e}");
                            false
                        }
                        None => false,
                    }
                }
                Watch::Folder(dir, seen) => {
                    let now = newest_mtime(dir);
                    let changed = now > *seen;
                    *seen = now;
                    changed
                }
            };
            if !changed {
                continue;
            }
            match CharData::load_with_data(&def, &self.data_dir) {
                Ok(d) => {
                    for w in &d.warnings {
                        log::debug!("{}: {w}", d.name);
                    }
                    log::info!("reloaded {}", d.name);
                    // both slots share data when the same character fights itself
                    for (i, slot_data) in self.world.data.iter_mut().enumerate() {
                        if i == slot || (self.setup.p1 == self.setup.p2 && i < 2) {
                            *slot_data = d.clone();
                            self.gpu_dirty.push(i);
                            self.banks.renew(i);
                        }
                    }
                }
                Err(e) => log::error!("reload failed: {e}"),
            }
        }
    }

    /// Upload palettes once the renderer exists.
    pub fn init_gpu(&mut self, r: &mut Renderer) {
        if !self.chars.is_empty() {
            return;
        }
        let mut sffs: Vec<&rugen_formats::sff::Sff> =
            self.world.data.iter().map(|d| &d.sff).collect();
        if let Some(fx) = &self.world.fx {
            sffs.push(&fx.sff);
        }
        self.chars = sffs
            .iter()
            .map(|sff| {
                let mut palettes: Vec<PaletteId> =
                    sff.palettes.iter().map(|p| r.add_palette(p)).collect();
                if palettes.is_empty() {
                    palettes.push(r.add_palette(&[[0; 4]; 256]));
                }
                CharGpu {
                    textures: HashMap::new(),
                    shared: Default::default(),
                    palettes,
                }
            })
            .collect();
        if let Some(sv) = self.stage_visual.as_mut() {
            sv.init_gpu(r);
        }
    }

    /// Advance one tick with both players' inputs. Returns false when the
    /// frame did not advance (paused, or netplay waiting on the peer).
    pub fn tick(&mut self, inputs: [Buttons; 2], audio: &mut Option<Audio>) -> bool {
        if self.paused && !self.step {
            return false;
        }
        // the debugger's slow motion: only every nth tick moves (a STEP always)
        if !self.step && self.debugger.as_mut().is_some_and(|d| !d.gate()) {
            return false;
        }
        self.step = false;
        // the other player left: the victory over the frozen fight, then out
        if let Some((_, t)) = self.early.as_mut() {
            *t += 1;
            let skip = inputs.iter().any(|b| b.a || b.s);
            if *t > EARLY_LINGER || (*t > 60 && skip) {
                self.leave = true;
            }
            return true;
        }
        self.check_reload();
        let mut inputs = inputs;
        if let Some((feed, cursor, started)) = self.stream.as_mut() {
            match feed.frame(*cursor) {
                Some(f) => {
                    inputs = [Buttons::from_bits(f[0]), Buttons::from_bits(f[1])];
                    *cursor += 1;
                    *started = true;
                }
                None if feed.status().1 => {
                    self.leave = true;
                    return false;
                }
                None => return false,
            }
        }
        if let Some((frames, cursor)) = self.playback.as_mut() {
            match frames.get(*cursor) {
                Some(f) => {
                    inputs = [Buttons::from_bits(f[0]), Buttons::from_bits(f[1])];
                    *cursor += 1;
                }
                None => self.leave = true,
            }
        }
        // TRAINING: the dummy's inputs (recording, playback, guard); never
        // while a replay plays (it holds what this produced)
        if self.playback.is_none()
            && self.net.is_none()
            && let Some(t) = self.training.as_mut()
            && let Some(view) = crate::training::DummyView::of(&self.world)
        {
            inputs = t.inputs_for(inputs, view);
        }
        // TAG belongs to offline Tag teams: never on the wire
        if self.net.is_some() {
            for b in &mut inputs {
                b.t = false;
            }
        }
        if let Some(rec) = self.record.as_mut() {
            rec.inputs.push([inputs[0].bits(), inputs[1].bits()]);
        }
        for (h, b) in self.input_history.iter_mut().zip(inputs) {
            h.push(b);
            if h.len() > 24 {
                h.remove(0);
            }
        }
        let advanced = match self.net.as_mut() {
            Some(net) => {
                net.poll();
                let shown = crate::smooth::positions(&self.world);
                match net.advance(&mut self.world, inputs) {
                    Ok(adv) => {
                        let corrected = adv.corrected.as_deref().map(crate::smooth::positions);
                        if adv.new_frames > 0 || corrected.is_some() {
                            self.smooth.tick(&shown, corrected.as_deref());
                        }
                        for n in &adv.notices {
                            // once per episode: a stall says "waiting for
                            // remote" every tick until it ends
                            if *n != self.net_status {
                                log::info!("netplay: {n}");
                            }
                            self.net_status = n.clone();
                        }
                        let st = net.stats();
                        if st.synchronized && adv.notices.is_empty() {
                            self.net_status = format!(
                                "{} ping {} ms  delay {}  rollbacks {}  ahead {}",
                                st.path.label(),
                                st.ping_ms,
                                st.input_delay,
                                st.rollbacks,
                                st.frames_ahead
                            );
                        }
                        let moved = adv.new_frames > 0;
                        let gone = net.peer_exit();
                        self.handle_events(adv.events, audio);
                        if let Some(e) = gone {
                            if self.world.round.match_over {
                                // it left after the end: nothing to wait for
                                self.over_ticks = self.over_ticks.max(150);
                                self.leave = true;
                            } else if self.can_flee() {
                                log::info!("netplay: the other player is gone ({e:?})");
                                self.flee_menu = None;
                                self.early = Some((EarlyEnd::PeerGone(e), 0));
                                self.net_status.clear();
                            }
                        }
                        moved
                    }
                    Err(e) => {
                        log::error!("netplay: {e}");
                        self.net_status = format!("netplay error: {e}");
                        self.net = None;
                        false
                    }
                }
            }
            None => {
                // a panicking character is disabled instead of taking the match down
                if let Some(culprit) = self.world.step_checked(&inputs) {
                    log::error!(
                        "P{} crashed the simulation and was disabled; see `rugen verify`",
                        culprit + 1
                    );
                }
                let ev = self.world.events.clone();
                self.handle_events(ev, audio);
                true
            }
        };
        if !advanced {
            return false;
        }
        if let Some(d) = self.debugger.as_mut() {
            d.record(&self.world);
        }
        // the training lab measures what the step did (reads the world only)
        if let Some(t) = self.training.as_mut() {
            t.observe(&self.world);
        }
        if let Some(sv) = self.stage_visual.as_mut() {
            sv.step();
        }
        if let Some(lb) = self.lifebar.as_mut() {
            lb.step();
        }
        if let Some((_, t)) = self.banner.as_mut() {
            *t -= 1;
        }
        if self.banner.as_ref().is_some_and(|(_, t)| *t <= 0) {
            self.banner = None;
        }
        // harness: `RUGEN_HARNESS_LOWLIFE=1` makes P2 drop in one hit so matches end fast
        if std::env::var_os("RUGEN_HARNESS_LOWLIFE").is_some()
            && self.world.round.round_state() == 2
            && let Some(p2) = self.world.side_leader(1)
            && self.world.players[p2].life > 1
        {
            self.world.players[p2].life = 1;
        }
        harness_states(&mut self.world);
        if self.world.round.match_over {
            self.over_ticks += 1;
            // linger on the win pose, then hand over to the result screen
            if self.over_ticks > 150 || (self.over_ticks > 30 && (inputs[0].s || inputs[1].s)) {
                self.leave = true;
            }
        }
        if let Some(t) = self.tutorial.as_mut() {
            if self.world.round.time_left >= 0 {
                self.world.round.time_left = -1;
            }
            t.tick(&self.world);
        }
        // content warnings (a character asking for a missing action, ...):
        // debug level, a normal session log stays readable
        for w in self.world.warnings.drain(..) {
            log::debug!("{w}");
        }
        true
    }

    /// A player pressed START (not a fighter button): skips a tutorial
    /// step / leaves a finished tutorial, skips the win pose and the
    /// early-end linger.
    /// Returns true when START was used up (it ended a training recording).
    pub fn start_pressed(&mut self) -> bool {
        if self.training.as_mut().is_some_and(|t| t.stop_recording()) {
            return true;
        }
        if let Some(t) = self.tutorial.as_mut() {
            if t.finished {
                self.leave = true;
            } else {
                t.skip();
            }
        }
        if self.world.round.match_over && self.over_ticks > 30 {
            self.leave = true;
        }
        if self.early.as_ref().is_some_and(|(_, t)| *t > 60) {
            self.leave = true;
        }
        false
    }

    pub fn setup(&self) -> &FightSetup {
        &self.setup
    }

    pub fn is_net(&self) -> bool {
        self.net.is_some()
    }

    /// Watching a match from the server's input log.
    pub fn is_watch(&self) -> bool {
        self.stream.is_some()
    }

    /// Watching: frames to simulate this drawn frame ([`crate::watch::steps`]);
    /// 1 otherwise.
    pub fn stream_steps(&mut self) -> usize {
        let Some((feed, cursor, started)) = self.stream.as_ref() else {
            return 1;
        };
        let (have, complete) = feed.status();
        let n = crate::watch::steps(*cursor, have, complete, *started);
        self.catching = n > 1;
        // the stream is over and fully played: the end screen as usual
        if complete && *cursor >= have {
            return 1;
        }
        n
    }

    /// We play one side of a netplay match against a remote player (not a
    /// spectator, not a local sync test).
    pub fn is_netplay_seat(&self) -> bool {
        self.net
            .as_ref()
            .is_some_and(|n| !n.is_synctest() && !n.is_spectator())
    }

    /// The online menu may open / a flee may happen: a netplay seat in a
    /// match still being fought.
    pub fn can_flee(&self) -> bool {
        self.is_netplay_seat() && !self.world.round.match_over && self.early.is_none()
    }

    /// Esc / pad Select in a netplay match: open or close the menu.
    pub fn toggle_flee_menu(&mut self) {
        self.flee_menu = match self.flee_menu {
            None if self.can_flee() => Some(FleeMenu::default()),
            _ => None,
        };
    }

    /// Newly pressed buttons of the local player while the menu is open.
    pub fn flee_menu_input(&mut self, pressed: Buttons) {
        let Some(menu) = self.flee_menu.as_mut() else {
            return;
        };
        match menu.input(pressed) {
            MenuAction::Stay => {}
            MenuAction::Close => self.flee_menu = None,
            MenuAction::Flee => self.flee(),
        }
    }

    /// Forfeit: tell the other player (every path), then leave at once.
    pub fn flee(&mut self) {
        if !self.can_flee() {
            return;
        }
        if let Some(net) = self.net.as_mut() {
            net.flee();
        }
        log::info!("netplay: fled the match at frame {}", self.world.frame);
        self.flee_menu = None;
        self.early = Some((EarlyEnd::Fled, 0));
        self.leave = true;
    }

    /// How the match ended early, if it did.
    pub fn early_end(&self) -> Option<EarlyEnd> {
        self.early.map(|(e, _)| e)
    }

    /// Length of the next tick (online: stretched while ahead of the peer).
    pub fn tick_duration(&self) -> std::time::Duration {
        self.net
            .as_ref()
            .map_or(rugen_net::TICK, Netplay::tick_duration)
    }

    /// What a result report carries: last frame, world checksum and both
    /// players' inputs (the server compares the two reports).
    pub fn evidence(&self) -> rugen_net::online::Evidence {
        let turn = self.net.as_ref().and_then(|n| n.turn_stats());
        rugen_net::online::Evidence {
            frames: self.world.frame,
            checksum: Some(self.world.checksum()),
            inputs: self
                .net
                .as_ref()
                .map(|n| n.input_log_bytes())
                .unwrap_or_default(),
            checksums: self
                .net
                .as_ref()
                .map(|n| n.checksum_log().to_vec())
                .unwrap_or_default(),
            compact: turn.is_some(),
            turn_bytes: turn
                .filter(|s| s.tx_packets > 0)
                .map(|s| (s.rx_wire, s.tx_wire)),
            ending: self.early.map(|(e, _)| e.ending()),
            rounds: Some(self.world.round.wins),
        }
    }

    /// The online menu (RESUME / FLEE, then the question) or the victory
    /// over a fight the other player left. fight.def fonts when it has
    /// them, else the built-in 3x5 font.
    fn draw_flee_overlay(
        &self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        screen_w: f32,
        screen_h: f32,
        mut lifebar: Option<&mut crate::lifebar::Lifebar>,
    ) {
        let won = match self.early {
            Some((e @ EarlyEnd::PeerGone(_), _)) => Some(e),
            _ => None,
        };
        if self.flee_menu.is_none() && won.is_none() {
            return;
        }
        let cx = screen_w / 2.0;
        let (w, h) = (210.0, if won.is_some() { 76.0 } else { 92.0 });
        let top = screen_h * 0.5 - h / 2.0;
        // one line centred on `y` (HUD units), shrunk to fit the panel
        let mut text = |r: &mut Renderer, line: &str, y: f32, kind: usize, col: [f32; 4]| {
            // (fight.def font, bank, 3x5 pixel size) per kind: title, row, small
            let (index, bank, px) = [(4, 0, 2.0), (3, 1, 1.4), (1, 5, 1.0)][kind];
            let room = w - 16.0;
            if let Some(lb) = lifebar.as_deref_mut()
                && let Some([tw, th]) = lb.text_size(index, line)
            {
                let k = if tw > room { room / tw } else { 1.0 };
                lb.text_at(r, index, line, hud(cx, y + th * k / 2.0), bank, 0, u * k);
                return;
            }
            let tw = line.chars().count() as f32 * 4.0 * px;
            let px = if tw > room { px * room / tw } else { px };
            draw_text(r, line, hud(cx, y - 2.5 * px), px * u, col);
        };
        // dim the fight, an ink panel with a hard red shadow
        r.rect(hud(0.0, 0.0), hud(screen_w, screen_h), [0.0, 0.0, 0.0, 0.5]);
        r.rect(
            hud(cx - w / 2.0 + 5.0, top + 5.0),
            hud(cx + w / 2.0 + 5.0, top + h + 5.0),
            [0.86, 0.12, 0.12, 1.0],
        );
        r.rect(
            hud(cx - w / 2.0, top),
            hud(cx + w / 2.0, top + h),
            [0.04, 0.04, 0.05, 0.96],
        );
        r.rect_outline(
            hud(cx - w / 2.0, top),
            hud(cx + w / 2.0, top + h),
            1.5 * u,
            [0.95, 0.94, 0.89, 1.0],
        );
        let white = [1.0, 1.0, 1.0, 1.0];
        let grey = [0.7, 0.7, 0.72, 1.0];
        let red = [1.0, 0.3, 0.25, 1.0];
        if let Some(e) = won {
            text(r, "YOU WIN", top + 20.0, 0, [1.0, 0.85, 0.3, 1.0]);
            text(r, e.message(), top + 42.0, 1, white);
            if self.early.is_some_and(|(_, t)| t > 60) {
                text(
                    r,
                    &format!("PRESS {}", self.start_label),
                    top + 62.0,
                    2,
                    grey,
                );
            }
            return;
        }
        let Some(menu) = self.flee_menu else { return };
        let (title, rows, note) = if menu.confirm {
            let note = match self.ranked {
                Some(true) => "RANKED: YOU LOSE RATING",
                Some(false) => "CASUAL: RECORDED AS A FLEE",
                None => "IT COUNTS AS A LOSS",
            };
            ("FLEE THE MATCH?", ["NO", "YES, FLEE"], note)
        } else {
            ("MATCH MENU", ["RESUME", "FLEE"], "THE FIGHT GOES ON")
        };
        text(r, title, top + 15.0, 1, white);
        r.rect(
            hud(cx - w / 2.0 + 10.0, top + 26.0),
            hud(cx + w / 2.0 - 10.0, top + 27.0),
            [0.95, 0.94, 0.89, 0.5],
        );
        for (i, row) in rows.iter().enumerate() {
            let y = top + 41.0 + i as f32 * 18.0;
            let on = menu.sel as usize == i;
            if on {
                r.rect(
                    hud(cx - 64.0, y - 8.0),
                    hud(cx + 64.0, y + 8.0),
                    [0.86, 0.12, 0.12, 1.0],
                );
            }
            text(r, row, y, 1, if on { white } else { grey });
        }
        text(
            r,
            note,
            top + h - 11.0,
            2,
            if menu.confirm { red } else { grey },
        );
    }

    /// Watching: a LIVE tag with both players over the fight, "CATCHING
    /// UP" with the progress while fast-forwarding, a wait line while the
    /// players are ahead of the stream.
    fn draw_watch_overlay(
        &self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        screen_w: f32,
        screen_h: f32,
        mut lifebar: Option<&mut crate::lifebar::Lifebar>,
    ) {
        let Some((feed, cursor, started)) = self.stream.as_ref() else {
            return;
        };
        let (have, complete) = feed.status();
        let cx = screen_w / 2.0;
        let names = feed
            .header()
            .map(|h| {
                format!(
                    "{} VS {}",
                    h.players[0].name.to_uppercase(),
                    h.players[1].name.to_uppercase()
                )
            })
            .unwrap_or_default();
        // the tag under the HUD: LIVE (a running match) or REPLAY
        let tag = if complete && !self.catching {
            format!("REPLAY · {names}")
        } else {
            let behind = have.saturating_sub(*cursor) as f32 / 60.0;
            format!("● LIVE · {names} · {:.0} S BEHIND", behind.max(0.0))
        };
        // a red strip under the clock, in the HUD's small font, kept narrow
        let y = 46.0;
        let raw = lifebar
            .as_deref_mut()
            .and_then(|lb| lb.text_size(1, &tag))
            .map(|[w, _]| w);
        let k = raw.map_or(1.0, |w| (130.0 / w.max(1.0)).min(1.0));
        let tw = raw.map_or(tag.chars().count() as f32 * 4.0, |w| w * k) + 12.0;
        r.rect(
            hud(cx - tw / 2.0, y - 5.0),
            hud(cx + tw / 2.0, y + 5.0),
            [0.78, 0.06, 0.1, 0.92],
        );
        match lifebar.as_deref_mut() {
            Some(lb) if raw.is_some() => {
                let th = lb.text_size(1, &tag).map_or(8.0, |[_, h]| h) * k;
                lb.text_at(r, 1, &tag, hud(cx, y + th / 2.0), 0, 0, u * k);
            }
            _ => draw_text(r, &tag, hud(cx, y - 2.5), u, [1.0, 1.0, 1.0, 1.0]),
        }
        let mut text = |r: &mut Renderer, line: &str, y: f32, kind: usize, col: [f32; 4]| {
            let (index, bank, px) = [(4, 0, 2.0), (3, 1, 1.4), (1, 5, 1.0)][kind];
            if let Some(lb) = lifebar.as_deref_mut()
                && let Some([_, th]) = lb.text_size(index, line)
            {
                lb.text_at(r, index, line, hud(cx, y + th / 2.0), bank, 0, u);
                return;
            }
            draw_text(r, line, hud(cx, y - 2.5 * px), px * u, col);
        };
        let waiting = !*started || (have <= *cursor && !complete);
        if !self.catching && !waiting {
            return;
        }
        let (w, h) = (210.0, 54.0);
        let top = screen_h * 0.5 - h / 2.0;
        r.rect(
            hud(0.0, 0.0),
            hud(screen_w, screen_h),
            [0.0, 0.0, 0.0, 0.45],
        );
        r.rect(
            hud(cx - w / 2.0 + 5.0, top + 5.0),
            hud(cx + w / 2.0 + 5.0, top + h + 5.0),
            [0.86, 0.12, 0.12, 1.0],
        );
        r.rect(
            hud(cx - w / 2.0, top),
            hud(cx + w / 2.0, top + h),
            [0.04, 0.04, 0.05, 0.96],
        );
        let white = [1.0, 1.0, 1.0, 1.0];
        if self.catching {
            text(r, "CATCHING UP...", top + 18.0, 1, white);
            let frac = (*cursor as f32 / have.max(1) as f32).clamp(0.0, 1.0);
            r.rect(
                hud(cx - 80.0, top + 34.0),
                hud(cx + 80.0, top + 40.0),
                [0.25, 0.25, 0.28, 1.0],
            );
            r.rect(
                hud(cx - 80.0, top + 34.0),
                hud(cx - 80.0 + 160.0 * frac, top + 40.0),
                [0.86, 0.12, 0.12, 1.0],
            );
        } else {
            text(r, "WAITING FOR THE PLAYERS...", top + 22.0, 1, white);
        }
    }

    /// Tiny connection indicator for online matches: 1-4 bars from the
    /// ping and a warning while the connection is unstable.
    fn draw_net_quality(
        &self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        screen_w: f32,
    ) {
        let Some(net) = self.net.as_ref().filter(|n| !n.is_synctest()) else {
            return;
        };
        let st = net.stats();
        if !st.synchronized {
            return;
        }
        let bars = st.bars();
        let color = match bars {
            4 => [0.3, 0.9, 0.4, 0.9],
            3 => [0.7, 0.9, 0.3, 0.9],
            2 => [1.0, 0.7, 0.2, 0.9],
            _ => [1.0, 0.3, 0.2, 0.9],
        };
        let x0 = screen_w - 26.0;
        // top right, under the name plate (clear of power gauges)
        let y1 = 46.0;
        for i in 0..4u8 {
            let h = 2.0 + f32::from(i) * 1.5;
            let x = x0 + f32::from(i) * 5.0;
            let c = if i < bars {
                color
            } else {
                [0.2, 0.2, 0.2, 0.6]
            };
            r.rect(hud(x, y1 - h), hud(x + 3.5, y1), c);
        }
        // connection type, ping and input delay (e.g. "DIRECT 182 MS D1")
        let label = format!("{} {} MS D{}", st.path.label(), st.ping_ms, st.input_delay);
        draw_text(
            r,
            &label,
            hud(screen_w - 30.0 - label.len() as f32 * 2.0, 43.0),
            u,
            [0.9, 0.9, 0.9, 0.8],
        );
        if st.unstable {
            draw_text(
                r,
                "CONNECTION UNSTABLE",
                hud(screen_w / 2.0, 56.0),
                1.5 * u,
                [1.0, 0.5, 0.3, 0.95],
            );
        }
    }

    /// Winning side (0/1) once the match is over, None for a draw or unfinished match.
    pub fn winner(&self) -> Option<usize> {
        if let (Some(e), Some(net)) = (self.early_end(), self.net.as_ref()) {
            let me = net.local_handle().min(1);
            return Some(if e == EarlyEnd::Fled { 1 - me } else { me });
        }
        if self.world.teams.active() {
            return self.world.match_winner();
        }
        let w = self.world.round.wins;
        if !self.world.round.match_over || w[0] == w[1] {
            None
        } else {
            Some(usize::from(w[1] > w[0]))
        }
    }

    /// `rugen debug`: trace the simulation and keep the frames for STEP
    /// BACK; the fight's own HUD makes room for the debugger's panel.
    pub fn attach_debugger(&mut self, data_dir: &Path) {
        self.debugger = Some(crate::debugger::Debugger::new(&mut self.world, data_dir));
        self.hud_hidden = true;
    }

    /// A debugger button or key (`play`, `step`, `back`, `speed`, `boxes`,
    /// `next`). True when it changed something.
    pub fn debug_action(&mut self, a: &str) -> bool {
        let Some(d) = self.debugger.as_mut() else {
            return false;
        };
        match a {
            "play" => self.paused = !self.paused,
            "step" => {
                self.paused = true;
                self.step = true;
            }
            "back" => {
                self.paused = true;
                match d.back() {
                    Some(w) => self.world = w,
                    None => return false,
                }
            }
            "speed" => d.cycle_speed(),
            "boxes" => self.show_boxes = !self.show_boxes,
            "next" => d.select_next(&self.world),
            _ => return false,
        }
        log::debug!("debugger: {a} at frame {}", self.world.frame);
        true
    }

    /// Freeze the match (the debugger's PAUSE).
    pub fn hold(&mut self) {
        self.paused = true;
    }

    /// The debugger panel's data (`ui/debug.toml`).
    pub fn debug_data(&mut self) -> Option<serde_json::Value> {
        let (paused, boxes) = (self.paused, self.show_boxes);
        let d = self.debugger.as_mut()?;
        Some(d.data(&self.world, paused, boxes))
    }

    /// Where player `i` stands on screen (drawing coordinates of
    /// [`render`](Self::render)): its feet and the top of its hurt boxes.
    fn player_view(&self, i: usize) -> Option<([f32; 2], f32, f32)> {
        let p = self.world.players.get(i).filter(|p| !p.removed)?;
        let scale = self.world.data_scale(p.anim_owner);
        let ground = self.world.stage.zoffset;
        let cam_y = self.world.cam_y + self.world.envshake.offset[1];
        let feet = [p.pos[0] * scale, ground + p.pos[1] * scale - cam_y];
        let frame = self.world.data[p.anim_owner]
            .air
            .get(p.anim.action)
            .and_then(|a| p.anim.frame(a));
        let boxes = frame.map(|f| f.clsn2.clone()).unwrap_or_default();
        let (top, half) = if boxes.is_empty() {
            (-60.0 * scale, 20.0 * scale)
        } else {
            let top = boxes.iter().map(|b| b[1].min(b[3])).min().unwrap_or(-60) as f32;
            let half = boxes
                .iter()
                .map(|b| b[0].abs().max(b[2].abs()))
                .max()
                .unwrap_or(20) as f32;
            (top * scale, half * scale)
        };
        Some((feet, feet[1] + top, half.max(8.0 * scale)))
    }

    /// The debugger's pick: the player under `at` (drawing coordinates),
    /// the smallest one when they overlap, else the nearest within reach.
    pub fn debug_pick(&mut self, at: [f32; 2]) -> bool {
        let mut best: Option<(f32, usize)> = None;
        for i in 0..self.world.players.len() {
            let Some((feet, top, half)) = self.player_view(i) else {
                continue;
            };
            let inside = (at[0] - feet[0]).abs() <= half && at[1] <= feet[1] + 4.0 && at[1] >= top;
            let mid = [feet[0], (feet[1] + top) / 2.0];
            let (dx, dy) = (at[0] - mid[0], at[1] - mid[1]);
            let dist = libm::sqrtf(dx * dx + dy * dy);
            let score = if inside { half } else { 1000.0 + dist };
            if dist < 120.0 && best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        match (best, self.debugger.as_mut()) {
            (Some((_, i)), Some(d)) => {
                d.selected = i;
                true
            }
            _ => false,
        }
    }

    /// Debug keys. Returns true when consumed.
    pub fn key(&mut self, key: &Key, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        if self.debugger.is_some() {
            let a = match key.as_ref() {
                Key::Character(",") => "back",
                Key::Named(NamedKey::Tab) => "next",
                Key::Named(NamedKey::F4) => "speed",
                Key::Character(".") => "step",
                _ => "",
            };
            if !a.is_empty() {
                self.debug_action(a);
                return true;
            }
        }
        match key.as_ref() {
            // a netplay match cannot pause (the peer would stall)
            Key::Named(NamedKey::Space) | Key::Character(".") if self.net.is_some() => {
                return false;
            }
            Key::Named(NamedKey::Space) => self.paused = !self.paused,
            Key::Character(".") => self.step = true,
            Key::Named(NamedKey::F2) => self.show_boxes = !self.show_boxes,
            // TRAINING: F1 is the HITBOXES switch of the pause menu
            Key::Named(NamedKey::F1) if self.training.is_some() => {
                if let Some(t) = self.training.as_mut() {
                    t.cfg.hitboxes = !t.cfg.hitboxes;
                }
            }
            Key::Named(NamedKey::F1) => self.debug = !self.debug,
            Key::Named(NamedKey::F3) => self.overlay = !self.overlay,
            Key::Character("+") | Key::Character("=") => self.zoom = (self.zoom * 1.25).min(8.0),
            Key::Character("-") => self.zoom = (self.zoom / 1.25).max(0.25),
            _ => return false,
        }
        true
    }

    pub fn debug_title(&self) -> Option<String> {
        if !self.debug {
            return None;
        }
        let p = &self.world.players[0];
        let d = &self.world.data[p.data];
        let anim = self.world.data[p.anim_owner].air.get(p.anim.action);
        Some(format!(
            "rugen — {} — state {} t={} anim {} elem {} animtime {} pos ({:.1},{:.1}) vel ({:.2},{:.2}) ctrl {} life {}/{}{}",
            d.name,
            p.state_no,
            p.state_time,
            p.anim.action,
            p.anim.elem + 1,
            anim.map_or(0, |a| p.anim.anim_time(a)),
            p.pos[0],
            p.pos[1],
            p.vel[0],
            p.vel[1],
            p.ctrl as u8,
            p.life,
            self.world.players[1].life,
            if self.paused { " [paused]" } else { "" }
        ))
    }

    /// Play one sound event through the audio crate.
    #[allow(clippy::too_many_arguments)]
    fn play_sound(
        &mut self,
        audio: &mut Option<Audio>,
        player: usize,
        own: bool,
        group: i32,
        number: i32,
        channel: i32,
        volume: i32,
        announcer: bool,
    ) {
        let Some(audio) = audio.as_mut() else {
            return;
        };
        let (key, bytes) = resolve_sound(
            &self.world,
            &mut self.banks,
            player,
            own,
            announcer,
            group,
            number,
        );
        if !audio.is_loaded(key) {
            match bytes {
                Some(b) => {
                    audio.load(key, b);
                }
                None => return,
            }
        }
        let root = self.world.players[player].root;
        let pan = ((self.world.players[player].pos[0] - self.world.cam_x)
            / (self.world.stage.width / 2.0))
            .clamp(-1.0, 1.0)
            * 0.5;
        audio.play(key, root, channel, volume, pan, false);
    }

    pub fn handle_events(&mut self, events: Vec<Event>, audio: &mut Option<Audio>) {
        if let Some(lb) = self.lifebar.as_mut() {
            for e in &events {
                lb.event(&self.world, e);
            }
        }
        for e in events {
            match e {
                Event::Sound {
                    player,
                    own,
                    group,
                    number,
                    channel,
                    volume,
                } => self.play_sound(audio, player, own, group, number, channel, volume, false),
                Event::StopSound { player, channel } => {
                    if let Some(a) = audio.as_mut() {
                        a.stop_channel(player, channel);
                    }
                }
                Event::RoundStart(n) => {
                    self.banner = Some((format!("ROUND {n}"), 90));
                    self.play_sound(audio, 0, false, 0, n, -1, 100, true);
                }
                Event::Fight => {
                    self.banner = Some(("FIGHT".into(), 45));
                    self.play_sound(audio, 0, false, 1, 0, -1, 100, true);
                }
                Event::KO => {
                    self.banner = Some(("K.O.".into(), 90));
                    self.play_sound(audio, 0, false, 2, 0, -1, 100, true);
                }
                Event::TimeOver => {
                    self.banner = Some(("TIME OVER".into(), 90));
                    self.play_sound(audio, 0, false, 2, 2, -1, 100, true);
                }
                Event::RoundWon(_) => {}
                Event::MatchOver(t) => {
                    let name = if t >= 0 {
                        self.world.data[self.world.players[t as usize].data]
                            .display_name
                            .clone()
                    } else {
                        "DRAW".into()
                    };
                    self.banner = Some((format!("{name} WINS"), 100_000));
                }
                Event::Spark { .. } => {}
            }
        }
        if let Some(a) = audio.as_mut() {
            a.gc();
        }
    }

    /// GPU sprite from a data slot (`None` = fightfx).
    fn sprite_from(
        &mut self,
        r: &mut Renderer,
        src: Option<usize>,
        group: i32,
        number: i32,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2])> {
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let (sff, slot) = match src {
            Some(d) => (&self.world.data[d].sff, d),
            None => (&self.world.fx.as_ref()?.sff, self.world.data.len()),
        };
        let cg = self.chars.get_mut(slot)?;
        let spr = sff.get(key.0, key.1)?;
        let shared = &mut cg.shared;
        let entry = cg
            .textures
            .entry(key)
            .or_insert_with(|| Some(crate::sprites::upload_sprite(r, spr, shared)));
        let (tex, pal) = (*entry)?;
        let pal = pal.map(|p| cg.palettes[p.min(cg.palettes.len() - 1)]);
        Some((tex, pal, [spr.offset[0] as f32, spr.offset[1] as f32]))
    }

    fn collect_items(&mut self, r: &mut Renderer) -> Vec<Item> {
        let all_pfx = self.world.allpalfx.sample();
        let mut items: Vec<Item> = Vec::new();
        for i in 0..self.world.players.len() {
            let p = &self.world.players[i];
            if p.removed {
                continue;
            }
            let scale = self.world.data_scale(p.anim_owner);
            let d = &self.world.data[p.anim_owner];
            let Some(frame) = d
                .air
                .get(p.anim.action)
                .and_then(|a| p.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let sprite_data = p.data;
            let invisible = p.asserted(rugen_sim::player::assert::INVISIBLE);
            let p_action = p.anim.action;
            // rollback corrections ease in on screen (display only)
            let off = self.smooth.offset(crate::smooth::Key::Player(p.id));
            let (pos, facing, prio) = (
                [(p.pos[0] + off[0]) * scale, (p.pos[1] + off[1]) * scale],
                p.facing,
                p.spr_priority,
            );
            let blend = resolve_blend(&frame, p.trans, p.alpha);
            let angle = if p.angle_drawn {
                p.angle * p.facing
            } else {
                0.0
            };
            // AngleDraw scale times the player's [Size] xscale / yscale (explods never take it)
            let ascale = [
                p.angle_scale[0] * p.size.xscale,
                p.angle_scale[1] * p.size.yscale,
            ];
            let pfx = tint(self.world.drawn_palfx(i).sample().stacked(all_pfx));
            // afterimages first so they sit behind the player
            let aimg = p
                .afterimage
                .as_ref()
                .map(|ai| (ai.visible(p.spr_priority), ai.trans, ai.alpha));
            if let Some((visible, ai_trans, ai_alpha)) = aimg {
                for (img, img_pfx, img_prio) in visible {
                    let d = &self.world.data[img.anim_owner];
                    let Some(f) = d
                        .air
                        .get(img.action)
                        .and_then(|a| a.frames.get(img.elem))
                        .cloned()
                    else {
                        continue;
                    };
                    let spr = self.sprite_from(r, Some(img.sprite_data), f.group, f.number);
                    let blend = resolve_blend(&f, ai_trans, ai_alpha);
                    items.push(Item {
                        spr,
                        pos: [img.pos[0] * scale, img.pos[1] * scale],
                        facing: img.facing,
                        vfacing: 1.0,
                        scale: [scale * img.scale[0], scale * img.scale[1]],
                        frame: f,
                        boxes: false,
                        layer: 0,
                        prio: img_prio,
                        seq: items.len(),
                        blend,
                        angle: img.angle * img.facing,
                        tint: tint(img_pfx.stacked(all_pfx)),
                        screen: None,
                        who: ('I', i, img.action),
                    });
                }
            }
            // AssertSpecial invisible hides the body; its afterimages stay (Ikemen)
            if invisible {
                continue;
            }
            let spr = self.sprite_from(r, Some(sprite_data), frame.group, frame.number);
            items.push(Item {
                spr,
                pos,
                facing,
                vfacing: 1.0,
                scale: [scale * ascale[0], scale * ascale[1]],
                frame,
                boxes: true,
                layer: 0,
                prio,
                seq: items.len(),
                blend,
                angle,
                tint: pfx,
                screen: None,
                who: ('P', i, p_action),
            });
        }
        let explods: Vec<rugen_sim::explod::Explod> = self
            .world
            .explods
            .iter()
            .filter(|e| !e.removed)
            .cloned()
            .collect();
        for e in explods {
            // screen-space explods: the owner's localcoord without its
            // FIGHTER SIZE, like their positions (World::screen_scale)
            let screen = e.on_screen().then_some(match e.postype {
                PosType::Left | PosType::None => -1.0,
                PosType::Right => 1.0,
                _ => 0.0,
            });
            let (src, scale) = match (e.src, screen.is_some()) {
                (AnimSrc::Player(d), false) => (Some(d), self.world.data_scale(d)),
                (AnimSrc::Player(d), true) => (Some(d), self.world.screen_scale(d)),
                (AnimSrc::Fx, false) => (None, self.world.fx_scale(e.owner)),
                (AnimSrc::Fx, true) => (
                    None,
                    self.world.stage.width
                        / self.world.fx.as_ref().map_or(320.0, |f| f.localcoord[0]),
                ),
            };
            let Some(frame) = e
                .air(&self.world)
                .and_then(|a| a.get(e.anim.action))
                .and_then(|a| e.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let spr = self.sprite_from(r, src, frame.group, frame.number);
            let blend = resolve_blend(&frame, e.trans, e.alpha);
            // ownpal: the explod's own PalFX; else its owner's, as it is now
            // (MUGEN: "affected by subsequent execution of the player's PalFX")
            let owner_pfx = if e.ownpal {
                e.palfx.sample()
            } else if e.owner < self.world.players.len() {
                self.world.drawn_palfx(e.owner).sample()
            } else {
                PalFxOut::default()
            };
            items.push(Item {
                spr,
                pos: e.draw_pos(),
                facing: e.facing,
                vfacing: e.vfacing,
                scale: [scale * e.scale[0], scale * e.scale[1]],
                frame,
                boxes: false,
                layer: if e.ontop {
                    1
                } else if e.under {
                    -1
                } else {
                    0
                },
                prio: e.sprpriority,
                seq: items.len(),
                blend,
                angle: e.angle * e.facing,
                tint: tint(owner_pfx.stacked(all_pfx)),
                screen,
                who: ('E', e.owner, e.anim.action),
            });
        }
        let keys = crate::smooth::proj_keys(&self.world);
        let projs: Vec<(rugen_sim::projectile::Projectile, crate::smooth::Key)> = self
            .world
            .projs
            .iter()
            .zip(keys)
            .filter(|(p, _)| !p.removed && p.anim_no >= 0)
            .map(|(p, k)| (p.clone(), k))
            .collect();
        for (pr, key) in projs {
            let off = self.smooth.offset(key);
            let d = self.world.players[pr.owner].data;
            let scale = self.world.data_scale(d);
            let Some(frame) = self.world.data[d]
                .air
                .get(pr.anim_no)
                .and_then(|a| pr.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let spr = self.sprite_from(r, Some(d), frame.group, frame.number);
            let blend = resolve_blend(&frame, Trans::Default, [255, 0]);
            let owner_pfx = self.world.drawn_palfx(pr.owner).sample();
            items.push(Item {
                spr,
                pos: [pr.pos[0] + off[0], pr.pos[1] + off[1]],
                facing: pr.facing,
                vfacing: 1.0,
                scale: [scale * pr.scale[0], scale * pr.scale[1]],
                frame,
                boxes: true,
                layer: 0,
                prio: pr.sprpriority,
                seq: items.len(),
                blend,
                angle: 0.0,
                tint: tint(owner_pfx.stacked(all_pfx)),
                screen: None,
                who: ('J', pr.owner, pr.anim_no),
            });
        }
        // equal sprpriority: what is queued later is drawn behind (Ikemen
        // `DrawList.draw`), queued as projectiles, players (afterimages
        // before their player), then explods. Two copies of one sprite at the
        // same priority, an additive glow over a subtractive shade (Goku's
        // ki blasts), stack as their author saw them
        items.sort_by_key(|it| (it.layer, it.prio, queue_order(it.who.0, it.seq)));
        // subtractive draws use PalFX's "negative" multiply (Ikemen
        // `toShader` under TT_sub): each channel takes the mean of the other
        // two, so a palfx that keeps blue subtracts red
        for it in items.iter_mut() {
            if matches!(it.blend, Blend::Sub { .. }) {
                it.tint.mul = sub_mul(it.tint.mul);
            }
        }
        // harness: `RUGEN_HARNESS_DRAWLOG=120` dumps what frame 120 draws
        // (`120-180`: every frame of the range)
        let logged = std::env::var("RUGEN_HARNESS_DRAWLOG").ok().and_then(|v| {
            let (a, b) = v.split_once('-').unwrap_or((&v, &v));
            Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
        });
        if logged.is_some_and(|(a, b)| (a..=b).contains(&self.world.frame)) {
            for it in &items {
                log::info!(
                    "draw f{} {}{} action {} spr {},{} pos {:?} scale {:?} screen {:?} blend {:?} tint {:?} prio {} layer {}",
                    self.world.frame,
                    it.who.0,
                    it.who.1,
                    it.who.2,
                    it.frame.group,
                    it.frame.number,
                    it.pos,
                    it.scale,
                    it.screen,
                    it.blend,
                    it.tint,
                    it.prio,
                    it.layer
                );
            }
        }
        items
    }

    /// F3 overlay: state and frame data per player plus recent inputs.
    fn draw_overlay(
        &self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        screen_w: f32,
    ) {
        let w = &self.world;
        let roots: Vec<usize> = w
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
            .map(|(i, _)| i)
            .take(2)
            .collect();
        for (side, &pi) in roots.iter().enumerate() {
            let p = &w.players[pi];
            let d = &w.data[p.anim_owner];
            let anim = d.air.get(p.anim.action);
            let lines = [
                format!(
                    "P{} ST {} T {} {:?} {:?} CTRL {}",
                    side + 1,
                    p.state_no,
                    p.state_time,
                    p.state_type,
                    p.move_type,
                    p.ctrl as u8
                ),
                format!(
                    "ANIM {} EL {}/{} AT {}",
                    p.anim.action,
                    p.anim.elem + 1,
                    anim.map_or(0, |a| a.frames.len()),
                    anim.map_or(0, |a| p.anim.anim_time(a))
                ),
                format!(
                    "POS {:.1} {:.1} VEL {:.2} {:.2}",
                    p.pos[0], p.pos[1], p.vel[0], p.vel[1]
                ),
                format!(
                    "LIFE {} POW {} HIT {} JUG {}",
                    p.life, p.power, p.hit_count, p.juggle
                ),
            ];
            let x = if side == 0 { 6.0 } else { screen_w - 6.0 };
            for (k, line) in lines.iter().enumerate() {
                let y = 40.0 + k as f32 * 7.0;
                let wpx = line.len() as f32 * 4.0;
                let cx = if side == 0 {
                    x + wpx / 2.0
                } else {
                    x - wpx / 2.0
                };
                draw_text(r, line, hud(cx, y), u, [1.0, 1.0, 0.6, 0.95]);
            }
            // input history, newest at the bottom
            let hist = &self.input_history[side];
            for (k, b) in hist.iter().rev().take(12).enumerate() {
                let mut s = String::new();
                for (on, ch) in [
                    (b.u, 'U'),
                    (b.d, 'D'),
                    (b.l, 'L'),
                    (b.r, 'R'),
                    (b.a, 'A'),
                    (b.b, 'B'),
                    (b.c, 'C'),
                    (b.x, 'X'),
                    (b.y, 'Y'),
                    (b.z, 'Z'),
                    (b.s, 'S'),
                ] {
                    s.push(if on { ch } else { '.' });
                }
                let y = 160.0 - k as f32 * 6.0;
                let wpx = s.len() as f32 * 4.0;
                let cx = if side == 0 {
                    x + wpx / 2.0
                } else {
                    x - wpx / 2.0
                };
                let alpha = 1.0 - k as f32 * 0.07;
                draw_text(r, &s, hud(cx, y), u, [0.8, 0.9, 1.0, alpha]);
            }
        }
    }

    /// The debugger's selected player: a bar over its head and its name.
    fn draw_debug_marker(&self, r: &mut Renderer, u: f32) {
        let Some(d) = self.debugger.as_ref() else {
            return;
        };
        let Some((feet, top, half)) = self.player_view(d.selected) else {
            return;
        };
        let yellow = [1.0, 0.82, 0.25, 1.0];
        let y = top - 6.0 * u;
        r.rect(
            [feet[0] - half, y - 2.0 * u],
            [feet[0] + half, y + 1.0 * u],
            yellow,
        );
        let label = format!(
            "SELECTED - {}",
            crate::debugger::Debugger::who(&self.world, d.selected)
        );
        let w = label.chars().count() as f32 * 4.0 * u + 6.0 * u;
        let (x0, y0) = (feet[0] - w / 2.0, y - 13.0 * u);
        r.rect([x0, y0], [x0 + w, y0 + 9.0 * u], [0.08, 0.07, 0.06, 0.92]);
        draw_text(r, &label, [feet[0], y0 + 2.0 * u], u, yellow);
    }

    /// Queue this frame's draws; the shell calls `begin` before and `end` after.
    pub fn render(&mut self, r: &mut Renderer) -> (Camera, [f64; 4]) {
        let stage = self.world.stage.clone();
        let ground = stage.zoffset;
        let items = self.collect_items(r);
        let show_boxes = self.show_boxes || self.training.as_ref().is_some_and(|t| t.cfg.hitboxes);
        let brightness = self.world.superbrightness;
        let cam_x = self.world.cam_x;

        let shake = self.world.envshake.offset;
        let cam_x = cam_x + shake[0];
        let cam_y = self.world.cam_y + shake[1];
        let zoom = self.world.cam_zoom * self.zoom;
        let bg_tint = tint(self.world.bgpalfx.sample());
        // every root player's 9000,0 (team partners get small faces)
        let portraits: Vec<_> = (0..self.world.players.len())
            .map(|i| {
                let (root, data) = (
                    self.world.players[i].helper_index == 0,
                    self.world.players[i].data,
                );
                root.then(|| self.sprite_from(r, Some(data), 9000, 0))
                    .flatten()
            })
            .collect();
        let mut stage_visual = self.stage_visual.take();
        let mut lifebar = self.lifebar.take();
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let screen_w = stage.height * aspect;
        // zooming in keeps the ground line where it is; zooming out lifts it
        // toward two thirds of the screen so the wider view shows floor as well
        // as sky. HUD coordinates are "screen" units mapped back into the
        // zoomed world so bars never scale with the camera
        let ground_frac = ground_screen_frac(ground / stage.height, zoom);
        let center_y = ground - (ground_frac - 0.5) * stage.height / zoom;
        let u = 1.0 / zoom;
        let hud_left = cam_x - screen_w * u / 2.0;
        let hud_w = screen_w * u;
        let hud_top = center_y - stage.height * u / 2.0;
        let hud = |sx: f32, sy: f32| [hud_left + sx * u, hud_top + sy * u];
        if let Some(sv) = stage_visual.as_mut() {
            sv.set_platform_offsets(
                rugen_sim::hazards::platform_views(&self.world)
                    .iter()
                    .map(|v| (v.platform, v.offset))
                    .collect(),
            );
        }
        // AssertSpecial nobg / nofg / nobardisplay from any fighter (a super's
        // cut-in hides the stage and the bars)
        let asserted = |f: u32| {
            self.world
                .players
                .iter()
                .any(|p| !p.removed && p.asserted(f))
        };
        let (no_bg, no_fg, no_bars) = (
            asserted(rugen_sim::player::assert::NOBG),
            asserted(rugen_sim::player::assert::NOFG),
            asserted(rugen_sim::player::assert::NOBARDISPLAY),
        );
        match stage_visual.as_mut().filter(|_| !no_bg) {
            Some(sv) => sv.draw(
                r,
                0,
                [cam_x, cam_y],
                stage.width / sv.localcoord[0],
                screen_w * u,
                bg_tint,
            ),
            None if no_bg => r.rect([-8000.0, -8000.0], [8000.0, 8000.0], [0.0, 0.0, 0.0, 1.0]),
            None => r.rect(
                [-4000.0, ground],
                [4000.0, ground + 1.0],
                [0.35, 0.35, 0.35, 1.0],
            ),
        }
        // screen space (Ikemen draws it with camera scale 1 and no shake):
        // into world units around the unshaken view, `u` world units per
        // screen unit
        let mut items = items;
        let screen_left = self.world.cam_x - screen_w * u / 2.0;
        let extra_half = (screen_w - stage.width) / 2.0;
        for it in items.iter_mut() {
            if let Some(anchor) = it.screen {
                it.pos = [
                    screen_left + (screen_w / 2.0 + it.pos[0] + anchor * extra_half) * u,
                    hud_top + it.pos[1] * u - ground + cam_y,
                ];
                it.scale = [it.scale[0] * u, it.scale[1] * u];
            }
        }
        for it in &items {
            let frame = &it.frame;
            let origin = [it.pos[0], ground + it.pos[1] - cam_y];
            if let Some((tex, pal, axis)) = it.spr {
                let mut d = Draw::new(
                    tex,
                    pal,
                    [
                        origin[0] + frame.x as f32 * it.scale[0] * it.facing,
                        origin[1] + frame.y as f32 * it.scale[1] * it.vfacing,
                    ],
                    axis,
                );
                d.hflip = (it.facing < 0.0) != frame.hflip;
                d.vflip = (it.vfacing < 0.0) != frame.vflip;
                d.scale = [frame.xscale * it.scale[0], frame.yscale * it.scale[1]];
                d.blend = it.blend;
                d.angle = it.angle;
                d.tint = it.tint;
                if it.angle != 0.0 {
                    // rotate about the player's axis, not the sprite's
                    let (dx, dy) = (d.pos[0] - origin[0], d.pos[1] - origin[1]);
                    let rad = it.angle * (std::f32::consts::PI / 180.0);
                    let (sn, cs) = (libm::sinf(rad), libm::cosf(rad));
                    d.pos = [origin[0] + dx * cs + dy * sn, origin[1] - dx * sn + dy * cs];
                }
                r.draw(d);
            }
            if show_boxes && it.boxes {
                for pl in stage.platforms.iter().filter(|p| !p.gone) {
                    r.rect(
                        [pl.x1, ground + pl.y - cam_y - 1.0],
                        [pl.x2, ground + pl.y - cam_y + 1.0],
                        [0.2, 1.0, 0.4, 0.9],
                    );
                }
                for (boxes, color) in [
                    (&frame.clsn2, [0.2, 0.4, 1.0, 0.9]),
                    (&frame.clsn1, [1.0, 0.2, 0.2, 0.9]),
                ] {
                    for b in boxes {
                        let (x1, x2) = (
                            b[0] as f32 * it.scale[0] * it.facing,
                            b[2] as f32 * it.scale[0] * it.facing,
                        );
                        let min = [
                            origin[0] + x1.min(x2),
                            origin[1] + b[1] as f32 * it.scale[1],
                        ];
                        let max = [
                            origin[0] + x1.max(x2),
                            origin[1] + b[3] as f32 * it.scale[1],
                        ];
                        r.rect_outline(min, max, 1.0, color);
                        r.rect(min, max, [color[0], color[1], color[2], 0.15]);
                    }
                }
            }
        }
        self.draw_debug_marker(r, u);
        self.draw_hazards(r, ground, cam_y, cam_x, screen_w * u);
        self.draw_arena_items(r, ground, cam_y);
        if let Some(sv) = stage_visual.as_mut().filter(|_| !no_bg && !no_fg) {
            sv.draw(
                r,
                1,
                [cam_x, cam_y],
                stage.width / sv.localcoord[0],
                screen_w * u,
                bg_tint,
            );
        }
        if self.overlay {
            self.draw_overlay(r, &hud, u, screen_w);
        }
        if let Some(t) = self.training.as_ref().filter(|_| !self.hud_hidden) {
            t.draw(
                r,
                &crate::training::Hud {
                    at: &hud,
                    u,
                    screen_w,
                    screen_h: stage.height,
                },
            );
        }
        if let Some(t) = self.tutorial.as_ref() {
            let lines = t.text();
            // screenpack fonts when the fight.def has them, else the built-in
            // 3x5 font; lines stack by their real height so a big title
            // never runs into the hint below it
            let fonts = [(4, 0), (3, 1), (1, 5)];
            let builtin = [
                (2.0, [1.0, 0.9, 0.3, 1.0]),
                (1.2, [1.0, 1.0, 1.0, 1.0]),
                (1.0, [0.7, 0.8, 1.0, 1.0]),
            ];
            let ink = |k: usize| {
                lifebar
                    .as_ref()
                    .and_then(|lb| lb.text_ink(fonts[k].0, &lines[k]))
                    .unwrap_or_else(|| {
                        // the built-in font draws down from the given top
                        let px = builtin[k].0;
                        [lines[k].chars().count() as f32 * 4.0 * px, 0.0, 5.0 * px]
                    })
            };
            let inks = [ink(0), ink(1), ink(2)];
            let layout = tutorial_layout(&inks);
            let half = layout.width / 2.0;
            r.rect(
                hud(screen_w / 2.0 - half, layout.top),
                hud(screen_w / 2.0 + half, layout.bottom),
                [0.0, 0.0, 0.0, 0.55],
            );
            for (k, (index, bank)) in fonts.iter().enumerate() {
                let base = layout.baselines[k];
                let drawn = lifebar.as_mut().is_some_and(|lb| {
                    lb.text_at(r, *index, &lines[k], hud(screen_w / 2.0, base), *bank, 0, u)
                });
                if !drawn {
                    let (px, col) = builtin[k];
                    draw_text(r, &lines[k], hud(screen_w / 2.0, base), px * u, col);
                }
            }
        }
        self.draw_flee_overlay(r, &hud, u, screen_w, stage.height, lifebar.as_mut());
        self.draw_watch_overlay(r, &hud, u, screen_w, stage.height, lifebar.as_mut());
        if brightness < 1.0 {
            r.rect(
                [-4000.0, -4000.0],
                [4000.0, 4000.0],
                [0.0, 0.0, 0.0, 1.0 - brightness],
            );
        }
        if self.hud_hidden || no_bars {
        } else if let Some(lb) = lifebar.as_mut() {
            // fight.def space (usually 320x240) scaled to the stage height and centred
            let lc = lb.localcoord;
            // never wider than the window: narrow windows shrink the HUD instead of cutting it
            let ratio = (stage.height / lc[1].max(1.0)).min(screen_w / lc[0].max(1.0));
            let x_off = (screen_w - lc[0] * ratio) / 2.0;
            let lb_hud = |x: f32, y: f32| hud(x * ratio + x_off, y * ratio);
            // wider than the fight.def: each side's bars move out to its edge
            let arena = self.is_arena();
            // Tag teams a player drives: the TAG key by the power bar
            let tag_keys = [0, 1].map(|s| {
                (self.world.team_mode(s) == TeamMode::Tag && self.setup.ai[s] == 0)
                    .then(|| self.tag_labels[s].clone())
            });
            lb.draw(
                r,
                &self.world,
                &lb_hud,
                u * ratio,
                &portraits,
                x_off / ratio,
                arena,
                &tag_keys,
            );
            if let Some((left, right)) = self.run_hud.as_ref() {
                lb.draw_run_strip(r, &lb_hud, u * ratio, left, right);
            }
            if arena {
                let design = u * ratio * lc[1] / 720.0;
                for (anchor, mut hint) in self.arena_hints(ground, cam_y) {
                    // no room over it (the top of the view): under it
                    let mut anchor = anchor;
                    if anchor[1] - hint.design_height() * design < hud_top {
                        hint.below = true;
                        anchor[1] += 12.0;
                    }
                    lb.draw_hint(r, anchor, u * ratio, &hint);
                }
                // a gust coming or blowing: a tag under the clock
                if let Some(hint) = wind_hint(rugen_sim::hazards::wind(&self.world)) {
                    let anchor = lb_hud(lc[0] / 2.0, 140.0 * lc[1] / 720.0);
                    lb.draw_hint(r, anchor, u * ratio, &hint);
                }
            }
        } else {
            // life bars (screen units via `hud`)
            for (i, p) in self
                .world
                .players
                .iter()
                .enumerate()
                .filter(|(_, p)| p.helper_index == 0)
            {
                let frac = (p.life.max(0) as f32 / p.life_max.max(1) as f32).clamp(0.0, 1.0);
                let bar_w = screen_w * 0.4;
                let (x0, x1) = if i == 0 {
                    (10.0, 10.0 + bar_w)
                } else {
                    (screen_w - 10.0 - bar_w, screen_w - 10.0)
                };
                r.rect(hud(x0, 8.0), hud(x1, 16.0), [0.1, 0.1, 0.1, 0.9]);
                let (fx0, fx1) = if i == 0 {
                    (x1 - bar_w * frac, x1)
                } else {
                    (x0, x0 + bar_w * frac)
                };
                r.rect(hud(fx0, 8.0), hud(fx1, 16.0), [0.9, 0.8, 0.2, 1.0]);
                let pf = (p.power as f32 / p.power_max.max(1) as f32).clamp(0.0, 1.0);
                let (px0, px1) = if i == 0 {
                    (x0, x0 + bar_w * pf)
                } else {
                    (x1 - bar_w * pf, x1)
                };
                r.rect(
                    hud(x0, stage.height - 14.0),
                    hud(x1, stage.height - 10.0),
                    [0.1, 0.1, 0.1, 0.9],
                );
                r.rect(
                    hud(px0, stage.height - 14.0),
                    hud(px1, stage.height - 10.0),
                    [0.3, 0.6, 1.0, 1.0],
                );
            }
            // round clock, win marks, announcements
            let clock = self.world.round.clock();
            if clock >= 0 {
                draw_number(
                    r,
                    clock,
                    hud(screen_w / 2.0, 8.0),
                    2.0 * u,
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            for (i, wins) in self.world.round.wins.iter().enumerate() {
                for k in 0..*wins {
                    let x = if i == 0 {
                        10.0 + k as f32 * 8.0
                    } else {
                        screen_w - 15.0 - k as f32 * 8.0
                    };
                    r.rect(hud(x, 20.0), hud(x + 5.0, 25.0), [1.0, 0.9, 0.3, 1.0]);
                }
            }
            if !self.net_status.is_empty() {
                draw_text(
                    r,
                    &self.net_status,
                    hud(screen_w / 2.0, stage.height - 4.0),
                    u,
                    [0.8, 0.9, 1.0, 0.9],
                );
            }
            if let Some((text, _)) = &self.banner {
                draw_text(
                    r,
                    text,
                    hud(screen_w / 2.0, stage.height * 0.35),
                    3.0 * u,
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
        }
        self.draw_net_quality(r, &hud, u, screen_w);
        let _ = hud_w;
        let cam = Camera {
            screen: [screen_w, stage.height],
            center: [cam_x, center_y],
            zoom,
        };
        let clear = stage_visual
            .as_ref()
            .map_or([0.08, 0.08, 0.1, 1.0], |sv| sv.clear_color);
        self.stage_visual = stage_visual;
        self.lifebar = lifebar;
        (cam, clear)
    }
}

impl Match {
    /// In-world hints over the human fighters of an arena match (world
    /// position of the head, hint): the ledge actions while hanging.
    fn arena_hints(&self, ground: f32, cam_y: f32) -> Vec<([f32; 2], crate::lifebar::Hint)> {
        let w = &self.world;
        let mut out = Vec::new();
        for (i, p) in w.players.iter().enumerate() {
            if p.helper_index != 0 || p.removed || p.ai_level > 0 {
                continue;
            }
            let side = p.input_slot.min(1);
            let s = w.local_scale(i);
            let h = w
                .data_height
                .get(p.data)
                .copied()
                .filter(|h| *h > 0.0)
                .unwrap_or(60.0);
            let anchor = [p.pos[0] * s, ground + p.pos[1] * s - h - cam_y];
            if let Some(l) = p
                .ledge
                .filter(|l| l.action == rugen_sim::ledge::LedgeAction::Hang)
            {
                out.push((
                    anchor,
                    ledge_hint(l.left(), l.side, &self.arena_caps[side][0]),
                ));
            } else if w.items.held_by(i).is_some_and(|it| it.age < 180) {
                out.push((anchor, item_hint(&self.arena_caps[side][1])));
            }
        }
        // platforms about to drop: a countdown over each
        for v in rugen_sim::hazards::platform_views(w) {
            if let rugen_sim::hazards::DropPhase::Warn(left) = v.phase
                && let Some(pl) = w.stage.platforms.get(v.platform)
            {
                let anchor = [(pl.x1 + pl.x2) / 2.0, ground + pl.y - 6.0 - cam_y];
                out.push((anchor, drop_hint(left)));
            }
        }
        out
    }

    /// Hazard marks in the world: red stripes over a platform about to
    /// drop (blinking in its last second), wind streaks during a gust.
    fn draw_hazards(&self, r: &mut Renderer, ground: f32, cam_y: f32, cam_x: f32, view_w: f32) {
        let w = &self.world;
        for v in rugen_sim::hazards::platform_views(w) {
            let rugen_sim::hazards::DropPhase::Warn(left) = v.phase else {
                continue;
            };
            if left < 60 && (w.frame / 4) % 2 == 0 {
                continue;
            }
            let Some(pl) = w.stage.platforms.get(v.platform) else {
                continue;
            };
            let y = ground + pl.y - cam_y;
            let mut x = pl.x1;
            while x < pl.x2 {
                r.rect(
                    [x, y],
                    [(x + 6.0).min(pl.x2), y + 5.0],
                    [0.784, 0.063, 0.102, 0.85],
                );
                x += 12.0;
            }
        }
        let (push, _) = rugen_sim::hazards::wind(w);
        if push != 0.0 {
            // streaks drifting downwind across the view
            let t = w.frame as f32;
            for k in 0..9 {
                let y = ground - 20.0 - (k as f32 * 37.0) % 260.0 - cam_y;
                let span = view_w + 200.0;
                let x = cam_x - span / 2.0
                    + ((k as f32 * 131.0 + t * 9.0 * push.signum()).rem_euclid(span));
                let len = 30.0 + (k % 3) as f32 * 14.0;
                r.rect(
                    [x, y],
                    [x + len, y + 1.5],
                    [0.078, 0.071, 0.063, 0.35 * push.abs().min(1.0)],
                );
            }
        }
    }
}

/// The countdown over a platform about to drop.
pub fn drop_hint(left: i32) -> crate::lifebar::Hint {
    use crate::lifebar::{Hint, HintSeg};
    Hint {
        dark: false,
        title: None,
        rows: vec![vec![HintSeg::Text(format!(
            "DROPS IN {} S",
            (left + 59) / 60
        ))]],
        below: false,
    }
}

/// The tag under the clock while a gust is coming or blowing.
pub fn wind_hint((push, next): (f32, Option<(i32, f32)>)) -> Option<crate::lifebar::Hint> {
    use crate::lifebar::{Hint, HintSeg};
    let arrow = |d: f32| if d > 0.0 { "→" } else { "←" };
    let row = if push != 0.0 {
        vec![
            HintSeg::Text("WIND".into()),
            HintSeg::Cap(arrow(push).into()),
        ]
    } else {
        let (left, dir) = next?;
        vec![
            HintSeg::Text(format!("WIND IN {} S", (left + 59) / 60)),
            HintSeg::Cap(arrow(dir).into()),
        ]
    };
    Some(Hint {
        dark: true,
        title: None,
        rows: vec![row],
        below: false,
    })
}

/// The tag over a fighter that just picked up a throwable: B throws it.
pub fn item_hint(throw: &str) -> crate::lifebar::Hint {
    use crate::lifebar::{Hint, HintSeg};
    Hint {
        dark: false,
        title: None,
        rows: vec![vec![
            HintSeg::Text("PICKED UP ·".into()),
            HintSeg::Cap(throw.into()),
            HintSeg::Text("THROW".into()),
        ]],
        below: false,
    }
}

/// Ink colours of the arena items (approved mockup "Arena features").
const ITEM_INK: [f32; 4] = [0.078, 0.071, 0.063, 1.0];
const ITEM_RED: [f32; 4] = [0.784, 0.063, 0.102, 1.0];
const ITEM_BLUE: [f32; 4] = [0.145, 0.388, 0.922, 1.0];
const ITEM_PAPER: [f32; 4] = [0.949, 0.933, 0.894, 1.0];

/// A filled shape of horizontal strips centred on `c`: `half_w(dy)` gives
/// the half width at height `dy` (-rad..rad).
fn strips(r: &mut Renderer, c: [f32; 2], rad: f32, color: [f32; 4], half_w: impl Fn(f32) -> f32) {
    let n = 14;
    let step = 2.0 * rad / n as f32;
    for k in 0..n {
        let y0 = -rad + k as f32 * step;
        let w = half_w(y0 + step / 2.0);
        if w > 0.0 {
            r.rect([c[0] - w, c[1] + y0], [c[0] + w, c[1] + y0 + step], color);
        }
    }
}

/// One arena item at `c` (world space), `h` = half its size: a black
/// block (throwable), a red disc (heal) or a blue diamond (power), each
/// with an ink rim.
pub fn draw_item(r: &mut Renderer, kind: rugen_sim::items::ItemKind, c: [f32; 2], h: f32) {
    use rugen_sim::items::ItemKind;
    let rim = (h * 0.2).max(1.0);
    // a paper halo keeps the ink rim readable on dark backgrounds
    let halo = rim * 0.8;
    let o = h + halo;
    match kind {
        ItemKind::Throwable => r.rect([c[0] - o, c[1] - o], [c[0] + o, c[1] + o], ITEM_PAPER),
        ItemKind::Heal => strips(r, c, o, ITEM_PAPER, |y| {
            libm::sqrtf((o * o - y * y).max(0.0))
        }),
        ItemKind::Power => {
            let o = h * 1.25 + halo * 1.4;
            strips(r, c, o, ITEM_PAPER, |y| o - y.abs());
        }
    }
    match kind {
        ItemKind::Throwable => {
            r.rect([c[0] - h, c[1] - h], [c[0] + h, c[1] + h], ITEM_INK);
            r.rect_outline(
                [c[0] - h + rim, c[1] - h + rim],
                [c[0] + h - rim, c[1] + h - rim],
                rim * 0.5,
                [0.3, 0.29, 0.27, 1.0],
            );
        }
        ItemKind::Heal => {
            strips(r, c, h, ITEM_INK, |y| libm::sqrtf((h * h - y * y).max(0.0)));
            let i = h - rim;
            strips(r, c, i, ITEM_RED, |y| libm::sqrtf((i * i - y * y).max(0.0)));
        }
        ItemKind::Power => {
            let o = h * 1.25;
            strips(r, c, o, ITEM_INK, |y| o - y.abs());
            let i = o - rim * 1.4;
            strips(r, c, i, ITEM_BLUE, |y| i - y.abs());
        }
    }
}

impl Match {
    /// The arena items, in front of the fighters (a lying item about to go
    /// blinks).
    fn draw_arena_items(&self, r: &mut Renderer, ground: f32, cam_y: f32) {
        let w = &self.world;
        for it in &w.items.list {
            if it.expiring() && (w.frame / 6) % 2 == 0 {
                continue;
            }
            let c = [it.pos[0], ground + it.pos[1] - cam_y];
            draw_item(r, it.kind, c, rugen_sim::items::HALF);
        }
    }
}

/// The ledge panel: time left, then climb (up), roll in (toward the deck)
/// and jump (the X key).
pub fn ledge_hint(left: i32, side: f32, jump: &str) -> crate::lifebar::Hint {
    use crate::lifebar::{Hint, HintSeg};
    let tenths = (left.max(0) * 10 + 59) / 60;
    let row = |cap: &str, label: &str| vec![HintSeg::Cap(cap.into()), HintSeg::Text(label.into())];
    Hint {
        dark: true,
        title: Some(format!("LEDGE · {}.{} S", tenths / 10, tenths % 10)),
        rows: vec![
            row("↑", "CLIMB"),
            row(if side > 0.0 { "←" } else { "→" }, "ROLL IN"),
            row(jump, "JUMP"),
        ],
        below: false,
    }
}

/// 3x5 pixel font, enough for the HUD.
fn glyph(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        _ => [0; 5],
    }
}

/// Harness: `RUGEN_HARNESS_STATE=1:1300@120,2:200@150` puts player 1 into
/// state 1300 at the first fight frame from world frame 120 on (and P2 into
/// 200 from 150) with full power,
/// so a super can be shot without its command; `RUGEN_HARNESS_POWER=1`
/// keeps both root players' power full.
fn harness_states(w: &mut World) {
    if std::env::var_os("RUGEN_HARNESS_POWER").is_some() {
        for p in w.players.iter_mut().filter(|p| p.helper_index == 0) {
            p.power = p.power_max;
        }
    }
    let Some(spec) = std::env::var("RUGEN_HARNESS_STATE").ok() else {
        return;
    };
    for tok in spec.split(',') {
        let parse = || -> Option<(usize, i32, u32)> {
            let (who, rest) = tok.trim().split_once(':')?;
            let (state, at) = rest.split_once('@')?;
            Some((who.parse().ok()?, state.parse().ok()?, at.parse().ok()?))
        };
        // the first fight frame (round state 2) at or after `at`, once
        thread_local!(static FIRED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) });
        if let Some((who, state, at)) = parse()
            && w.frame >= at
            && w.round.round_state() == 2
            && !FIRED.with_borrow(|f| f.iter().any(|t| t == tok))
            && let Some(i) = w.side_leader(who.saturating_sub(1).min(1))
        {
            FIRED.with_borrow_mut(|f| f.push(tok.to_string()));
            w.players[i].power = w.players[i].power_max;
            w.change_state(i, state, None, Some(false));
        }
    }
}

/// Draw centered text with the pixel font; `px` = pixel size in logical units.
/// `--script`: comma-separated tokens of key letters held for one tick
/// (`U D L R a b c x y z s`, `.` = nothing) with an optional `*n` repeat,
/// e.g. `R*40,.*20,D,DR,R,xy,.*10`.
pub fn parse_script(s: &str) -> Vec<Buttons> {
    let mut out = Vec::new();
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
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
        for _ in 0..n {
            out.push(b);
        }
    }
    out
}

/// Where the tutorial panel goes (HUD units): its box and each line's
/// baseline, from the lines' `[width, ink top, ink bottom]` (the ink band
/// relative to the baseline).
struct TutorialLayout {
    top: f32,
    bottom: f32,
    width: f32,
    baselines: [f32; 3],
}

fn tutorial_layout(inks: &[[f32; 3]; 3]) -> TutorialLayout {
    const TOP: f32 = 44.0;
    const PAD: f32 = 5.0;
    const GAP: f32 = 2.0;
    let mut y = TOP + PAD;
    let mut baselines = [0.0; 3];
    for (k, m) in inks.iter().enumerate() {
        baselines[k] = y - m[1];
        y = baselines[k] + m[2] + GAP;
    }
    let widest = inks.iter().map(|m| m[0]).fold(0.0, f32::max);
    TutorialLayout {
        top: TOP,
        bottom: y - GAP + PAD,
        width: (widest + 4.0 * PAD).max(300.0),
        baselines,
    }
}

pub(crate) fn draw_text(r: &mut Renderer, text: &str, center: [f32; 2], px: f32, color: [f32; 4]) {
    let w = text.chars().count() as f32 * 4.0 * px;
    let mut x = center[0] - w / 2.0;
    for c in text.chars() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    let x0 = x + col as f32 * px;
                    let y0 = center[1] + row as f32 * px;
                    r.rect([x0, y0], [x0 + px, y0 + px], color);
                }
            }
        }
        x += 4.0 * px;
    }
}

fn draw_number(r: &mut Renderer, n: i32, center: [f32; 2], px: f32, color: [f32; 4]) {
    draw_text(r, &format!("{n:02}"), center, px, color);
}

/// What hot reload watches for a fighter.
#[derive(Clone, Debug)]
enum Watch {
    /// The def lives in the `cache/` conversion of a folder in `chars/`.
    WorkingCopy,
    /// A def loaded straight from a folder: its newest source mtime.
    Folder(PathBuf, u64),
}

impl Watch {
    /// `None` for content that cannot change (an `.rgc` / `.rgs` file).
    fn of(def: &Path) -> Option<Self> {
        match rugen_formats::vfs::split(def) {
            Some((content, _)) => {
                let mut side = content.into_os_string();
                side.push(".source.json");
                PathBuf::from(side).is_file().then_some(Watch::WorkingCopy)
            }
            None => {
                let dir = def.parent().unwrap_or(Path::new(".")).to_path_buf();
                let m = newest_mtime(&dir);
                Some(Watch::Folder(dir, m))
            }
        }
    }
}

/// Newest modification time (seconds) of any state/anim/command file under `dir`.
fn newest_mtime(dir: &Path) -> u64 {
    let mut newest = 0u64;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(
            ext.as_str(),
            "cns" | "cmd" | "air" | "def" | "st" | "sff" | "snd"
        ) {
            continue;
        }
        if let Ok(m) = e.metadata()
            && let Ok(t) = m.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            newest = newest.max(d.as_secs());
        }
    }
    newest
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Where the ground line sits on screen (0 = top, 1 = bottom) at `zoom`:
/// the stage's own `zoffset` fraction at zoom 1 and above, easing to 0.66 as
/// the camera pulls back to 0.3.
fn ground_screen_frac(at_rest: f32, zoom: f32) -> f32 {
    let target = 0.66f32.min(at_rest);
    let t = ((1.0 - zoom) / 0.7).clamp(0.0, 1.0);
    at_rest + (target - at_rest) * t
}

/// Join a relay session (UDP fast path, WebSocket fallback), wait for the
/// other slots, bring up the fastest player ↔ player path (direct peer to
/// peer unless either player opted out, else TURN, else the relay itself),
/// measure the round trip on that path and start the rollback session (or
/// a spectator session for slots 2+).
fn relay_session(
    server: &str,
    ticket: &rugen_net::online::SessionTicket,
    websocket_only: bool,
    cfg: &Config,
) -> anyhow::Result<(Netplay, Option<u32>)> {
    use std::time::Duration;
    let base = rugen_net::online::base_url(server);
    let probe = if websocket_only {
        Duration::ZERO
    } else {
        Duration::from_millis(1500)
    };
    let mut socket =
        rugen_net::RelaySocket::connect(ticket, &base, probe, &mut |s| log::info!("netplay: {s}"))
            .context("joining the relay session")?;
    let transport = socket.transport_name();
    if ticket.slot >= 2 {
        socket.wait_for(0b001, Duration::from_secs(60), &mut |s| {
            log::info!("netplay: {s}")
        })?;
        log::info!(
            "netplay: spectating session {:08x} over {transport}",
            ticket.session
        );
        let n = Netplay::spectate(socket, &NetOptions::default())?;
        return Ok((n, None));
    }
    // players: gather direct candidates while the other player connects
    // (exchanged only when both allow direct connections)
    socket.enable_direct(&rugen_net::DirectConfig {
        allowed: cfg.net.direct,
        stun: stun_servers(ticket),
        ..Default::default()
    });
    socket.wait_for(0b011, Duration::from_secs(90), &mut |s| {
        log::info!("netplay: {s}")
    })?;
    // direct, TURN or the relay: the fastest working path within 1.5 s
    // (the others keep trying and take over mid-match when better)
    let path = socket.settle_paths(Duration::from_millis(1500));
    if let Some(off) = socket.direct_off() {
        log::info!("netplay: direct path not used: {}", off.reason());
    }
    let transport = socket.transport_name();
    let peer = 1 - ticket.slot;
    let samples = socket.peer_rtt_samples(peer, 24, Duration::from_millis(1500));
    let rtt = rugen_net::RttStats::from_us(&samples);
    let policy = cfg.net.delay_policy();
    let o = NetOptions {
        input_delay: cfg.net.input_delay.frames(rtt.map(|r| r.p95), &policy),
        max_prediction: policy.prediction_window,
        ..Default::default()
    };
    log::info!(
        "netplay: relay session {:08x}, path {} over {transport}, playing P{}, rtt {} ms (p95 {} ms, jitter {} ms), input delay {}",
        ticket.session,
        path.label(),
        ticket.slot + 1,
        rtt.map_or(0, |r| r.median),
        rtt.map_or(0, |r| r.p95),
        rtt.map_or(0, |r| r.jitter),
        o.input_delay
    );
    let n = Netplay::relay(socket, ticket.slot as usize, ticket.spectators, &o)?;
    Ok((n, rtt.map(|r| r.median)))
}

/// STUN servers for the direct path: the ticket's (`stun:` URLs next to
/// its TURN service), else Cloudflare's.
fn stun_servers(ticket: &rugen_net::online::SessionTicket) -> Vec<String> {
    let mut v: Vec<String> = ticket
        .transports
        .iter()
        .flat_map(|t| t.urls.iter())
        .filter_map(|u| u.strip_prefix("stun:"))
        .map(|h| {
            let h = h.split('?').next().unwrap_or(h);
            if h.rsplit_once(':')
                .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
            {
                h.to_string()
            } else {
                format!("{h}:3478")
            }
        })
        .collect();
    if v.is_empty() {
        v.push(rugen_net::direct::DEFAULT_STUN.to_string());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owner report 4 (Goku's ki blasts dark): the blast is an additive
    /// helper and a subtractive one on the same sprite and sprpriority,
    /// created in that order. Ikemen draws the later one behind, so the
    /// glow lands on top of the shade, and the shade's PalFX multiply is the
    /// mean of the other two channels.
    #[test]
    fn equal_priority_draws_stack_like_ikemen() {
        // helpers P7 (add, pushed first) and P8 (sub): P8 sorts first = behind
        assert!(queue_order('P', 7) > queue_order('P', 8));
        // players over explods, projectiles over players
        assert!(queue_order('E', 0) < queue_order('P', 9));
        assert!(queue_order('P', 9) < queue_order('J', 20));
        // Goku's FXSUB helper: mul = 0,256,512 subtracts mostly red
        assert_eq!(sub_mul([0.0, 1.0, 2.0]), [1.5, 1.0, 0.5]);
        assert_eq!(sub_mul([1.0; 3]), [1.0; 3]);
    }

    /// Online camera determinism: the camera is part of the simulation, so
    /// two players with different zoom settings (and the server's
    /// re-simulation, which uses `ONLINE_ZOOM`) must simulate the same
    /// match. Offline and in a local sync test the player's own settings stay.
    #[test]
    fn online_sessions_ignore_the_players_camera_settings() {
        use crate::config::CameraCfg;
        let wide = CameraCfg {
            zoom_out: 0.3,
            zoom_in: 0.7,
        };
        let close = CameraCfg {
            zoom_out: 0.95,
            zoom_in: 1.6,
        };
        let p2p = NetMode::P2P {
            port: 7000,
            remote: "127.0.0.1:7001".parse().unwrap(),
            side: 1,
        };
        let relay = NetMode::Relay {
            server: "http://127.0.0.1:8090".into(),
            ticket: Default::default(),
            websocket_only: false,
        };
        let spectator = NetMode::Relay {
            server: "http://127.0.0.1:8090".into(),
            ticket: rugen_net::online::SessionTicket {
                slot: 2,
                spectators: 1,
                ..Default::default()
            },
            websocket_only: true,
        };
        for net in [&p2p, &relay, &spectator] {
            assert_eq!(
                match_zoom(Some(net), &wide),
                rugen_sim::headless::ONLINE_ZOOM
            );
            assert_eq!(
                match_zoom(Some(net), &close),
                rugen_sim::headless::ONLINE_ZOOM
            );
        }
        assert_eq!(match_zoom(None, &wide), (0.3, 0.7));
        assert_eq!(match_zoom(Some(&NetMode::SyncTest), &close), (0.95, 1.6));
        // the server's re-simulation (rugen-server src/resim.rs) uses these
        assert_eq!(rugen_sim::headless::ONLINE_ZOOM, (0.6, 1.25));

        // and the worlds really agree (bundled KFM, skipped when absent)
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = root.join("stages/kfm.def");
        if !kfm.exists() || !stage.exists() {
            return;
        }
        let run = |zoom: (f32, f32)| {
            let spec = rugen_sim::headless::MatchSpec {
                p1: &kfm,
                p2: &kfm,
                stage: Some(&stage),
                data_dir: &root.join("data"),
                motif: &root.join("data/system.def"),
                zoom,
                movement: rugen_sim::world::Movement::default(),
                round_time: None,
                ai: [0, 0],
                fighter_size: Default::default(),
                seed: rugen_sim::headless::LEGACY_SEED,
            };
            let mut w = rugen_sim::headless::build_world(&spec).expect("KFM world");
            // walk apart, then together: the camera zooms with the distance
            let mut inputs = Vec::new();
            for f in 0..600u32 {
                let (l, r) = (1u16 << 2, 1u16 << 3);
                let (a, b) = if f < 300 { (l, r) } else { (r, l) };
                inputs.extend_from_slice(&a.to_le_bytes());
                inputs.extend_from_slice(&b.to_le_bytes());
            }
            rugen_sim::headless::replay(&mut w, &inputs, None).checksum
        };
        assert_eq!(
            run(match_zoom(Some(&relay), &wide)),
            run(match_zoom(Some(&relay), &close)),
            "online worlds of two differently set players agree"
        );
        assert_ne!(
            run(match_zoom(None, &wide)),
            run(match_zoom(None, &close)),
            "the settings do shape the camera (the test means something)"
        );
    }

    /// A Survival fight: one round with P1's carried life; P1 standing
    /// still loses it and the match is over (no second round, life full
    /// never comes back). Replaying the inputs gives the same world.
    #[test]
    fn run_rules_play_one_round_with_carried_life() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = root.join("stages/kfm.def");
        if !kfm.exists() || !stage.exists() {
            return;
        }
        let spec = rugen_sim::headless::MatchSpec {
            p1: &kfm,
            p2: &kfm,
            stage: Some(&stage),
            data_dir: &root.join("data"),
            motif: &root.join("data/system.def"),
            zoom: (1.0, 1.0),
            movement: rugen_sim::world::Movement::default(),
            round_time: Some(10),
            ai: [0, 4],
            fighter_size: Default::default(),
            seed: rugen_sim::headless::LEGACY_SEED,
        };
        let rules = RunRules {
            match_wins: 1,
            p1_life: Some(150),
        };
        let build = || {
            let mut w = rugen_sim::headless::build_world(&spec).expect("KFM world");
            rules.apply(&mut w);
            w
        };
        let mut w = build();
        assert_eq!(w.players[0].life, 150);
        assert_eq!(w.players[1].life, w.players[1].life_max);
        assert_eq!(w.round.config.match_wins, 1);
        let mut frames = 0;
        while !w.round.match_over && frames < 60 * 30 {
            w.step(&[Buttons::default(), Buttons::default()]);
            assert!(w.players[0].life <= 150, "no refill mid-run");
            frames += 1;
        }
        assert!(w.round.match_over, "one round decides the fight");
        assert_eq!(w.round.round_no, 1);
        let a = rugen_sim::headless::replay(&mut build(), &vec![0u8; frames * 4], None);
        let b = rugen_sim::headless::replay(&mut build(), &vec![0u8; frames * 4], None);
        assert_eq!(a.checksum, b.checksum);
        // a carried life never starts below 1
        let mut w = rugen_sim::headless::build_world(&spec).expect("KFM world");
        RunRules {
            match_wins: 1,
            p1_life: Some(0),
        }
        .apply(&mut w);
        assert_eq!(w.players[0].life, 1);
    }

    /// `--team1 turns:goku,kfm`: the mode, then the partners.
    #[test]
    fn team_sides_parse() {
        assert_eq!(
            TeamSetup::parse_side("turns:goku, kfm"),
            Some((TeamMode::Turns, vec!["goku".into(), "kfm".into()]))
        );
        assert_eq!(
            TeamSetup::parse_side("single"),
            Some((TeamMode::Single, vec![]))
        );
        assert_eq!(TeamSetup::parse_side("trio:goku"), None);
        // VS: both humans, their partners play at COMPUTER LEVEL
        assert_eq!(partner_ai([0, 0], 5), [5, 5]);
        // the CPU side keeps its level; a level of 0 still means a computer partner
        assert_eq!(partner_ai([0, 7], 0), [1, 7]);
    }

    /// Per-match seed: offline matches each draw a different seed, never
    /// the legacy 1 that every match used to play with.
    #[test]
    fn offline_matches_get_fresh_seeds() {
        let seeds: Vec<u64> = (0..64).map(|_| fresh_seed()).collect();
        let mut d = seeds.clone();
        d.sort_unstable();
        d.dedup();
        assert_eq!(d.len(), seeds.len());
        assert!(!seeds.contains(&rugen_sim::headless::LEGACY_SEED));
    }

    /// LAN netplay settings desync: air jumps, air dash and fast fall came
    /// from each player's own `[game]` options. The host's ride the
    /// handshake now, the guest takes them, and a peer that cannot carry
    /// them (0.6.1 and earlier) is refused instead of drifting apart.
    #[test]
    fn lan_peers_simulate_the_hosts_movement() {
        let host = LanRules {
            fighter_size: FighterSize::parse("chibi").unwrap(),
            movement: rugen_sim::world::Movement {
                air_jumps: 2,
                air_dash: true,
                fast_fall: false,
            },
            seed: Some(77),
        };
        let wire = host.encode();
        assert!(!wire.contains(' '), "the punch format splits at spaces");
        let v = Some(rugen_net::LAN_PROTOCOL);
        // the guest (its own options: none of these) takes the host's
        let guest = lan_agreement(1, v, Some(&wire)).unwrap();
        assert_eq!(guest, Some(host));
        // the host keeps its own
        assert_eq!(lan_agreement(0, v, None).unwrap(), None);
        // a 0.6.1 host: the size only, no protocol number
        assert!(lan_agreement(1, None, Some("fighter_size=chibi")).is_err());
        assert!(lan_agreement(1, v, Some("fighter_size=chibi")).is_err());
        // a 0.6.1 guest (no protocol number) is refused by the host
        assert!(lan_agreement(0, None, None).is_err());
        // a newer host's extra rule is ignored
        assert_eq!(LanRules::decode(&format!("{wire};future=1")), Some(host));

        // per-match seed: two protocol 3 peers play the host's
        assert_eq!(lan_seed(0, v, 77, None), 77);
        assert_eq!(lan_seed(1, v, 5, guest.as_ref()), 77);
        // a protocol 2 guest skips the seed: both sides play seed 1
        let old = Some(rugen_net::LAN_PROTOCOL_MIN);
        assert!(lan_agreement(0, old, None).is_ok());
        assert_eq!(lan_seed(0, old, 77, None), rugen_sim::headless::LEGACY_SEED);
        // a protocol 2 host sends rules without a seed
        let no_seed = LanRules { seed: None, ..host }.encode();
        assert!(!no_seed.contains("seed"));
        let from_old = lan_agreement(1, old, Some(&no_seed)).unwrap();
        assert_eq!(
            lan_seed(1, old, 5, from_old.as_ref()),
            rugen_sim::headless::LEGACY_SEED
        );

        // and the worlds really agree (bundled KFM, skipped when absent)
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = root.join("stages/kfm.def");
        if !kfm.exists() || !stage.exists() {
            return;
        }
        let run = |m: rugen_sim::world::Movement| {
            let spec = rugen_sim::headless::MatchSpec {
                p1: &kfm,
                p2: &kfm,
                stage: Some(&stage),
                data_dir: &root.join("data"),
                motif: &root.join("data/system.def"),
                zoom: rugen_sim::headless::ONLINE_ZOOM,
                movement: m,
                round_time: None,
                ai: [0, 0],
                fighter_size: Default::default(),
                seed: rugen_sim::headless::LEGACY_SEED,
            };
            let mut w = rugen_sim::headless::build_world(&spec).expect("KFM world");
            // P1 jumps forward and presses up again in the air
            let mut inputs = Vec::new();
            for f in 0..240u32 {
                let a: u16 = match f % 40 {
                    0..=2 | 14..=16 => 1 | (1 << 3),
                    _ => 0,
                };
                inputs.extend_from_slice(&a.to_le_bytes());
                inputs.extend_from_slice(&0u16.to_le_bytes());
            }
            rugen_sim::headless::replay(&mut w, &inputs, None).checksum
        };
        assert_ne!(
            run(host.movement),
            run(rugen_sim::world::Movement::default()),
            "the movement shapes the match (the test means something)"
        );
        assert_eq!(run(host.movement), run(guest.unwrap().movement));
    }

    /// The tutorial's step title, hint and progress never overlap ("JUMP"
    /// sat on "PRESS ↑": fixed baselines ignored the font heights).
    #[test]
    fn ledge_hint_counts_down_and_points_at_the_deck() {
        use crate::lifebar::HintSeg;
        let h = ledge_hint(84, 1.0, "U");
        assert_eq!(h.title.as_deref(), Some("LEDGE · 1.4 S"));
        assert_eq!(
            h.rows[1][0],
            HintSeg::Cap("←".into()),
            "right edge: roll in leftward"
        );
        assert_eq!(h.rows[2][0], HintSeg::Cap("U".into()));
        assert_eq!(
            ledge_hint(120, -1.0, "X").title.as_deref(),
            Some("LEDGE · 2.0 S")
        );
        assert_eq!(
            ledge_hint(1, -1.0, "X").rows[1][0],
            HintSeg::Cap("→".into())
        );
        assert_eq!(
            ledge_hint(0, -1.0, "X").title.as_deref(),
            Some("LEDGE · 0.0 S")
        );
    }

    #[test]
    fn hazard_hints_count_down() {
        use crate::lifebar::HintSeg;
        assert_eq!(
            drop_hint(180).rows[0][0],
            HintSeg::Text("DROPS IN 3 S".into())
        );
        assert_eq!(
            drop_hint(1).rows[0][0],
            HintSeg::Text("DROPS IN 1 S".into())
        );
        assert!(wind_hint((0.0, None)).is_none());
        let coming = wind_hint((0.0, Some((120, -1.0)))).unwrap();
        assert_eq!(coming.rows[0][0], HintSeg::Text("WIND IN 2 S".into()));
        assert_eq!(coming.rows[0][1], HintSeg::Cap("←".into()));
        let blowing = wind_hint((0.7, None)).unwrap();
        assert_eq!(blowing.rows[0][1], HintSeg::Cap("→".into()));
    }

    #[test]
    fn item_hint_names_the_throw_key() {
        use crate::lifebar::HintSeg;
        let h = item_hint("K");
        assert!(!h.dark && h.title.is_none());
        assert_eq!(h.rows[0][1], HintSeg::Cap("K".into()));
        assert_eq!(h.rows[0][2], HintSeg::Text("THROW".into()));
    }

    #[test]
    fn tutorial_lines_do_not_overlap() {
        // fight.def fonts: ink 14 units over the baseline for the title, 9
        // for the hint (2 below it), the built-in font's 5 down from its top
        let inks = [[60.0, -14.0, 0.0], [80.0, -9.0, 2.0], [400.0, 0.0, 5.0]];
        let l = tutorial_layout(&inks);
        let band = |k: usize| (l.baselines[k] + inks[k][1], l.baselines[k] + inks[k][2]);
        assert!(band(0).0 > l.top, "inside the box");
        for k in 1..3 {
            assert!(
                band(k).0 > band(k - 1).1,
                "line {k} starts below line {}",
                k - 1
            );
        }
        assert!(l.bottom > band(2).1, "the box holds every line");
        assert!(l.width >= 400.0, "and the widest one");
    }

    /// Goku's ki blasts: AIR flags reach the draw (players, helpers,
    /// explods, projectiles and afterimages all resolve through here), the
    /// `Trans` controller wins over them, and a faded-out explod alpha (0)
    /// stays faded.
    #[test]
    fn effect_blends_reach_the_draw() {
        let air = rugen_formats::air::Air::parse(
            "[Begin Action 410]\n411,0, 0,0, 2, , A\n411,1, 0,0, 2,, A1\n\
             420,8, 0,0, 2,, S\n450,2, 0,0, 3,, AS200D256\n415,0, 0,0, 1\n",
        );
        let f = |i: usize| air.get(410).expect("action 410").frames[i].clone();
        let none = [-1, 0];
        assert_eq!(
            resolve_blend(&f(0), Trans::Default, none),
            Blend::Add { src: 255, dst: 255 }
        );
        assert_eq!(
            resolve_blend(&f(1), Trans::Default, none),
            Blend::Add { src: 255, dst: 128 }
        );
        assert_eq!(
            resolve_blend(&f(2), Trans::Default, none),
            Blend::Sub { src: 255, dst: 255 }
        );
        assert_eq!(
            resolve_blend(&f(3), Trans::Default, none),
            Blend::Add { src: 200, dst: 255 }
        );
        assert_eq!(resolve_blend(&f(4), Trans::Default, none), Blend::Alpha);
        // Trans = add on a frame without flags, Trans = sub over an A frame
        assert_eq!(
            resolve_blend(&f(4), Trans::Add, none),
            Blend::Add { src: 255, dst: 255 }
        );
        assert_eq!(
            resolve_blend(&f(0), Trans::Sub, none),
            Blend::Sub { src: 255, dst: 255 }
        );
        assert_eq!(
            resolve_blend(&f(4), Trans::AddAlpha, [156, 255]),
            Blend::Add { src: 156, dst: 255 }
        );
        assert_eq!(
            resolve_blend(&f(4), Trans::AddAlpha, [0, 255]),
            Blend::Add { src: 0, dst: 255 },
            "a faded explod stays faded"
        );
        assert_eq!(
            resolve_blend(&f(4), Trans::AddAlpha, none),
            Blend::Add { src: 255, dst: 0 },
            "addalpha without alpha"
        );
    }

    fn press(f: impl Fn(&mut Buttons)) -> Buttons {
        let mut b = Buttons::default();
        f(&mut b);
        b
    }

    #[test]
    fn fleeing_takes_a_confirmation() {
        let (down, ok, back) = (
            press(|b| b.d = true),
            press(|b| b.a = true),
            press(|b| b.b = true),
        );
        let mut m = FleeMenu::default();
        assert_eq!(m.input(ok), MenuAction::Close, "RESUME first");
        assert_eq!(m.input(down), MenuAction::Stay);
        assert_eq!(m.input(ok), MenuAction::Stay, "FLEE asks");
        assert!(m.confirm && m.sel == 0, "focused on NO");
        assert_eq!(m.input(ok), MenuAction::Stay, "NO goes back");
        assert!(!m.confirm);
        m.input(down);
        m.input(ok);
        assert_eq!(m.input(back), MenuAction::Stay, "back leaves the question");
        assert!(!m.confirm);
        assert_eq!(m.input(back), MenuAction::Close);
        m.input(down);
        m.input(ok);
        m.input(down);
        assert_eq!(
            m.input(press(|b| b.s = true)),
            MenuAction::Flee,
            "YES, FLEE"
        );
    }

    #[test]
    fn early_endings_report_what_happened() {
        use rugen_net::PeerExit;
        assert_eq!(EarlyEnd::Fled.ending(), rugen_proto::Ending::Flee);
        assert_eq!(
            EarlyEnd::PeerGone(PeerExit::Left).ending(),
            rugen_proto::Ending::Flee
        );
        assert_eq!(
            EarlyEnd::PeerGone(PeerExit::Lost).ending(),
            rugen_proto::Ending::Disconnect
        );
        assert_eq!(
            EarlyEnd::PeerGone(PeerExit::Fled).message(),
            "YOUR OPPONENT FLED"
        );
    }

    /// A vs B, then C vs D in the same process (one audio cache, like the
    /// game): C's PlaySnd must play C's sample, never the one A cached under
    /// the same slot / group / number.
    #[test]
    fn a_later_fight_never_hears_an_earlier_fighters_sounds() {
        use rugen_sim::world::Stage;
        use std::sync::Arc;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        let def = root.join("chars/kfm/kfm.def");
        if !def.exists() {
            return;
        }
        let a = CharData::load(&def).expect("kfm loads");
        let snd = a.snd.as_ref().expect("kfm has sounds");
        // two sounds of different lengths: C plays the second under the first's number
        let mut keys = snd.sounds.keys().copied();
        let first = keys.next().expect("a sound");
        let len = |k: (i32, i32)| snd.get(k.0, k.1).map(<[u8]>::len);
        let second = keys
            .find(|k| len(*k) != len(first))
            .expect("two sounds of different sizes");
        let mut c = (*a).clone();
        let csnd = c.snd.as_mut().unwrap();
        let swapped = csnd.sounds[&second].clone();
        csnd.sounds.insert(first, swapped);
        let c = Arc::new(c);

        let fight = |p1: Arc<CharData>, p2: Arc<CharData>| {
            let mut w = World::new(Stage::default(), 1);
            w.add_player(p1, 0, 1);
            w.add_player(p2, 1, 2);
            w
        };
        let mut cache = rugen_audio::SampleCache::default();
        let mut play = |w: &World, banks: &mut SoundBanks| {
            let (key, bytes) = resolve_sound(w, banks, 0, true, false, first.0, first.1);
            if !cache.is_loaded(key) {
                cache.load(key, bytes.expect("sound found"));
            }
            (key, cache.get(key).expect("decoded").frames.len())
        };
        let frames_of = |k: (i32, i32)| {
            let mut c = rugen_audio::SampleCache::default();
            let key = SoundKey {
                bank: 1,
                group: k.0,
                number: k.1,
            };
            c.load(key, snd.get(k.0, k.1).unwrap());
            c.get(key).unwrap().frames.len()
        };
        assert_ne!(frames_of(first), frames_of(second));

        // fight 1: A vs B
        let w1 = fight(a.clone(), a.clone());
        let mut banks1 = SoundBanks::new();
        let (k1, f1) = play(&w1, &mut banks1);
        assert_eq!(f1, frames_of(first), "A plays its own sound");
        drop(w1);
        // fight 2: C vs D, same slot, same group / number, cache still warm
        let w2 = fight(c.clone(), a.clone());
        let mut banks2 = SoundBanks::new();
        let (k2, f2) = play(&w2, &mut banks2);
        assert_ne!(k1.bank, k2.bank, "C's bank is not A's");
        assert_eq!(f2, frames_of(second), "C hears C's sound, not A's");
        // D (slot 1) has its own bank too, apart from C's
        let (kd, _) = resolve_sound(&w2, &mut banks2, 1, true, false, first.0, first.1);
        assert_ne!(kd.bank, k2.bank);
        // common / announcer sounds of the two fights stay apart as well
        let (c1, _) = resolve_sound(&w2, &mut banks1, 0, false, false, 0, 0);
        let (c2, _) = resolve_sound(&w2, &mut banks2, 0, false, false, 0, 0);
        assert_ne!(c1.bank, c2.bank);
    }
}
