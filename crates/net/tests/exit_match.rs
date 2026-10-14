//! A match that ends early, over a loopback relay (a minimal stand-in for
//! rugen-server's UDP relay that, like the real one, announces a slot's
//! BYE to the others):
//!
//! * P1 flees from its menu: P2 learns it from the flee message;
//! * P1 quits without a word (its socket is dropped): P2 learns it from the
//!   relay's announcement of P1's BYE;
//! * P1 goes silent on every path: P2 declares it lost after the
//!   disconnect timeout (shortened here), not before.
//!
//! Skipped when the reference tree (KFM) is absent.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rugen_net::{Netplay, Options, PeerExit, RelaySocket};
use rugen_proto::SessionTicket;
use rugen_proto::relay::{self as wire, Header};
use rugen_sim::{Buttons, CharData, Stage, World};

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

fn token(slot: usize) -> [u8; 16] {
    [slot as u8 + 3; 16]
}

/// Forwards DATA by slot, answers HELLO / PING, announces BYE; `mute`
/// drops everything slot 0 sends (its connection died).
fn relay(session: u32, mute: Arc<AtomicBool>) -> SocketAddr {
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
            if h.from == 0 && mute.load(Ordering::Relaxed) {
                continue;
            }
            let bye = h.kind == wire::BYE;
            routes[h.from as usize] = if bye { None } else { Some(from) };
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
            let announce = |s: &UdpSocket| {
                for (i, r) in routes.iter().enumerate() {
                    if i != h.from as usize
                        && let Some(r) = r
                    {
                        let _ = s.send_to(&reply(wire::PEERS, i as u8, &[mask]), r);
                    }
                }
            };
            match h.kind {
                wire::HELLO => {
                    let _ = s.send_to(&reply(wire::ACK, h.from, &[mask]), from);
                    announce(&s);
                }
                wire::BYE => announce(&s),
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

#[derive(Clone, Copy, PartialEq)]
enum Plan {
    Flee,
    Quit,
    GoSilent,
}

/// Play until frame 120, then P1 leaves the way `plan` says; P2 plays on
/// until it notices. Returns what P2 saw, how long after P1 left, and what
/// P1 itself saw of its peer.
fn play(plan: Plan, session: u32) -> (Option<PeerExit>, Duration, Option<PeerExit>) {
    let root = reference().unwrap();
    let mute = Arc::new(AtomicBool::new(false));
    let server = relay(session, mute.clone());
    let left_at = Arc::new(std::sync::Mutex::new(None::<Instant>));
    let player = |slot: usize| {
        let root = root.clone();
        let mute = mute.clone();
        let left_at = left_at.clone();
        std::thread::spawn(move || {
            let mut w = world(&root);
            let ticket = SessionTicket {
                session,
                slot: slot as u8,
                token: rugen_proto::to_hex(&token(slot)),
                spectators: 0,
                udp: Some(server.to_string()),
                transports: Vec::new(),
                live: false,
            };
            let mut sock =
                RelaySocket::connect_udp(&ticket, &server.to_string(), Duration::from_secs(3))
                    .unwrap();
            sock.wait_for(0b11, Duration::from_secs(10), &mut |_| {})
                .unwrap();
            let opts = Options {
                disconnect_timeout: Duration::from_millis(1500),
                ..Default::default()
            };
            let mut net = Netplay::relay(sock, slot, 0, &opts).unwrap();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(30) {
                net.poll();
                net.advance(&mut w, [Buttons::default(); 2])
                    .expect("no desync");
                if slot == 0 && w.frame >= 120 {
                    *left_at.lock().unwrap() = Some(Instant::now());
                    let mine = net.peer_exit();
                    match plan {
                        Plan::Flee => net.flee(),
                        Plan::Quit => {}
                        Plan::GoSilent => {
                            // the connection dies: nothing in, nothing out
                            mute.store(true, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_secs(4));
                        }
                    }
                    return (mine, Duration::ZERO);
                }
                if slot == 1
                    && let Some(e) = net.peer_exit()
                {
                    let since = left_at
                        .lock()
                        .unwrap()
                        .map_or(Duration::ZERO, |t| t.elapsed());
                    return (Some(e), since);
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            (None, Duration::ZERO)
        })
    };
    let a = player(0);
    let b = player(1);
    let (mine, _) = a.join().unwrap();
    let (seen, after) = b.join().unwrap();
    (seen, after, mine)
}

#[test]
fn a_flee_reaches_the_other_player_at_once() {
    if reference().is_none() {
        eprintln!("reference tree missing; skipping");
        return;
    }
    let (seen, after, mine) = play(Plan::Flee, 0xF1EE);
    assert_eq!(mine, None, "the fleer's peer is still there");
    assert_eq!(seen, Some(PeerExit::Fled));
    assert!(after < Duration::from_secs(1), "noticed after {after:?}");
}

#[test]
fn a_silent_quit_is_announced_by_the_relay() {
    if reference().is_none() {
        eprintln!("reference tree missing; skipping");
        return;
    }
    let (seen, after, _) = play(Plan::Quit, 0xB1E5);
    assert_eq!(seen, Some(PeerExit::Left));
    assert!(after < Duration::from_secs(1), "noticed after {after:?}");
}

#[test]
fn a_dead_connection_is_lost_after_the_timeout() {
    if reference().is_none() {
        eprintln!("reference tree missing; skipping");
        return;
    }
    let (seen, after, _) = play(Plan::GoSilent, 0xDEAD);
    assert_eq!(seen, Some(PeerExit::Lost));
    assert!(
        after >= Duration::from_millis(1400) && after < Duration::from_secs(4),
        "lost after {after:?}"
    );
}
