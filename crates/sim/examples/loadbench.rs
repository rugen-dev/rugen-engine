//! Time and memory of loading one character, by phase.
//!
//! `cargo run --release -p rugen-sim --example loadbench -- <char.def> [data_dir]`
//!
//! Prints the phases of `CharData::load_with_data` re-done step by step
//! (SFF read + decode, SND, AIR, CMD, CNS parse + compile), then a full
//! `load_with_data`, then the process peak RSS (`VmHWM`). Run one character
//! per process so the peak belongs to it.
//!
//! Flags: `--phases` (step-by-step breakdown of the original loader),
//! `--parts` (resident memory per part: sprites, sounds, animations, logic,
//! and the WAV bytes every sound decodes to), `--original` (compile the
//! logic instead of using the precompiled one).
//! Works on a folder's def or on one inside an `.rgc` (`chars/x.rgc/x.def`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use rugen_formats::def::Def;
use rugen_formats::text::decode;
use rugen_sim::chardata::resolve;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn vm(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let def_path = PathBuf::from(args.get(1).expect("usage: loadbench <char.def> [data_dir]"));
    let data_dir = args.get(2).map(PathBuf::from).unwrap_or_default();
    let phases = args.iter().any(|a| a == "--phases");
    let dir = def_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    if phases {
        let def = Def::open(&def_path).unwrap();
        let files = def.section("files").unwrap();
        let find = |k: &str| files.raw(k).and_then(|n| resolve(&dir, n).ok());

        let t = Instant::now();
        let sff_bytes = std::fs::read(find("sprite").unwrap()).unwrap();
        let read_ms = ms(t);
        let t = Instant::now();
        let sff = rugen_formats::sff::Sff::parse(&sff_bytes, true).unwrap();
        let decode_ms = ms(t);
        let (mut px, mut linked_px) = (0usize, 0usize);
        let mut seen = std::collections::HashSet::new();
        for s in sff.sprites.values() {
            let (n, ptr) = match &s.pixels {
                rugen_formats::sff::Pixels::Indexed(p) => (p.len(), p.as_ptr() as usize),
                rugen_formats::sff::Pixels::Rgba(p) => (p.len(), p.as_ptr() as usize),
                rugen_formats::sff::Pixels::Packed(p) => (p.raw_len(), p.share_key().1),
            };
            px += n;
            if !seen.insert(ptr) {
                linked_px += n;
            }
        }
        println!(
            "sff: {} sprites, {} palettes, {:.1} MB file, read {read_ms:.1} ms, decode {decode_ms:.1} ms, {:.1} MB pixels{}",
            sff.sprites.len(),
            sff.palettes.len(),
            sff_bytes.len() as f64 / 1e6,
            px as f64 / 1e6,
            if linked_px > 0 {
                format!(" ({:.1} MB aliased)", linked_px as f64 / 1e6)
            } else {
                String::new()
            }
        );
        drop((sff, sff_bytes));

        if let Some(p) = find("sound") {
            let t = Instant::now();
            let snd = rugen_formats::snd::Snd::open(&p).unwrap();
            let total: usize = snd.sounds.values().map(|s| s.bytes().len()).sum();
            println!(
                "snd: {} sounds, {:.1} MB, {:.1} ms",
                snd.sounds.len(),
                total as f64 / 1e6,
                ms(t)
            );
        }
        let t = Instant::now();
        let air = rugen_formats::air::Air::open(find("anim").unwrap()).unwrap();
        println!("air: {} actions, {:.1} ms", air.actions.len(), ms(t));

        let cmd_name = files.raw("cmd").unwrap();
        let t = Instant::now();
        let cmd_text = decode(&std::fs::read(find("cmd").unwrap()).unwrap());
        let cmd = rugen_cns::cmd::parse(cmd_name, &cmd_text);
        println!("cmd: {} commands, {:.1} ms", cmd.commands.len(), ms(t));

        let mut names = vec![cmd_name.to_string()];
        for k in ["cns", "st"]
            .into_iter()
            .map(String::from)
            .chain((0..=9).map(|i| format!("st{i}")))
        {
            if let Some(n) = files.raw(&k).filter(|s| !s.is_empty()) {
                names.push(n.to_string());
            }
        }
        let (mut parse_ms, mut n_states) = (0.0, 0);
        let mut parsed = Vec::new();
        for n in &names {
            let Ok(p) = resolve(&dir, n) else { continue };
            let t = Instant::now();
            let text = decode(&std::fs::read(p).unwrap());
            let st = rugen_cns::state::parse_states(n, &text);
            parse_ms += ms(t);
            n_states += st.states.len();
            parsed.push(st);
        }
        println!("cns/st parse: {n_states} states, {parse_ms:.1} ms");
        let t = Instant::now();
        let mut n_ctrl = 0;
        for p in &parsed {
            for s in &p.states {
                for c in &s.controllers {
                    n_ctrl += 1;
                    let _ = rugen_sim::sctrl::Sctrl::compile(c);
                }
            }
        }
        println!("sctrl compile: {n_ctrl} controllers, {:.1} ms", ms(t));
        println!("rss after phases: {} MB", vm("VmRSS:") / 1024);
    }

    let parts = args.iter().any(|a| a == "--parts");
    // what each part costs resident, opened alone through the real loaders
    // (the logic is what the full load adds on top of these)
    let mut assets_kb = 0;
    if parts {
        let def = Def::open(&def_path).unwrap();
        let files = def.section("files").unwrap();
        let find = |k: &str| files.raw(k).and_then(|n| resolve(&dir, n).ok());
        let rss0 = vm("VmRSS:");
        let sff = rugen_formats::sff::Sff::open(find("sprite").unwrap(), true).unwrap();
        let rss1 = vm("VmRSS:");
        let packed: usize =
            sff.sprites
                .values()
                .map(|s| match &s.pixels {
                    rugen_formats::sff::Pixels::Packed(p) => p.packed_len(),
                    rugen_formats::sff::Pixels::Indexed(p)
                    | rugen_formats::sff::Pixels::Rgba(p) => p.len(),
                })
                .sum();
        println!(
            "part sff: {} sprites, {:.1} MB pixel bytes held, rss +{} MB",
            sff.sprites.len(),
            packed as f64 / 1e6,
            (rss1 - rss0) / 1024
        );
        let snd = find("sound").map(|p| rugen_formats::snd::Snd::open(p).unwrap());
        let rss2 = vm("VmRSS:");
        if let Some(snd) = &snd {
            println!(
                "part snd: {} sounds, rss +{} MB",
                snd.sounds.len(),
                (rss2 - rss1) / 1024
            );
        }
        let air = rugen_formats::air::Air::open(find("anim").unwrap()).unwrap();
        let rss3 = vm("VmRSS:");
        println!(
            "part air: {} actions, rss +{} MB",
            air.actions.len(),
            (rss3 - rss2) / 1024
        );
        assets_kb = rss3 - rss0;
        // every sound decoded to WAV, as a fight's prefetch did
        if let Some(snd) = &snd {
            let t = Instant::now();
            let mut total = 0usize;
            for s in snd.sounds.values() {
                let w = s.bytes();
                total += w.len();
            }
            let rss4 = vm("VmRSS:");
            println!(
                "part snd decoded: {:.1} MB of WAV in {:.0} ms, rss +{} MB",
                total as f64 / 1e6,
                ms(t),
                (rss4 - rss3) / 1024
            );
        }
        drop((sff, snd, air));
    }

    let original = args.iter().any(|a| a == "--original");
    let rss_before = vm("VmRSS:");
    let t = Instant::now();
    let data = if original {
        rugen_sim::CharData::load_original(&def_path, &data_dir).unwrap()
    } else {
        rugen_sim::CharData::load_with_data(&def_path, &data_dir).unwrap()
    };
    let full = ms(t);
    println!(
        "full load_with_data: {full:.1} ms ({} states, {} sprites)",
        data.states.len(),
        data.sff.sprites.len()
    );
    if parts {
        let rss = vm("VmRSS:") - rss_before;
        let ctrls: usize = data.states.values().map(|s| s.ctrls.len()).sum();
        let raw: usize = data.states.values().map(|s| s.def.controllers.len()).sum();
        let states_bytes = bincode::serialized_size(&data.states).unwrap_or(0);
        println!(
            "part logic: {} states, {ctrls} controllers ({raw} raw kept), states serialize to {:.1} MB; full load rss +{} MB, so logic + overhead about {} MB",
            data.states.len(),
            states_bytes as f64 / 1e6,
            rss / 1024,
            rss.saturating_sub(assets_kb) / 1024
        );
    }
    // texture memory if every sprite gets uploaded (shared pixels upload once)
    let mut seen = std::collections::HashSet::new();
    let mut vram = 0usize;
    for (k, s) in &data.sff.sprites {
        let bpp = match &s.pixels {
            rugen_formats::sff::Pixels::Rgba(_) => 4,
            rugen_formats::sff::Pixels::Packed(p) if p.is_rgba() => 4,
            _ => 1,
        };
        let key = s
            .share_key()
            .unwrap_or((usize::MAX, k.0 as usize * 65536 + k.1 as usize, 0));
        if seen.insert(key) {
            vram += s.width as usize * s.height as usize * bpp;
        }
    }
    println!(
        "textures if all uploaded: {} ({:.1} MB) + {} palettes",
        seen.len(),
        vram as f64 / 1e6,
        data.sff.palettes.len()
    );
    println!(
        "rss: {} MB, peak (VmHWM): {} MB",
        vm("VmRSS:") / 1024,
        vm("VmHWM:") / 1024
    );
}
