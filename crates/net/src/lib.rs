//! Rollback netplay via ggrs.
//!
//! The simulation ([`World`]) is deterministic, plain data and `Clone`, so
//! ggrs can snapshot it every frame and re-simulate after late inputs arrive.
//! The only thing on the wire is each player's [`NetInput`] (11 button bits).

#![forbid(unsafe_code)]

use std::net::SocketAddr;

use ggrs::{
    DesyncDetection, GgrsError, GgrsEvent, GgrsRequest, P2PSession, PlayerType, SessionBuilder,
    SessionState, SyncTestSession, UdpNonBlockingSocket,
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
}

/// What happened during one call to [`Netplay::advance`].
#[derive(Debug, Default)]
pub struct Advanced {
    /// Frames the authoritative timeline moved forward (0 while stalled / synchronizing).
    pub new_frames: u32,
    /// Sim events from newly reached frames only (re-simulated frames are muted).
    pub events: Vec<Event>,
    /// Human-readable session notices (sync progress, interruptions).
    pub notices: Vec<String>,
}

pub struct Netplay {
    session: Session,
    local_handle: usize,
    /// Highest frame the world has ever reached; events from frames at or below are rollback noise.
    highest_frame: u32,
    stats: Stats,
    skip_frames: u32,
}

impl Netplay {
    /// Peer-to-peer session over UDP. `local_handle` is 0 for P1, 1 for P2.
    pub fn p2p(
        local_port: u16,
        remote: SocketAddr,
        local_handle: usize,
        opts: &Options,
    ) -> Result<Self> {
        let socket = UdpNonBlockingSocket::bind_to_port(local_port)?;
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
        for handle in 0..2 {
            let kind = if handle == local_handle {
                PlayerType::Local
            } else {
                PlayerType::Remote(remote)
            };
            b = b.add_player(kind, handle)?;
        }
        let session = b.start_p2p_session(socket)?;
        Ok(Self {
            session: Session::P2P(Box::new(session)),
            local_handle,
            highest_frame: 0,
            stats: Stats::default(),
            skip_frames: 0,
        })
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
        Ok(Self {
            session: Session::SyncTest(Box::new(b.start_synctest_session()?)),
            local_handle: 0,
            highest_frame: 0,
            stats: Stats {
                synchronized: true,
                ..Default::default()
            },
            skip_frames: 0,
        })
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
        if let Session::P2P(s) = &mut self.session {
            s.poll_remote_clients();
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
                            out.notices.push(format!("{addr} disconnected"));
                        }
                        GgrsEvent::NetworkInterrupted {
                            disconnect_timeout, ..
                        } => out
                            .notices
                            .push(format!("connection interrupted ({disconnect_timeout} ms)")),
                        GgrsEvent::NetworkResumed { .. } => {
                            out.notices.push("connection resumed".into())
                        }
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
        };
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
                    world.step(&b);
                    if world.frame > self.highest_frame {
                        self.highest_frame = world.frame;
                        out.new_frames += 1;
                        out.events.extend(world.events.iter().cloned());
                    }
                }
            }
        }
        if let Session::P2P(s) = &self.session {
            self.stats.frames_ahead = s.frames_ahead();
            if let Some(h) = s.remote_player_handles().first()
                && let Ok(st) = s.network_stats(*h)
            {
                self.stats.ping_ms = st.ping;
            }
        }
        Ok(out)
    }
}
