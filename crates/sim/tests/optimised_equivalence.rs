//! Loading a character from its optimised artefacts (sprite pack + compiled
//! logic) must give the same game as loading the original files: same
//! states, commands, animations and constants, same pixels where they are
//! drawn, and bit-identical simulation (final checksum of an AI-vs-AI match
//! with random inputs). Uses the bundled KFM; skipped when the reference
//! tree is absent. `RUGEN_EQ_CHARS=/path/a.def,/path/b.def` checks more
//! characters (their folder is copied first).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rugen_formats::sff::{Pixels, Sff, Sprite};
use rugen_sim::chardata::{self, CharData};
use rugen_sim::headless::{MatchSpec, build_world, replay};
use rugen_sim::world::Movement;

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

/// A private copy of a character folder (artefacts are written next to the def).
fn copy_char(src: &Path, tag: &str) -> PathBuf {
    let dst = std::env::temp_dir().join(format!("rugen-opt-eq-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst).unwrap();
    for e in std::fs::read_dir(src).unwrap().flatten() {
        if e.path().is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
        }
    }
    dst
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

fn assert_same_sprites(a: &Sff, b: &Sff) {
    assert_eq!(
        a.sprites.keys().collect::<Vec<_>>(),
        b.sprites.keys().collect::<Vec<_>>()
    );
    for (k, sa) in &a.sprites {
        let sb = &b.sprites[k];
        let (pa, pb) = (sa.decoded(), sb.decoded());
        assert!(matches!(pb.as_ref(), Pixels::Indexed(_) | Pixels::Rgba(_)));
        // every texel of the original rectangle, relative to the axis
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
        a.snd.as_ref().map(|s| s.sounds.len()),
        b.snd.as_ref().map(|s| s.sounds.len())
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
fn optimised_kfm_plays_like_the_original() {
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
    let dir = copy_char(src_def.parent().unwrap(), &name);
    let def = dir.join(src_def.file_name().unwrap());
    let data = data.to_path_buf();
    // original timings, before any artefact exists
    let before = match_checksum(&def, &def, &data, 0xC0FFEE);
    let original = CharData::load_original(&def, &data).unwrap();

    let report = chardata::optimise(&def, &data, None).unwrap();
    assert!(report.sprites.is_some() && report.logic, "{report:?}");
    assert_eq!(chardata::opt_fresh(&def, &data), (true, true));
    let again = chardata::optimise(&def, &data, None).unwrap();
    assert!(
        again.sprites.is_none() && !again.logic,
        "second run is a no-op"
    );

    let optimised = CharData::load_with_data(&def, &data).unwrap();
    assert!(
        optimised
            .sff
            .sprites
            .values()
            .all(|s| matches!(s.pixels, Pixels::Packed(_))),
        "{name}: sprites come from the pack"
    );
    assert_same_logic(&original, &optimised);
    assert_same_sprites(&original.sff, &optimised.sff);

    let after = match_checksum(&def, &def, &data, 0xC0FFEE);
    eprintln!(
        "{name}: {} frames, checksum {:016x} (original) / {:016x} (optimised)",
        before.0, before.1, after.1
    );
    assert_eq!(before, after, "{name}: simulation diverged");

    // a mixed match (one side optimised, the other original) agrees too
    let world_mixed = {
        let mut w = rugen_sim::World::new(rugen_sim::Stage::default(), 3);
        w.add_player(Arc::clone(&optimised), 0, 1);
        w.add_player(CharData::load_original(&def, &data).unwrap(), 1, 2);
        w.start_round();
        w
    };
    let world_orig = {
        let mut w = rugen_sim::World::new(rugen_sim::Stage::default(), 3);
        w.add_player(Arc::clone(&original), 0, 1);
        w.add_player(CharData::load_original(&def, &data).unwrap(), 1, 2);
        w.start_round();
        w
    };
    let inputs: Vec<u8> = (0..2400u32)
        .flat_map(|i| {
            let v = (i.wrapping_mul(2_654_435_761) >> 7) as u16 & 0x7ff;
            [v as u8, (v >> 8) as u8, (v >> 3) as u8, (v >> 11) as u8]
        })
        .collect();
    let (mut a, mut b) = (world_mixed, world_orig);
    assert_eq!(replay(&mut a, &inputs, None), replay(&mut b, &inputs, None));

    // an edited source makes the artefacts stale: the loader falls back
    let air = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("air")))
        .unwrap();
    let text = std::fs::read(&air).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&air, [&text[..], b"\n"].concat()).unwrap();
    assert_eq!(chardata::opt_fresh(&def, &data), (true, false));
    let fallback = CharData::load_with_data(&def, &data).unwrap();
    assert_same_logic(&original, &fallback);

    let _ = std::fs::remove_dir_all(&dir);
}
