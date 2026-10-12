//! Misprediction rate of input prediction strategies on recorded matches.
//!
//! `cargo run --release -p rugen-net --example predict -- <replays dir>`
//!
//! Reads every rugen replay (`inputs: [[p1, p2], ...]`), keeps the sides
//! with real input, and prints, per prediction distance (frames between
//! the last received input and the predicted frame), how often each
//! strategy guessed wrong. See `rugen_net::predict`.

use rugen_net::predict::{Strategy, misprediction_rate};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "replays".into());
    let mut streams = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("replays dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    for f in files {
        let Ok(v) = std::fs::read_to_string(&f)
            .map_err(|_| ())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|_| ()))
        else {
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
                streams.push(s);
            }
        }
    }
    let frames: usize = streams.iter().map(Vec::len).sum();
    println!("{} input streams, {frames} frames", streams.len());
    let strategies = [
        ("repeat last (ggrs)", Strategy::RepeatLast),
        ("directions only", Strategy::DirectionsOnly),
        ("taps of 3 frames", Strategy::TapLength(3)),
        ("taps of 5 frames", Strategy::TapLength(5)),
        ("taps of 8 frames", Strategy::TapLength(8)),
    ];
    print!("{:<22}", "lag (frames)");
    let lags = [1, 2, 4, 6, 8, 10];
    for l in lags {
        print!("{l:>8}");
    }
    println!();
    for (name, s) in strategies {
        print!("{name:<22}");
        for l in lags {
            print!("{:>7.1}%", misprediction_rate(&streams, l, s) * 100.0);
        }
        println!();
    }
}
