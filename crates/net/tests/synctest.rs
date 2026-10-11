//! ggrs sync test over the bundled KFM: every tick rolls back and re-simulates
//! several frames and compares checksums. Skipped when the reference tree is absent.

use std::path::PathBuf;

use rugen_net::{Netplay, Options};
use rugen_sim::{Buttons, CharData, Stage, World};

fn reference() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
    root.join("chars/kfm/kfm.def").exists().then_some(root)
}

#[test]
fn synctest_kfm_mirror_match() {
    let Some(root) = reference() else {
        eprintln!("reference tree missing; skipping");
        return;
    };
    let data = CharData::load_with_data(&root.join("chars/kfm/kfm.def"), &root.join("data"))
        .expect("kfm loads");
    let mut world = World::new(Stage::default(), 7);
    world.add_player(data.clone(), 0, 1);
    world.add_player(data, 1, 2);
    world.start_round();

    let mut net = Netplay::synctest(7, &Options::default()).expect("synctest session");
    let mut seed: u32 = 0x1234_5678;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut frames = 0;
    for _ in 0..600 {
        let a = Buttons::from_bits((rnd() & 0x7ff) as u16);
        let b = Buttons::from_bits((rnd() & 0x7ff) as u16);
        let adv = net
            .advance(&mut world, [a, b])
            .expect("no checksum mismatch");
        frames += adv.new_frames;
    }
    assert!(frames >= 590, "advanced {frames} frames");
    assert!(world.frame >= 590);
}
