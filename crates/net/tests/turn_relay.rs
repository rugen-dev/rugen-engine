//! TURN against a real TURN server (coturn in Docker on loopback):
//!
//! ```text
//! docker run -d --name rugen-coturn --network host coturn/coturn -n \
//!   --listening-port=3478 --listening-ip=127.0.0.1 --relay-ip=127.0.0.1 \
//!   --realm=rugen --user=u:p --lt-cred-mech --no-tls --allow-loopback-peers \
//!   --min-port=49160 --max-port=49260 --no-cli
//! RUGEN_TEST_TURN=127.0.0.1:3478 RUGEN_TEST_TURN_USER=u RUGEN_TEST_TURN_PASS=p \
//!   cargo test -p rugen-net --test turn_relay -- --nocapture --test-threads=1
//! ```
//!
//! Skipped (passing) when `RUGEN_TEST_TURN` is not set. The match test also
//! needs the reference tree (KFM). It prints round trips and the bytes per
//! second each player moves through TURN, compact vs plain encoding.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rugen_net::turn::{TurnClient, TurnCredentials};
use rugen_net::{Netplay, Options, RelaySocket};
use rugen_proto::relay::{self as wire, Header};
use rugen_proto::{RelayTransport, SessionTicket};
use rugen_sim::{Buttons, CharData, Stage, World};

fn server() -> Option<(String, TurnCredentials)> {
    let host = std::env::var("RUGEN_TEST_TURN").ok()?;
    let user = std::env::var("RUGEN_TEST_TURN_USER").unwrap_or_else(|_| "u".into());
    let pass = std::env::var("RUGEN_TEST_TURN_PASS").unwrap_or_else(|_| "p".into());
    let creds = TurnCredentials {
        urls: vec![
            format!("turn:{host}?transport=udp"),
            format!("turn:{host}?transport=tcp"),
        ],
        username: user,
        credential: pass,
    };
    Some((host, creds))
}

fn only(creds: &TurnCredentials, tcp: bool) -> TurnCredentials {
    let mut c = creds.clone();
    c.urls.retain(|u| u.ends_with("tcp") == tcp);
    c
}

fn summary(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "no samples".into();
    }
    let jitter =
        v.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (v.len().max(2) - 1) as f64;
    v.sort_by(f64::total_cmp);
    let p = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
    format!(
        "n={} median {:.3} ms, p95 {:.3} ms, max {:.3} ms, jitter {:.3} ms",
        v.len(),
        p(0.5),
        p(0.95),
        p(1.0),
        jitter
    )
}

/// Round trips between two allocations through the TURN server.
fn echo_rtts(a: &mut TurnClient, b: &mut TurnClient, n: usize) -> Vec<f64> {
    let (pa, pb) = (a.relayed(), b.relayed());
    let mut out = Vec::new();
    let mut ra = Vec::new();
    let mut rb = Vec::new();
    for i in 0..n {
        let t = Instant::now();
        assert!(a.send(pb, &(i as u32).to_be_bytes()));
        'wait: while t.elapsed() < Duration::from_millis(500) {
            rb.clear();
            b.poll(&mut rb);
            for (from, d) in rb.drain(..) {
                assert_eq!(from, pa);
                b.send(pa, &d);
            }
            ra.clear();
            a.poll(&mut ra);
            for (_, d) in ra.drain(..) {
                if d == (i as u32).to_be_bytes() {
                    out.push(t.elapsed().as_secs_f64() * 1000.0);
                    break 'wait;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

#[test]
fn two_allocations_relay_to_each_other() {
    let Some((_, creds)) = server() else {
        eprintln!("RUGEN_TEST_TURN not set; skipping");
        return;
    };
    for tcp in [false, true] {
        let c = only(&creds, tcp);
        let mut a = TurnClient::allocate(&c, Duration::from_secs(3)).expect("allocate a");
        let mut b = TurnClient::allocate(&c, Duration::from_secs(3)).expect("allocate b");
        assert_ne!(a.relayed(), b.relayed());
        a.bind_peer(b.relayed(), Duration::from_secs(2))
            .expect("bind a→b");
        b.bind_peer(a.relayed(), Duration::from_secs(2))
            .expect("bind b→a");
        let rtts = echo_rtts(&mut a, &mut b, 200);
        assert!(rtts.len() >= 190, "lost too many: {}", rtts.len());
        eprintln!("{}: {}", a.proto().name(), summary(rtts));
        // refreshes and keep-alives do not disturb the channel
        a.tick();
        b.tick();
        let s = a.stats();
        assert!(s.tx_packets >= 200 && s.rx_packets >= 190);
    }
    // a wrong password is refused, not hung on
    let mut bad = only(&creds, false);
    bad.credential = "wrong".into();
    let t = Instant::now();
    assert!(TurnClient::allocate(&bad, Duration::from_secs(2)).is_err());
    assert!(t.elapsed() < Duration::from_secs(3));
}

// ---- a full rollback match through TURN, signalled over a loopback relay

const SESSION: u32 = 0x7E57;

fn token(slot: usize) -> [u8; 16] {
    [slot as u8 + 9; 16]
}

/// Minimal relay (as in relay_match.rs) that also counts what it forwards.
fn relay(forwarded: Arc<AtomicU64>) -> SocketAddr {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut routes: [Option<SocketAddr>; 2] = [None; 2];
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let Some((h, t, p)) = wire::decode_client(&buf[..n]) else {
                continue;
            };
            if h.session != SESSION || h.from > 1 || t != token(h.from as usize) {
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
                    if let Some(o) = routes[1 - h.from as usize] {
                        let _ = s.send_to(&reply(wire::PEERS, &[mask]), o);
                    }
                }
                wire::DATA => {
                    if let Some(Some(to)) = routes.get(h.to as usize) {
                        let f = wire::encode_server(h, p);
                        forwarded.fetch_add(f.len() as u64 + 28, Ordering::Relaxed);
                        let _ = s.send_to(&f, to);
                    }
                }
                _ => {}
            }
        }
    });
    addr
}

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
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

struct Run {
    frames: u32,
    log: Vec<u8>,
    transport: &'static str,
    turn_rtt: Option<u32>,
    tx_wire: u64,
    rx_wire: u64,
    tx_packets: u64,
    secs: f64,
}

/// Two players, `frames` frames at 60 fps through TURN; `turn = false`
/// keeps them on the relay (for comparison).
fn play(
    root: &std::path::Path,
    creds: &TurnCredentials,
    frames: u32,
    turn: bool,
    plain: bool,
) -> ([Run; 2], u64) {
    let forwarded = Arc::new(AtomicU64::new(0));
    let server = relay(forwarded.clone());
    let player = |slot: usize| {
        let root = root.to_path_buf();
        let creds = creds.clone();
        std::thread::spawn(move || {
            let mut t = SessionTicket {
                session: SESSION,
                slot: slot as u8,
                token: rugen_proto::to_hex(&token(slot)),
                udp: Some(server.to_string()),
                ..Default::default()
            };
            if turn {
                t.transports.push(RelayTransport::turn(
                    creds.urls.clone(),
                    &creds.username,
                    &creds.credential,
                    0,
                ));
            }
            let mut w = world(&root);
            let mut sock =
                RelaySocket::connect_udp(&t, &server.to_string(), Duration::from_secs(3)).unwrap();
            sock.set_turn_plain(plain);
            sock.wait_for(0b11, Duration::from_secs(10), &mut |_| {})
                .unwrap();
            let on_turn = sock.settle_turn(Duration::from_millis(1500));
            assert_eq!(on_turn, turn, "TURN path state");
            let transport = sock.transport_name();
            let turn_rtt = sock.turn_rtt();
            let rtt = sock.peer_rtt(1 - slot as u8, 5, Duration::from_secs(2));
            let meter = sock.turn_meter();
            let opts = Options {
                input_delay: rugen_net::auto_input_delay(rtt.unwrap_or(0)),
                ..Default::default()
            };
            let before = meter
                .as_ref()
                .map(|m| *m.lock().unwrap())
                .unwrap_or_default();
            let mut net = Netplay::relay(sock, slot, 0, &opts).unwrap();
            let mut seed: u32 = 0x1234_5678 ^ slot as u32;
            let mut held = Buttons::default();
            let start = Instant::now();
            let tick = Duration::from_micros(16_667);
            let mut next = Instant::now();
            while w.frame < frames + 30 && start.elapsed() < Duration::from_secs(120) {
                // a new random input every 4-11 frames (people hold buttons)
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                if seed % 8 == 0 {
                    held = Buttons::from_bits(((seed >> 8) & 0x7ff) as u16);
                }
                net.poll();
                net.advance(&mut w, [held, held]).expect("no desync");
                next += tick;
                if let Some(d) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            let secs = start.elapsed().as_secs_f64();
            let after = meter
                .as_ref()
                .map(|m| *m.lock().unwrap())
                .unwrap_or_default();
            let end = Instant::now() + Duration::from_millis(600);
            while Instant::now() < end {
                net.poll();
                let _ = net.advance(&mut w, [Buttons::default(); 2]);
                std::thread::sleep(Duration::from_millis(8));
            }
            Run {
                frames: w.frame,
                log: net.input_log_bytes(),
                transport,
                turn_rtt,
                tx_wire: after.tx_wire - before.tx_wire,
                rx_wire: after.rx_wire - before.rx_wire,
                tx_packets: after.tx_packets - before.tx_packets,
                secs,
            }
        })
    };
    let a = player(0);
    let b = player(1);
    let (ra, rb) = (a.join().unwrap(), b.join().unwrap());
    ([ra, rb], forwarded.load(Ordering::Relaxed))
}

#[test]
fn rollback_match_over_turn_stays_in_sync() {
    let (Some((_, creds)), Some(root)) = (server(), reference()) else {
        eprintln!("RUGEN_TEST_TURN or the reference tree missing; skipping");
        return;
    };
    let creds = only(&creds, false);
    let frames: u32 = std::env::var("RUGEN_TEST_TURN_FRAMES")
        .ok()
        .and_then(|f| f.parse().ok())
        .unwrap_or(600);
    let mut report = Vec::new();
    for (label, turn, plain) in [
        ("turn compact", true, false),
        ("turn plain", true, true),
        ("server relay", false, false),
    ] {
        let ([a, b], relayed) = play(&root, &creds, frames, turn, plain);
        assert!(a.frames >= frames && b.frames >= frames);
        let n = (frames as usize - 20) * 4;
        assert_eq!(a.log[..n], b.log[..n], "{label}: input streams diverged");
        if turn {
            assert!(a.transport.starts_with("TURN"), "{}", a.transport);
            // match traffic left the server relay (only keep-alives remain)
            assert!(
                relayed < 2000,
                "{label}: {relayed} bytes still went through the relay"
            );
            let per_s = |r: &Run| (r.rx_wire as f64) / r.secs;
            report.push(format!(
                "{label}: setup rtt {:?} ms; per player received {:.0} B/s ({:.1} datagrams/s sent), \
                 99 s match ≈ {:.0} KB per player per direction",
                a.turn_rtt,
                (per_s(&a) + per_s(&b)) / 2.0,
                (a.tx_packets as f64 / a.secs + b.tx_packets as f64 / b.secs) / 2.0,
                (per_s(&a) + per_s(&b)) / 2.0 * 99.0 / 1000.0,
            ));
            let _ = (a.tx_wire, b.tx_wire);
        } else {
            let per_s = relayed as f64 / 2.0 / a.secs;
            report.push(format!(
                "{label}: per player received {per_s:.0} B/s (UDP relay framing), 99 s ≈ {:.0} KB",
                per_s * 99.0 / 1000.0
            ));
        }
    }
    for r in report {
        eprintln!("{r}");
    }
}
