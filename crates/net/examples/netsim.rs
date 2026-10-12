//! High-ping simulation: two rollback peers on this machine play a match
//! through an in-process UDP proxy that delays every datagram (round trip,
//! jitter) and drops some, so the netcode can be measured at any distance.
//!
//! ```text
//! cargo run --release -p rugen-net --example netsim -- \
//!     --p1 chars/a.def --p2 chars/b.def --data data/ --replays replays/ \
//!     --rtt 180 [--jitter 8] [--loss 1] [--secs 40] [--warmup 5]
//! ```
//!
//! Inputs are recorded human inputs (`rugen` replay JSON files in
//! `--replays`, P1 and P2 streams), so the remote input is as predictable
//! as a real player's. Each peer runs its own 60 Hz loop (stretched by the
//! session's time dilation) and a separate exact 60 Hz "display" that
//! samples the newest simulated frame, like the game's render loop.
//!
//! Reported per peer after the warm-up: input delay chosen, rollbacks per
//! second, mean / max rollback depth, hard stalls (ticks the prediction
//! window was full) and skipped ticks (wait recommendations), displayed
//! frames per second (distinct frames shown per 60 refreshes), the longest
//! freeze in refreshes, the misprediction rate, and the netcode's CPU time
//! per tick.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rugen_net::{Netplay, Options, Socket};
use rugen_sim::{Buttons, CharData, Stage, World};

struct Args {
    p1: PathBuf,
    p2: PathBuf,
    data: PathBuf,
    replays: PathBuf,
    /// Busy random inputs instead of the recorded ones (stress).
    random: bool,
    rtt: f64,
    jitter: f64,
    loss: f64,
    secs: f64,
    warmup: f64,
}

fn args() -> Args {
    let v: Vec<String> = std::env::args().collect();
    let get = |k: &str| {
        v.iter()
            .position(|a| a == k)
            .and_then(|i| v.get(i + 1))
            .cloned()
    };
    let num = |k: &str, d: f64| get(k).and_then(|s| s.parse().ok()).unwrap_or(d);
    let path = |k: &str| {
        PathBuf::from(get(k).unwrap_or_else(|| {
            eprintln!("missing {k}");
            std::process::exit(2)
        }))
    };
    Args {
        p1: path("--p1"),
        p2: path("--p2"),
        data: path("--data"),
        replays: path("--replays"),
        random: get("--inputs").as_deref() == Some("random"),
        rtt: num("--rtt", 150.0),
        jitter: num("--jitter", 8.0),
        loss: num("--loss", 1.0),
        secs: num("--secs", 40.0),
        warmup: num("--warmup", 5.0),
    }
}

/// xorshift, so runs are repeatable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A busy player: walks / crouches / jumps in holds of 6-24 frames and taps
/// a random button (3-6 frames) every 8-30 frames.
fn random_inputs(seed: u64) -> Vec<u16> {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
    let mut out = Vec::with_capacity(60 * 600);
    let dirs = [0u16, 0b1000, 0b0100, 0b0010, 0b1010, 0b0001, 0b1001];
    let (mut dir, mut dir_left, mut btn, mut btn_left, mut gap) = (0u16, 0, 0u16, 0, 10);
    while out.len() < 60 * 600 {
        if dir_left == 0 {
            dir = dirs[(r.next() * dirs.len() as f64) as usize % dirs.len()];
            dir_left = 6 + (r.next() * 18.0) as u32;
        }
        if btn_left == 0 {
            btn = 0;
            if gap == 0 {
                btn = 1 << (4 + (r.next() * 6.0) as u16 % 6);
                btn_left = 3 + (r.next() * 3.0) as u32;
                gap = 8 + (r.next() * 22.0) as u32;
            } else {
                gap -= 1;
            }
        } else {
            btn_left -= 1;
        }
        dir_left -= 1;
        out.push(dir | btn);
    }
    out
}

/// Recorded human input streams (one per side of every replay with input).
fn streams(dir: &Path) -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.sort();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(frames) = v.get("inputs").and_then(|i| i.as_array()) else {
            continue;
        };
        for side in 0..2 {
            let s: Vec<u16> = frames
                .iter()
                .map(|f| f.get(side).and_then(|b| b.as_u64()).unwrap_or(0) as u16)
                .collect();
            if s.iter().filter(|b| **b != 0).count() > 50 {
                out.push(s);
            }
        }
    }
    assert!(!out.is_empty(), "no recorded inputs in {}", dir.display());
    out
}

/// Delaying, lossy UDP proxy between two peers: `a` faces peer A, `b`
/// faces peer B. Delivery is FIFO per direction (jitter delays, never
/// reorders).
fn proxy(rtt: f64, jitter: f64, loss: f64, stop: Arc<AtomicBool>) -> (SocketAddr, SocketAddr) {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    a.set_nonblocking(true).unwrap();
    b.set_nonblocking(true).unwrap();
    let addrs = (a.local_addr().unwrap(), b.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let mut peer: [Option<SocketAddr>; 2] = [None, None];
        // per direction: (deliver at, bytes); 0 = A->B, 1 = B->A
        let mut q: [VecDeque<(Instant, Vec<u8>)>; 2] = [VecDeque::new(), VecDeque::new()];
        let mut last = [Instant::now(); 2];
        let mut buf = [0u8; 4096];
        let socks = [&a, &b];
        while !stop.load(Ordering::Relaxed) {
            for side in 0..2 {
                while let Ok((n, from)) = socks[side].recv_from(&mut buf) {
                    peer[side] = Some(from);
                    if rng.next() * 100.0 < loss {
                        continue;
                    }
                    // ~normal jitter (sum of three uniforms), clipped at 0
                    let z = (rng.next() + rng.next() + rng.next() - 1.5) * 2.0;
                    let ms = (rtt / 2.0 + jitter * z).max(0.0);
                    let at =
                        (Instant::now() + Duration::from_secs_f64(ms / 1000.0)).max(last[side]);
                    last[side] = at;
                    q[side].push_back((at, buf[..n].to_vec()));
                }
            }
            let now = Instant::now();
            for side in 0..2 {
                while q[side].front().is_some_and(|(t, _)| *t <= now) {
                    let (_, d) = q[side].pop_front().unwrap();
                    if let Some(to) = peer[1 - side] {
                        let _ = socks[1 - side].send_to(&d, to);
                    }
                }
            }
            std::thread::sleep(Duration::from_micros(100));
        }
    });
    addrs
}

#[derive(Debug, Default, Clone)]
struct Report {
    delay: usize,
    rtt_ms: u32,
    secs: f64,
    rollbacks: u64,
    resimulated: u64,
    max_rollback: u32,
    stalls: u64,
    skipped: u64,
    refreshes: u64,
    repeats: u64,
    longest_freeze: u64,
    min_fps_1s: f64,
    mispredict: f32,
    cpu_us_mean: f64,
    cpu_us_max: f64,
    tick_hz_min: f64,
}

fn sleep_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_micros(1500) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn peer(
    handle: usize,
    via: SocketAddr,
    mut world: World,
    input: Vec<u16>,
    secs: f64,
    warmup: f64,
) -> Report {
    let mut socket = Socket::bind(0).unwrap();
    let shake = socket
        .handshake(via, Duration::from_secs(10), &mut |_| {})
        .expect("handshake through the proxy");
    // DELAY-CHOICE (the baseline harness patches this line)
    let p95 = shake.rtt.map(|r| r.p95).or(shake.rtt_ms).unwrap_or(0);
    let delay = rugen_net::DelayPolicy::default().delay(p95);
    let opts = Options {
        input_delay: delay,
        ..Default::default()
    };
    let mut net = Netplay::p2p_with_socket(socket, via, handle, &opts).unwrap();
    let mut r = Report {
        delay,
        rtt_ms: p95,
        min_fps_1s: 60.0,
        tick_hz_min: 60.0,
        ..Default::default()
    };
    let refresh = Duration::from_nanos(1_000_000_000 / 60);
    let start = Instant::now();
    let mut next_tick = start;
    let mut next_refresh = start;
    let mut i = 0usize;
    let mut shown = 0u32;
    let mut freeze = 0u64;
    let mut window: VecDeque<bool> = VecDeque::new();
    let mut base: Option<rugen_net::Stats> = None;
    let mut cpu: Vec<f64> = Vec::new();
    let mut measuring_since = None;
    while start.elapsed().as_secs_f64() < secs + warmup {
        let t = next_tick.min(next_refresh);
        sleep_until(t);
        let now = Instant::now();
        if now >= next_tick {
            let b = Buttons::from_bits(input[i % input.len()]);
            i += 1;
            let mut inputs = [Buttons::default(); 2];
            inputs[handle] = b;
            let c = Instant::now();
            net.poll();
            net.advance(&mut world, inputs).expect("advance");
            let us = c.elapsed().as_secs_f64() * 1e6;
            let tick = net.tick_duration();
            next_tick += tick;
            if next_tick + Duration::from_millis(100) < now {
                next_tick = now;
            }
            if measuring_since.is_some() {
                cpu.push(us);
                r.tick_hz_min = r.tick_hz_min.min(1.0 / tick.as_secs_f64());
            }
        }
        if now >= next_refresh {
            next_refresh += refresh;
            let st = net.stats();
            let in_measure = start.elapsed().as_secs_f64() >= warmup && st.synchronized;
            if in_measure && measuring_since.is_none() {
                measuring_since = Some(Instant::now());
                base = Some(st);
                shown = world.frame;
            }
            if measuring_since.is_some() {
                r.refreshes += 1;
                let repeat = world.frame == shown;
                shown = world.frame;
                if repeat {
                    r.repeats += 1;
                    freeze += 1;
                    r.longest_freeze = r.longest_freeze.max(freeze);
                } else {
                    freeze = 0;
                }
                window.push_back(repeat);
                if window.len() > 60 {
                    window.pop_front();
                }
                if window.len() == 60 {
                    let fps = window.iter().filter(|x| !**x).count() as f64;
                    r.min_fps_1s = r.min_fps_1s.min(fps);
                }
            }
        }
    }
    let st = net.stats();
    let b = base.unwrap_or_default();
    r.secs = measuring_since.map_or(0.0, |t| t.elapsed().as_secs_f64());
    r.rollbacks = st.rollbacks - b.rollbacks;
    r.resimulated = st.resimulated - b.resimulated;
    r.max_rollback = st.max_rollback;
    r.stalls = st.stalls - b.stalls;
    r.skipped = st.skipped - b.skipped;
    r.mispredict = st.misprediction_rate();
    cpu.sort_by(f64::total_cmp);
    r.cpu_us_mean = cpu.iter().sum::<f64>() / cpu.len().max(1) as f64;
    r.cpu_us_max = cpu.last().copied().unwrap_or(0.0);
    // keep answering for a moment so the other peer finishes cleanly
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        net.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    r
}

fn main() {
    let a = args();
    let d1 = CharData::load_with_data(&a.p1, &a.data).expect("p1 loads");
    let d2 = if a.p2 == a.p1 {
        d1.clone()
    } else {
        CharData::load_with_data(&a.p2, &a.data).expect("p2 loads")
    };
    let mut world = World::new(Stage::default(), 11);
    world.add_player(d1, 0, 1);
    world.add_player(d2, 1, 2);
    world.start_round();
    let s = streams(&a.replays);
    // long streams so the inputs never repeat within a run
    let concat = |from: usize| -> Vec<u16> {
        s.iter()
            .cycle()
            .skip(from)
            .take(s.len())
            .flatten()
            .copied()
            .collect()
    };
    let (in1, in2) = if a.random {
        (random_inputs(1), random_inputs(2))
    } else {
        (concat(0), concat(s.len() / 2))
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (via_a, via_b) = proxy(a.rtt, a.jitter, a.loss, stop.clone());
    let (w1, w2) = (world.clone(), world);
    let (secs, warmup) = (a.secs, a.warmup);
    let t1 = std::thread::spawn(move || peer(0, via_a, w1, in1, secs, warmup));
    let t2 = std::thread::spawn(move || peer(1, via_b, w2, in2, secs, warmup));
    let (r1, r2) = (t1.join().unwrap(), t2.join().unwrap());
    stop.store(true, Ordering::Relaxed);
    println!("rtt {} ms jitter {} ms loss {}%:", a.rtt, a.jitter, a.loss);
    for (n, r) in [(1, &r1), (2, &r2)] {
        let rb_s = r.rollbacks as f64 / r.secs.max(1e-9);
        let depth = r.resimulated as f64 / r.rollbacks.max(1) as f64;
        let fps = 60.0 * (r.refreshes - r.repeats) as f64 / r.refreshes.max(1) as f64;
        println!(
            "  P{n}: delay {} (p95 {} ms)  rollbacks/s {rb_s:.1}  depth mean {depth:.1} max {}  \
             stalls {} skipped {}  fps {fps:.1} (min 1 s {:.0}, longest freeze {} refreshes)  \
             mispredict {:.1}%  netcode cpu {:.0} us/tick (max {:.0})  slowest tick {:.1} Hz",
            r.delay,
            r.rtt_ms,
            r.max_rollback,
            r.stalls,
            r.skipped,
            r.min_fps_1s,
            r.longest_freeze,
            r.mispredict * 100.0,
            r.cpu_us_mean,
            r.cpu_us_max,
            r.tick_hz_min
        );
    }
}
