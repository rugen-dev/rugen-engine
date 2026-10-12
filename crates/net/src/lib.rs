//! Rollback netplay via ggrs.
//!
//! The simulation ([`World`]) is deterministic, plain data and `Clone`, so
//! ggrs can snapshot it every frame and re-simulate after late inputs arrive.
//! The only thing on the wire is each player's [`NetInput`] (11 button bits).
//!
//! Online matches go through the player's chosen `rugen-server`
//! ([`relay::RelaySocket`]): nobody learns the other player's address. LAN /
//! explicit-IP matches use a plain UDP [`Socket`].

#![forbid(unsafe_code)]

pub mod online;
pub mod quality;
pub mod relay;
pub mod socket;
pub mod stun;
pub mod turn;

pub use quality::{EventFilter, Quality, auto_input_delay};
pub use relay::{RelaySocket, slot_addr};
pub use socket::{Punched, Socket};

use std::net::SocketAddr;

use ggrs::{
    DesyncDetection, GgrsError, GgrsEvent, GgrsRequest, NonBlockingSocket, P2PSession, PlayerType,
    SessionBuilder, SessionState, SpectatorSession, SyncTestSession,
};
use rugen_sim::world::Event;
use rugen_sim::{Buttons, World};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ggrs: {0}")]
    Ggrs(#[from] GgrsError),
    #[error("socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("desync at frame {frame}: local {local:#x} remote {remote:#x}")]
    Desync {
        frame: i32,
        local: u128,
        remote: u128,
    },
    #[error("rendezvous: {0}")]
    Rendezvous(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// One player's input for one frame: [`Buttons::bits`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetInput(pub u16);

impl From<Buttons> for NetInput {
    fn from(b: Buttons) -> Self {
        NetInput(b.bits())
    }
}

impl From<NetInput> for Buttons {
    fn from(n: NetInput) -> Self {
        Buttons::from_bits(n.0)
    }
}

/// ggrs configuration: the whole [`World`] is the saved state.
#[derive(Debug)]
pub struct NetConfig;

impl ggrs::Config for NetConfig {
    type Input = NetInput;
    type State = World;
    type Address = SocketAddr;
}

enum Session {
    P2P(Box<P2PSession<NetConfig>>),
    SyncTest(Box<SyncTestSession<NetConfig>>),
    Spectator(Box<SpectatorSession<NetConfig>>),
}

/// Options for a two-player session.
#[derive(Clone, Debug)]
pub struct Options {
    /// Frames of local input delay (fewer rollbacks at the cost of latency).
    pub input_delay: usize,
    /// How far ggrs may predict ahead before stalling.
    pub max_prediction: usize,
    /// Checksum comparison interval in frames (0 = off).
    pub desync_interval: u32,
    pub fps: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            input_delay: 2,
            max_prediction: 8,
            desync_interval: 30,
            fps: 60,
        }
    }
}

/// Connection quality snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub ping_ms: u128,
    pub frames_ahead: i32,
    pub rollbacks: u64,
    pub synchronized: bool,
    /// Average frames re-simulated per tick over the last second.
    pub avg_rollback: f32,
    /// Interruptions, a large frame advantage or deep rollbacks right now.
    pub unstable: bool,
    /// Input delay this session runs with.
    pub input_delay: usize,
}

impl Stats {
    /// 1-4 bars from the measured ping (see [`rugen_proto::ping_bars`]).
    pub fn bars(&self) -> u8 {
        rugen_proto::ping_bars(self.ping_ms.min(u32::MAX as u128) as u32)
    }
}

/// What happened during one call to [`Netplay::advance`].
#[derive(Debug, Default)]
pub struct Advanced {
    /// Frames the authoritative timeline moved forward (0 while stalled / synchronizing).
    pub new_frames: u32,
    /// Sim events to play: everything from newly reached frames, plus events a
    /// rollback revealed that were not played for that frame yet ([`EventFilter`]).
    pub events: Vec<Event>,
    /// Human-readable session notices (sync progress, interruptions).
    pub notices: Vec<String>,
}

pub struct Netplay {
    session: Session,
    local_handle: usize,
    /// Rollback-safe sound / announcer emission.
    events: EventFilter,
    quality: Quality,
    stats: Stats,
    skip_frames: u32,
    /// Inputs of both players per simulated frame (corrected on rollback),
    /// for result verification and replays.
    log: Vec<[u16; 2]>,
    log_start: Option<u32>,
    /// World checksum every [`rugen_proto::evidence::CHECKSUM_EVERY`]
    /// frames (corrected on rollback), for result verification.
    checks: Vec<(u32, u64)>,
    /// TURN byte counters when the session had a `turn` transport.
    turn_meter: Option<std::sync::Arc<std::sync::Mutex<turn::TurnStats>>>,
}

impl Netplay {
    /// Peer-to-peer session over UDP. `local_handle` is 0 for P1, 1 for P2.
    pub fn p2p(
        local_port: u16,
        remote: SocketAddr,
        local_handle: usize,
        opts: &Options,
    ) -> Result<Self> {
        let socket = Socket::bind(local_port)?;
        Self::p2p_with_socket(socket, remote, local_handle, opts)
    }

    /// Peer-to-peer session over an already bound [`Socket`] (after a handshake).
    pub fn p2p_with_socket(
        socket: Socket,
        remote: SocketAddr,
        local_handle: usize,
        opts: &Options,
    ) -> Result<Self> {
        let players: Vec<(PlayerType<SocketAddr>, usize)> = (0..2)
            .map(|h| {
                let kind = if h == local_handle {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(remote)
                };
                (kind, h)
            })
            .collect();
        Self::start(socket, &players, local_handle, opts)
    }

    /// Match through a `rugen-server` relay session. `local_handle` is our
    /// slot (0 = P1, 1 = P2); P1 also feeds `spectators` spectator slots.
    pub fn relay(
        socket: RelaySocket,
        local_handle: usize,
        spectators: u8,
        opts: &Options,
    ) -> Result<Self> {
        let mut players: Vec<(PlayerType<SocketAddr>, usize)> = (0..2)
            .map(|h| {
                let kind = if h == local_handle {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(slot_addr(h as u8))
                };
                (kind, h)
            })
            .collect();
        if local_handle == 0 {
            for i in 0..spectators {
                let slot = 2 + i;
                players.push((PlayerType::Spectator(slot_addr(slot)), slot as usize));
            }
        }
        let meter = socket.turn_meter();
        let mut n = Self::start(socket, &players, local_handle, opts)?;
        n.turn_meter = meter;
        Ok(n)
    }

    /// Watch a relayed match: inputs come from P1 (slot 0), confirmed only.
    pub fn spectate(socket: RelaySocket, opts: &Options) -> Result<Self> {
        let session = SessionBuilder::<NetConfig>::new()
            .with_num_players(2)
            .with_max_frames_behind(10)?
            .with_catchup_speed(2)?
            .start_spectator_session(slot_addr(0), socket);
        Ok(Self::wrap(Session::Spectator(Box::new(session)), 0, opts))
    }

    fn wrap(session: Session, local_handle: usize, opts: &Options) -> Self {
        Self {
            session,
            local_handle,
            events: EventFilter::new(opts.max_prediction as u32 + 2),
            quality: Quality::default(),
            stats: Stats {
                input_delay: opts.input_delay,
                ..Default::default()
            },
            skip_frames: 0,
            log: Vec::new(),
            log_start: None,
            checks: Vec::new(),
            turn_meter: None,
        }
    }

    fn start<S: NonBlockingSocket<SocketAddr> + 'static>(
        socket: S,
        players: &[(PlayerType<SocketAddr>, usize)],
        local_handle: usize,
        opts: &Options,
    ) -> Result<Self> {
        let mut b = SessionBuilder::<NetConfig>::new()
            .with_num_players(2)
            .with_input_delay(opts.input_delay)
            .with_max_prediction_window(opts.max_prediction)
            .with_fps(opts.fps)?;
        if opts.desync_interval > 0 {
            b = b.with_desync_detection_mode(DesyncDetection::On {
                interval: opts.desync_interval,
            });
        }
        for (kind, handle) in players {
            b = b.add_player(*kind, *handle)?;
        }
        let session = b.start_p2p_session(socket)?;
        Ok(Self::wrap(
            Session::P2P(Box::new(session)),
            local_handle,
            opts,
        ))
    }

    /// Local session that rolls back `check_distance` frames every tick and
    /// verifies the checksums match: proves the simulation is deterministic.
    pub fn synctest(check_distance: usize, opts: &Options) -> Result<Self> {
        let mut b = SessionBuilder::<NetConfig>::new()
            .with_num_players(2)
            .with_input_delay(opts.input_delay)
            .with_max_prediction_window(opts.max_prediction.max(check_distance + 1))
            .with_check_distance(check_distance);
        for handle in 0..2 {
            b = b.add_player(PlayerType::Local, handle)?;
        }
        let mut n = Self::wrap(
            Session::SyncTest(Box::new(b.start_synctest_session()?)),
            0,
            opts,
        );
        n.events = EventFilter::new(check_distance as u32 + 2);
        n.stats.synchronized = true;
        Ok(n)
    }

    pub fn is_spectator(&self) -> bool {
        matches!(self.session, Session::Spectator(_))
    }

    /// Both players' inputs per frame since the session started (4 bytes per
    /// frame, P1 then P2, little endian), for result reports and replays.
    pub fn input_log_bytes(&self) -> Vec<u8> {
        self.log
            .iter()
            .flat_map(|[a, b]| {
                let mut v = a.to_le_bytes().to_vec();
                v.extend(b.to_le_bytes());
                v
            })
            .collect()
    }

    /// `(frame, world checksum)` every
    /// [`CHECKSUM_EVERY`](rugen_proto::evidence::CHECKSUM_EVERY) frames.
    pub fn checksum_log(&self) -> &[(u32, u64)] {
        &self.checks
    }

    /// Bytes moved through TURN so far (`None`: the session had no `turn`
    /// transport, i.e. the server offers none).
    pub fn turn_stats(&self) -> Option<turn::TurnStats> {
        self.turn_meter
            .as_ref()
            .map(|m| m.lock().map(|s| *s).unwrap_or_default())
    }

    pub fn is_synctest(&self) -> bool {
        matches!(self.session, Session::SyncTest(_))
    }

    pub fn local_handle(&self) -> usize {
        self.local_handle
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Pump the network. Call once per tick before [`advance`](Self::advance).
    pub fn poll(&mut self) {
        match &mut self.session {
            Session::P2P(s) => s.poll_remote_clients(),
            Session::Spectator(s) => s.poll_remote_clients(),
            Session::SyncTest(_) => {}
        }
    }

    /// Feed this tick's local input(s) and advance the session, applying every
    /// save / load / step request to `world`. `inputs[local_handle]` is used
    /// for P2P; sync tests use both.
    pub fn advance(&mut self, world: &mut World, inputs: [Buttons; 2]) -> Result<Advanced> {
        let mut out = Advanced::default();
        if self.skip_frames > 0 {
            self.skip_frames -= 1;
            return Ok(out);
        }
        let requests = match &mut self.session {
            Session::P2P(s) => {
                for ev in s.events() {
                    match ev {
                        GgrsEvent::Synchronizing { total, count, .. } => {
                            out.notices.push(format!("synchronizing {count}/{total}"));
                        }
                        GgrsEvent::Synchronized { addr } => {
                            self.stats.synchronized = true;
                            out.notices.push(format!("connected to {addr}"));
                        }
                        GgrsEvent::Disconnected { addr } => {
                            self.stats.synchronized = false;
                            self.quality.interrupted = true;
                            out.notices.push(format!("{addr} disconnected"));
                        }
                        GgrsEvent::NetworkInterrupted {
                            disconnect_timeout, ..
                        } => {
                            self.quality.interrupted = true;
                            out.notices
                                .push(format!("connection interrupted ({disconnect_timeout} ms)"))
                        }
                        GgrsEvent::NetworkResumed { .. } => {
                            self.quality.interrupted = false;
                            out.notices.push("connection resumed".into())
                        }
                        // time sync: we are ahead of the peer, so pause a few
                        // frames (a brief slowdown instead of big rollbacks)
                        GgrsEvent::WaitRecommendation { skip_frames } => {
                            self.skip_frames = skip_frames;
                        }
                        GgrsEvent::DesyncDetected {
                            frame,
                            local_checksum,
                            remote_checksum,
                            ..
                        } => {
                            return Err(Error::Desync {
                                frame,
                                local: local_checksum,
                                remote: remote_checksum,
                            });
                        }
                    }
                }
                if s.current_state() != SessionState::Running {
                    return Ok(out);
                }
                s.add_local_input(self.local_handle, inputs[self.local_handle].into())?;
                match s.advance_frame() {
                    Ok(r) => r,
                    Err(GgrsError::PredictionThreshold) => {
                        out.notices.push("waiting for remote".into());
                        return Ok(out);
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Session::SyncTest(s) => {
                for (h, b) in inputs.iter().enumerate() {
                    s.add_local_input(h, (*b).into())?;
                }
                s.advance_frame()?
            }
            Session::Spectator(s) => {
                for ev in s.events() {
                    match ev {
                        GgrsEvent::Synchronized { .. } => {
                            self.stats.synchronized = true;
                            out.notices.push("watching".into());
                        }
                        GgrsEvent::Disconnected { .. } => {
                            self.stats.synchronized = false;
                            out.notices.push("the host disconnected".into());
                        }
                        GgrsEvent::Synchronizing { total, count, .. } => {
                            out.notices.push(format!("synchronizing {count}/{total}"));
                        }
                        _ => {}
                    }
                }
                if s.current_state() != SessionState::Running {
                    return Ok(out);
                }
                match s.advance_frame() {
                    Ok(r) => r,
                    Err(GgrsError::PredictionThreshold) => return Ok(out),
                    Err(e) => return Err(e.into()),
                }
            }
        };
        let mut resimulated = 0;
        for req in requests {
            match req {
                GgrsRequest::SaveGameState { cell, frame } => {
                    debug_assert_eq!(frame as u32, world.frame);
                    cell.save(frame, Some(world.clone()), Some(world.checksum() as u128));
                }
                GgrsRequest::LoadGameState { cell, .. } => {
                    if let Some(w) = cell.load() {
                        *world = w;
                        self.stats.rollbacks += 1;
                    }
                }
                GgrsRequest::AdvanceFrame { inputs } => {
                    let b: Vec<Buttons> = inputs.iter().map(|(i, _)| Buttons::from(*i)).collect();
                    let start = *self.log_start.get_or_insert(world.frame);
                    let idx = world.frame.saturating_sub(start) as usize;
                    self.log.truncate(idx);
                    self.log.push([
                        inputs.first().map_or(0, |(i, _)| i.0),
                        inputs.get(1).map_or(0, |(i, _)| i.0),
                    ]);
                    world.step(&b);
                    if world.frame % rugen_proto::evidence::CHECKSUM_EVERY == 0 {
                        let f = world.frame;
                        self.checks.retain(|(g, _)| *g < f);
                        self.checks.push((f, world.checksum()));
                    }
                    if world.frame > self.events.highest() {
                        out.new_frames += 1;
                    } else {
                        resimulated += 1;
                    }
                    out.events
                        .extend(self.events.filter(world.frame, &world.events));
                }
            }
        }
        self.quality.push_tick(resimulated);
        self.stats.avg_rollback = self.quality.avg_rollback();
        if let Session::P2P(s) = &self.session {
            self.stats.frames_ahead = s.frames_ahead();
            if let Some(h) = s.remote_player_handles().first()
                && let Ok(st) = s.network_stats(*h)
            {
                self.stats.ping_ms = st.ping;
            }
            self.stats.unstable = self.quality.unstable(self.stats.frames_ahead);
        }
        Ok(out)
    }
}
