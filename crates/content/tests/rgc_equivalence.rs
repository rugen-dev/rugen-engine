//! A character converted to `.rgc` must give the same game as its original
//! folder: same states, commands, animations and constants, the same
//! pixels where they are drawn, and a bit-identical simulation (final
//! checksum of an AI-vs-AI match fed pseudo-random inputs, both with the
//! precompiled logic and compiling the text). The export of the `.rgc`
//! converts back to the same fingerprint, with every sprite identical.
//!
//! Uses the bundled KFM (`reference/`, skipped when absent).
//! `RUGEN_EQ_CHARS=/path/a.def,/path/b.def` checks more characters (their
//! folders are converted, never modified).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rugen_content::convert::{self, Source};
use rugen_formats::rgc;
use rugen_formats::sff::{Pixels, Sff, Sprite};
use rugen_sim::chardata::CharData;
use rugen_sim::headless::{MatchSpec, build_world, replay};
use rugen_sim::world::Movement;

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rugen-rgc-eq-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// RGBA of the texel at `(x, y)` relative to the sprite axis (transparent outside).
fn texel(sff: &Sff, s: &Sprite, px: &Pixels, x: i32, y: i32) -> [u8; 4] {
    let (tx, ty) = (x + s.offset[0] as i32, y + s.offset[1] as i32);
    if tx < 0 || ty < 0 || tx >= s.width as i32 || ty >= s.height as i32 {
        return [0; 4];
    }
    let i = ty as usize * s.width as usize + tx as usize;
    let c = match px {
        Pixels::Indexed(p) => s
            .palette
            .and_then(|pal| sff.palettes.get(pal))
            .and_then(|pal| pal.get(p[i] as usize).copied())
            .unwrap_or([0; 4]),
        Pixels::Rgba(p) => [p[i * 4], p[i * 4 + 1], p[i * 4 + 2], p[i * 4 + 3]],
        Pixels::Packed(_) => unreachable!("decoded"),
    };
    if c[3] == 0 { [0; 4] } else { c }
}

/// Every sprite draws the same texels (the `.rgc` view may be trimmed).
fn assert_same_sprites(a: &Sff, b: &Sff) {
    assert_eq!(
        a.sprites.keys().collect::<Vec<_>>(),
        b.sprites.keys().collect::<Vec<_>>()
    );
    assert_eq!(a.palettes, b.palettes);
    for (k, sa) in &a.sprites {
        let sb = &b.sprites[k];
        let (pa, pb) = (sa.decoded(), sb.decoded());
        assert!(matches!(pb.as_ref(), Pixels::Indexed(_) | Pixels::Rgba(_)));
        if matches!(pa.as_ref(), Pixels::Indexed(p) if p.is_empty()) {
            continue;
        }
        for y in -(sa.offset[1] as i32)..sa.height as i32 - sa.offset[1] as i32 {
            for x in -(sa.offset[0] as i32)..sa.width as i32 - sa.offset[0] as i32 {
                assert_eq!(
                    texel(a, sa, &pa, x, y),
                    texel(b, sb, &pb, x, y),
                    "sprite {k:?} at {x},{y}"
                );
            }
        }
    }
}

fn assert_same_logic(a: &CharData, b: &CharData) {
    assert_eq!(a.name, b.name);
    assert_eq!(a.localcoord, b.localcoord);
    assert_eq!(format!("{:?}", a.consts), format!("{:?}", b.consts));
    assert_eq!(format!("{:?}", a.states), format!("{:?}", b.states));
    assert_eq!(format!("{:?}", a.cmd), format!("{:?}", b.cmd));
    assert_eq!(format!("{:?}", a.air), format!("{:?}", b.air));
    assert_eq!(a.warnings, b.warnings);
    assert_eq!(
        a.snd.as_ref().map(|s| s.sounds.keys().collect::<Vec<_>>()),
        b.snd.as_ref().map(|s| s.sounds.keys().collect::<Vec<_>>())
    );
}

/// Final checksum of an AI-vs-AI match fed pseudo-random inputs.
fn match_checksum(p1: &Path, p2: &Path, data: &Path, seed: u32) -> (u32, u64) {
    let spec = MatchSpec {
        p1,
        p2,
        stage: None,
        data_dir: data,
        motif: data,
        zoom: (1.0, 1.0),
        movement: Movement::default(),
        round_time: None,
        ai: [8, 8],
    };
    let mut world = build_world(&spec).expect("world builds");
    let mut s = seed;
    let mut inputs = Vec::new();
    for _ in 0..3600 {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        inputs.extend_from_slice(&((s & 0x7ff) as u16).to_le_bytes());
        inputs.extend_from_slice(&(((s >> 11) & 0x7ff) as u16).to_le_bytes());
    }
    let r = replay(&mut world, &inputs, None);
    (r.frames, r.checksum)
}

#[test]
fn rgc_characters_play_like_their_folders() {
    let Some(root) = reference() else {
        eprintln!("reference tree missing; skipping");
        return;
    };
    let data = root.join("data");
    let mut defs: Vec<PathBuf> = ["kfm", "kfm720"]
        .iter()
        .map(|n| root.join("chars").join(n).join(format!("{n}.def")))
        .filter(|p| p.exists())
        .collect();
    if let Ok(extra) = std::env::var("RUGEN_EQ_CHARS") {
        defs.extend(
            extra
                .split(',')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        );
    }
    for src_def in defs {
        check(&src_def, &data);
    }
}

fn check(src_def: &Path, data: &Path) {
    let name = src_def.file_stem().unwrap().to_string_lossy().to_string();
    let work = scratch(&name);
    let folder = src_def.parent().unwrap();
    let rgc_path = work.join(format!("{name}.rgc"));
    let src = Source {
        root: folder.to_path_buf(),
        def: src_def.file_name().unwrap().to_string_lossy().to_string(),
        kind: rgc::Kind::Char,
        name: name.clone(),
    };
    let t = std::time::Instant::now();
    let report = convert::convert(&src, &rgc_path, data, None).unwrap();
    eprintln!("{name}: {} ({:?})", report.summary(), t.elapsed());
    let fp = report.fingerprint.clone().expect("fingerprinted");
    let def = rgc_path.join(&src.def);

    // same game data
    let original = CharData::load_original(src_def, data).unwrap();
    let packed = CharData::load_with_data(&def, data).unwrap();
    let compiled = CharData::load_original(&def, data).unwrap();
    assert!(
        packed
            .sff
            .sprites
            .values()
            .all(|s| matches!(s.pixels, Pixels::Packed(_) | Pixels::Indexed(_))),
        "{name}: sprites come from the .rgc"
    );
    assert_same_logic(&original, &packed);
    assert_same_logic(&original, &compiled);
    assert_same_sprites(&original.sff, &packed.sff);

    // same simulation
    let before = match_checksum(src_def, src_def, data, 0xC0FFEE);
    let after = match_checksum(&def, &def, data, 0xC0FFEE);
    eprintln!(
        "{name}: {} frames, checksum {:016x} (folder) / {:016x} (.rgc)",
        before.0, before.1, after.1
    );
    assert_eq!(before, after, "{name}: simulation diverged");
    // a mixed match (one side .rgc, the other the folder) agrees too
    let mixed = |a: Arc<CharData>, b: Arc<CharData>| {
        let mut w = rugen_sim::World::new(rugen_sim::Stage::default(), 3);
        w.add_player(a, 0, 1);
        w.add_player(b, 1, 2);
        w.start_round();
        w
    };
    let inputs: Vec<u8> = (0..2400u32)
        .flat_map(|i| {
            let v = (i.wrapping_mul(2_654_435_761) >> 7) as u16 & 0x7ff;
            [v as u8, (v >> 8) as u8, (v >> 3) as u8, (v >> 11) as u8]
        })
        .collect();
    let mut a = mixed(Arc::clone(&packed), Arc::clone(&original));
    let mut b = mixed(Arc::clone(&original), Arc::clone(&original));
    assert_eq!(replay(&mut a, &inputs, None), replay(&mut b, &inputs, None));

    // the fingerprint of the .rgc, recomputed from its content, is the folder's
    let v = rugen_fair::package::verify(&rgc_path, u64::MAX).unwrap();
    assert!(v.honest());
    assert_eq!(v.fingerprint, fp);
    let files = rugen_fair::files::char_files(src_def, Some(data)).unwrap();
    assert_eq!(rugen_fair::fingerprint(&files).unwrap(), fp);

    // export → convert again: same fingerprint, identical sprites
    let out = work.join("exported");
    let ex = rugen_content::export::export(&rgc_path, &out, false).unwrap();
    let back = work.join("back.rgc");
    let src2 = Source {
        root: out.clone(),
        def: ex.def.clone(),
        kind: rgc::Kind::Char,
        name: "exported".into(),
    };
    let r2 = convert::convert(&src2, &back, data, None).unwrap();
    assert_eq!(
        r2.fingerprint.as_deref(),
        Some(fp.as_str()),
        "{name}: export round trip"
    );
    let files = rugen_fair::files::char_files(&out.join(&ex.def), Some(data)).unwrap();
    for e in files
        .entries
        .iter()
        .filter(|e| e.role == rugen_fair::Role::Sprite)
    {
        let orig = Sff::open(folder.join(&e.path), true).unwrap();
        let exported = Sff::open(&e.source, true).unwrap();
        assert_eq!(orig.sprites, exported.sprites, "{name}: exported sprites");
        assert_eq!(orig.palettes, exported.palettes);
    }
    let _ = std::fs::remove_dir_all(&work);
}
