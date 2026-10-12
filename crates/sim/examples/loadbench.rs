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
//! `--build` (build the optimised artefacts first), `--original` (ignore them).

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
            let total: usize = snd.sounds.values().map(|s| s.len()).sum();
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

    if args.iter().any(|a| a == "--build") {
        let t = Instant::now();
        let r = rugen_sim::chardata::optimise(&def_path, &data_dir, None).unwrap();
        println!("optimise: {:.1} ms, {r:?}", ms(t));
    }
    let original = args.iter().any(|a| a == "--original");
    let pack = rugen_sim::chardata::opt_dir(&def_path).join(rugen_sim::chardata::SPRITES_FILE);
    if !original && pack.exists() {
        let t = Instant::now();
        let n = rugen_formats::pack::open(&pack)
            .map(|(s, _)| s.sprites.len())
            .unwrap_or(0);
        println!("  sprite pack alone: {n} sprites, {:.1} ms", ms(t));
        let logic = pack.with_file_name(rugen_sim::chardata::LOGIC_FILE);
        let t = Instant::now();
        let b = std::fs::read(&logic).unwrap_or_default();
        println!(
            "  logic.bin read alone: {:.1} MB, {:.1} ms",
            b.len() as f64 / 1e6,
            ms(t)
        );
    }
    let t = Instant::now();
    let data = if original {
        rugen_sim::CharData::load_original(&def_path, &data_dir).unwrap()
    } else {
        rugen_sim::CharData::load_with_data(&def_path, &data_dir).unwrap()
    };
    let full = ms(t);
    let fresh = rugen_sim::chardata::opt_fresh(&def_path, &data_dir);
    println!("optimised artefacts fresh (sprites, logic): {fresh:?}");
    println!(
        "full load_with_data: {full:.1} ms ({} states, {} sprites)",
        data.states.len(),
        data.sff.sprites.len()
    );
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
            .unwrap_or((usize::MAX, k.0 as usize * 65536 + k.1 as usize));
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
