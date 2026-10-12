//! Two rollback sessions and a spectator play KFM through a relay on
//! loopback (a minimal stand-in for rugen-server's UDP relay): the players
//! stay in sync (ggrs desync detection on), their confirmed input streams
//! agree, and the spectator follows. Skipped when the reference tree is absent.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rugen_net::{Netplay, Options, RelaySocket};
use rugen_proto::SessionTicket;
use rugen_proto::relay::{self as wire, Header};
use rugen_sim::{Buttons, CharData, Stage, World};

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

const SESSION: u32 = 0xC0FFEE;
const SLOTS: usize = 3;

fn token(slot: usize) -> [u8; 16] {
    [slot as u8 + 1; 16]
}

/// Forwards DATA by slot after checking tokens; answers HELLO / PING.
fn relay() -> SocketAddr {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut routes: [Option<SocketAddr>; SLOTS] = [None; SLOTS];
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let Some((h, t, p)) = wire::decode_client(&buf[..n]) else {
                continue;
            };
            if h.session != SESSION || (h.from as usize) >= SLOTS || t != token(h.from as usize) {
                continue;
            }
            routes[h.from as usize] = Some(from);
            let mask = routes
                .iter()
                .enumerate()
                .fold(0u8, |m, (i, r)| m | (u8::from(r.is_some()) << i));
            let reply = |kind, p: &[u8]| {
                wire::encode_server(
                    Header {
                        kind,
                        from: 0xFF,
                        to: h.from,
                        session: SESSION,
                    },
                    p,
                )
            };
            match h.kind {
                wire::HELLO => {
                    let _ = s.send_to(&reply(wire::ACK, &[mask]), from);
                    for (i, r) in routes.iter().enumerate() {
                        if i != h.from as usize
                            && let Some(r) = r
                        {
                            let _ = s.send_to(&reply(wire::PEERS, &[mask]), r);
                        }
                    }
                }
                wire::PING => {
                    let _ = s.send_to(&reply(wire::PONG, p), from);
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

fn ticket(server: SocketAddr, slot: usize) -> SessionTicket {
    SessionTicket {
        session: SESSION,
        slot: slot as u8,
        token: rugen_proto::to_hex(&token(slot)),
        spectators: 1,
        udp: Some(server.to_string()),
        transports: Vec::new(),
    }
}

#[test]
fn relayed_rollback_match_stays_in_sync() {
    let Some(root) = reference() else {
        eprintln!("reference tree missing; skipping");
        return;
    };
    let server = relay();
    let target = 300u32;
    let player = |slot: usize| {
        let root = root.clone();
        std::thread::spawn(move || {
            let mut w = world(&root);
            let mut sock = RelaySocket::connect_udp(
                &ticket(server, slot),
                &server.to_string(),
                Duration::from_secs(3),
            )
            .unwrap();
            sock.wait_for(0b111, Duration::from_secs(10), &mut |_| {})
                .unwrap();
            let rtt = sock.peer_rtt(1 - slot as u8, 5, Duration::from_secs(2));
            let opts = Options {
                input_delay: rugen_net::auto_input_delay(rtt.unwrap_or(0)),
                ..Default::default()
            };
            let mut net = Netplay::relay(sock, slot, 1, &opts).unwrap();
            let mut seed: u32 = 0x9E37_79B9 ^ slot as u32;
            let start = Instant::now();
            while w.frame < target + 40 && start.elapsed() < Duration::from_secs(60) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let mine = Buttons::from_bits((seed & 0x7ff) as u16);
                net.poll();
                net.advance(&mut w, [mine, mine]).expect("no desync");
                std::thread::sleep(Duration::from_millis(4));
            }
            // keep serving the peer / spectator a little longer
            let end = Instant::now() + Duration::from_millis(800);
            while Instant::now() < end {
                net.poll();
                let _ = net.advance(&mut w, [Buttons::default(); 2]);
                std::thread::sleep(Duration::from_millis(4));
            }
            (w.frame, net.input_log_bytes(), rtt)
        })
    };
    let spectator = {
        let root = root.clone();
        std::thread::spawn(move || {
            let mut w = world(&root);
            let mut sock = RelaySocket::connect_udp(
                &ticket(server, 2),
                &server.to_string(),
                Duration::from_secs(3),
            )
            .unwrap();
            sock.wait_for(0b111, Duration::from_secs(10), &mut |_| {})
                .unwrap();
            let mut net = Netplay::spectate(sock, &Options::default()).unwrap();
            let start = Instant::now();
            while w.frame < target / 2 && start.elapsed() < Duration::from_secs(60) {
                net.poll();
                if let Err(e) = net.advance(&mut w, [Buttons::default(); 2]) {
                    eprintln!("spectator: {e}");
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            w.frame
        })
    };
    let a = player(0);
    let b = player(1);
    let (fa, la, rtt) = a.join().unwrap();
    let (fb, lb, _) = b.join().unwrap();
    let fs = spectator.join().unwrap();
    assert!(fa >= target && fb >= target, "frames {fa} / {fb}");
    assert!(rtt.is_some(), "peer round trip measured through the relay");
    // confirmed history is identical on both sides
    let n = (target as usize - 20) * 4;
    assert_eq!(la[..n], lb[..n], "input streams diverged");
    assert!(fs >= target / 2, "spectator reached frame {fs}");
}
