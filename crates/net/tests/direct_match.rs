//! Rollback matches over the direct peer-to-peer path, signalled through a
//! loopback relay (a minimal stand-in for rugen-server's UDP relay):
//!
//! * the direct path is chosen and the match stays in sync, forged
//!   datagrams on the direct socket are dropped;
//! * the direct path breaks mid-match: the match falls back to the relay
//!   at once and stays in sync;
//! * a direct path that comes up mid-match takes over (hysteresis);
//! * a player with direct connections off: no candidates, relayed;
//! * with a TURN service (`RUGEN_TEST_TURN`, coturn as in `turn_relay.rs`):
//!   a private player plays over TURN, and a direct path that breaks falls
//!   back to TURN rather than to the relay.
//!
//! Skipped when the reference tree (KFM) is absent.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rugen_net::{DirectConfig, Netplay, Options, PathKind, RelaySocket};
use rugen_proto::relay::{self as wire, Header};
use rugen_proto::{RelayTransport, SessionTicket};
use rugen_sim::{Buttons, CharData, Stage, World};

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

fn token(slot: usize) -> [u8; 16] {
    [slot as u8 + 7; 16]
}

/// Forwards DATA by slot after checking tokens; answers HELLO / PING.
fn relay(session: u32) -> SocketAddr {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut routes: [Option<SocketAddr>; 2] = [None; 2];
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let Some((h, t, p)) = wire::decode_client(&buf[..n]) else {
                continue;
            };
            if h.session != session || (h.from as usize) >= 2 || t != token(h.from as usize) {
                continue;
            }
            routes[h.from as usize] = Some(from);
            let mask = routes
                .iter()
                .enumerate()
                .fold(0u8, |m, (i, r)| m | (u8::from(r.is_some()) << i));
            let reply = |kind, to: u8, p: &[u8]| {
                wire::encode_server(
                    Header {
                        kind,
                        from: 0xFF,
                        to,
                        session,
                    },
                    p,
                )
            };
            match h.kind {
                wire::HELLO => {
                    let _ = s.send_to(&reply(wire::ACK, h.from, &[mask]), from);
                    if let Some(o) = routes[1 - h.from as usize] {
                        let _ = s.send_to(&reply(wire::PEERS, 1 - h.from, &[mask]), o);
                    }
                }
                wire::PING => {
                    let _ = s.send_to(&reply(wire::PONG, h.from, p), from);
                }
                wire::DATA => {
                    if let Some(Some(to)) = routes.get(h.to as usize) {
                        let _ = s.send_to(&wire::encode_server(h, p), to);
                    }
                }
                _ => {}
            }
        }
    });
    addr
}

fn world(root: &std::path::Path) -> World {
    let data = CharData::load_with_data(&root.join("chars/kfm/kfm.def"), &root.join("data"))
        .expect("kfm loads");
    let mut w = World::new(Stage::default(), 7);
    w.add_player(data.clone(), 0, 1);
    w.add_player(data, 1, 2);
    w.start_round();
    w
}

/// The coturn of `turn_relay.rs`, when `RUGEN_TEST_TURN` is set.
fn turn_service() -> Option<RelayTransport> {
    let host = std::env::var("RUGEN_TEST_TURN").ok()?;
    let user = std::env::var("RUGEN_TEST_TURN_USER").unwrap_or_else(|_| "u".into());
    let pass = std::env::var("RUGEN_TEST_TURN_PASS").unwrap_or_else(|_| "p".into());
    Some(RelayTransport::turn(
        vec![format!("turn:{host}?transport=udp")],
        &user,
        &pass,
        0,
    ))
}

fn ticket(server: SocketAddr, session: u32, slot: usize, turn: bool) -> SessionTicket {
    SessionTicket {
        session,
        slot: slot as u8,
        token: rugen_proto::to_hex(&token(slot)),
        spectators: 0,
        udp: Some(server.to_string()),
        transports: if turn {
            turn_service().into_iter().collect()
        } else {
            Vec::new()
        },
        live: false,
    }
}

fn loopback(allowed: bool) -> DirectConfig {
    DirectConfig {
        allowed,
        host_only: true,
        loopback: true,
        ..Default::default()
    }
}

#[derive(Clone, Copy, Default)]
struct Plan {
    /// Direct allowed (`None`: direct never enabled on this side).
    direct: Option<bool>,
    /// Enable direct only after the match started (mid-match switch).
    late: bool,
    /// Break the direct path at this frame.
    sever_at: Option<u32>,
    /// Inject this many forged datagrams at frame 100.
    forge: u32,
    /// The ticket carries the test TURN service.
    turn: bool,
}

struct Outcome {
    frames: u32,
    log: Vec<u8>,
    start_path: PathKind,
    paths: Vec<(u32, PathKind)>,
    forged: u64,
}

fn play(root: PathBuf, server: SocketAddr, session: u32, slot: usize, plan: Plan) -> Outcome {
    let target = 600u32;
    let mut w = world(&root);
    let mut sock = RelaySocket::connect_udp(
        &ticket(server, session, slot, plan.turn),
        &server.to_string(),
        Duration::from_secs(3),
    )
    .unwrap();
    if let (Some(allowed), false) = (plan.direct, plan.late) {
        sock.enable_direct(&loopback(allowed));
    }
    sock.wait_for(0b11, Duration::from_secs(10), &mut |_| {})
        .unwrap();
    let start_path = sock.settle_paths(Duration::from_millis(1500));
    if let (Some(allowed), true) = (plan.direct, plan.late) {
        sock.enable_direct(&loopback(allowed));
    }
    let hooks = sock.hooks();
    let opts = Options {
        input_delay: 1,
        ..Default::default()
    };
    let mut net = Netplay::relay(sock, slot, 0, &opts).unwrap();
    let mut seed: u32 = 0x9E37_79B9 ^ slot as u32;
    let start = Instant::now();
    let mut paths = vec![(0, start_path)];
    while w.frame < target + 40 && start.elapsed() < Duration::from_secs(60) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let mine = Buttons::from_bits((seed & 0x7ff) as u16);
        if plan.sever_at == Some(w.frame) {
            hooks.sever_direct.store(true, Ordering::Relaxed);
        }
        if plan.forge > 0 && w.frame == 100 {
            hooks.forge.store(plan.forge, Ordering::Relaxed);
        }
        net.poll();
        net.advance(&mut w, [mine, mine]).expect("no desync");
        let p = net.stats().path;
        if paths.last().is_none_or(|(_, last)| *last != p) {
            paths.push((w.frame, p));
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let end = Instant::now() + Duration::from_millis(800);
    while Instant::now() < end {
        net.poll();
        let _ = net.advance(&mut w, [Buttons::default(); 2]);
        std::thread::sleep(Duration::from_millis(4));
    }
    Outcome {
        frames: w.frame,
        log: net.input_log_bytes(),
        start_path,
        paths,
        forged: net.link().map_or(0, |l| l.forged),
    }
}

fn run(session: u32, a: Plan, b: Plan) -> Option<(Outcome, Outcome)> {
    let _ = env_logger::builder().is_test(true).try_init();
    let root = reference()?;
    let server = relay(session);
    let (r1, r2) = (root.clone(), root);
    let ta = std::thread::spawn(move || play(r1, server, session, 0, a));
    let tb = std::thread::spawn(move || play(r2, server, session, 1, b));
    let (oa, ob) = (ta.join().unwrap(), tb.join().unwrap());
    assert!(
        oa.frames >= 600 && ob.frames >= 600,
        "{} / {}",
        oa.frames,
        ob.frames
    );
    let n = (600 - 20) * 4;
    assert_eq!(oa.log[..n], ob.log[..n], "input streams diverged");
    Some((oa, ob))
}

#[test]
fn direct_path_is_chosen_and_forged_datagrams_are_dropped() {
    let on = Plan {
        direct: Some(true),
        forge: 40,
        ..Default::default()
    };
    let Some((a, b)) = run(0xD1, on, on) else {
        eprintln!("reference tree missing; skipping");
        return;
    };
    assert_eq!(a.start_path, PathKind::Direct);
    assert_eq!(b.start_path, PathKind::Direct);
    assert!(
        a.paths.iter().all(|(_, p)| *p == PathKind::Direct),
        "{:?}",
        a.paths
    );
    // each side forged 40 datagrams at the other: all dropped, match in sync
    assert!(
        a.forged >= 40 && b.forged >= 40,
        "{} / {}",
        a.forged,
        b.forged
    );
}

#[test]
fn broken_direct_path_falls_back_to_the_relay_mid_match() {
    let a = Plan {
        direct: Some(true),
        sever_at: Some(250),
        ..Default::default()
    };
    let b = Plan {
        direct: Some(true),
        ..Default::default()
    };
    let Some((a, b)) = run(0xD2, a, b) else {
        return;
    };
    assert_eq!(a.start_path, PathKind::Direct);
    for o in [&a, &b] {
        let last = o.paths.last().unwrap();
        assert_eq!(last.1, PathKind::Relay, "{:?}", o.paths);
        assert!(last.0 >= 250, "{:?}", o.paths);
    }
}

#[test]
fn direct_path_that_comes_up_mid_match_takes_over() {
    let a = Plan {
        direct: Some(true),
        ..Default::default()
    };
    let b = Plan {
        direct: Some(true),
        late: true,
        ..Default::default()
    };
    let Some((a, b)) = run(0xD3, a, b) else {
        return;
    };
    assert_eq!(b.start_path, PathKind::Relay);
    for o in [&a, &b] {
        assert_eq!(o.paths.last().unwrap().1, PathKind::Direct, "{:?}", o.paths);
    }
}

#[test]
fn a_private_player_stays_relayed() {
    let a = Plan {
        direct: Some(true),
        ..Default::default()
    };
    let b = Plan {
        direct: Some(false),
        ..Default::default()
    };
    let Some((a, b)) = run(0xD4, a, b) else {
        return;
    };
    for o in [&a, &b] {
        assert!(
            o.paths.iter().all(|(_, p)| *p == PathKind::Relay),
            "{:?}",
            o.paths
        );
    }
}

#[test]
fn a_private_player_plays_over_turn() {
    if turn_service().is_none() {
        eprintln!("RUGEN_TEST_TURN not set; skipping");
        return;
    }
    let a = Plan {
        direct: Some(true),
        turn: true,
        ..Default::default()
    };
    let b = Plan {
        direct: Some(false),
        turn: true,
        ..Default::default()
    };
    let Some((a, b)) = run(0xD5, a, b) else {
        return;
    };
    for o in [&a, &b] {
        assert_eq!(o.start_path, PathKind::Turn, "{:?}", o.paths);
        assert!(
            o.paths.iter().all(|(_, p)| *p == PathKind::Turn),
            "{:?}",
            o.paths
        );
    }
}

#[test]
fn broken_direct_path_falls_back_to_turn() {
    if turn_service().is_none() {
        eprintln!("RUGEN_TEST_TURN not set; skipping");
        return;
    }
    let a = Plan {
        direct: Some(true),
        turn: true,
        sever_at: Some(250),
        ..Default::default()
    };
    let b = Plan {
        direct: Some(true),
        turn: true,
        ..Default::default()
    };
    let Some((a, b)) = run(0xD6, a, b) else {
        return;
    };
    assert_eq!(a.start_path, PathKind::Direct);
    for o in [&a, &b] {
        let last = o.paths.last().unwrap();
        assert_eq!(last.1, PathKind::Turn, "{:?}", o.paths);
        assert!(last.0 >= 250, "{:?}", o.paths);
    }
}
