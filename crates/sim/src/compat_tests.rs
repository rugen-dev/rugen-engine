//! Compatibility regressions from the owner's play reports: MUGEN content
//! must work unchanged, so each fix is pinned here with a synthetic state on
//! KFM (always run when the reference tree is present) and, where the
//! report came from a community fighter, with that fighter
//! (`RUGEN_GOLDEN_CHARS=<dir with G6_Sasuke_Rinnegan/ Goku/>
//! cargo test -p rugen-sim compat -- --ignored`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::explod::{PosType, Space};
use crate::headless::{MatchSpec, build_world};
use crate::scale::FighterSize;
use crate::world::{Movement, Stage, World};
use crate::{Buttons, CharData};

fn reference() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1")
}

fn kfm() -> Option<Arc<CharData>> {
    let p = reference().join("chars/kfm/kfm.def");
    p.exists()
        .then(|| CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}")))
}

/// KFM vs KFM with `size`, past the intro.
fn kfm_world(size: FighterSize) -> Option<World> {
    let d = kfm()?;
    let mut w = World::new(Stage::default(), 1);
    w.fighter_size = size;
    w.add_player(d.clone(), 0, 1);
    w.add_player(d, 1, 2);
    w.start_round();
    skip_intro(&mut w);
    Some(w)
}

fn skip_intro(w: &mut World) {
    let mut guard = 0;
    while w.round.round_state() != 2 && guard < 2000 {
        w.step(&[Buttons::default(), Buttons::default()]);
        guard += 1;
    }
}

fn run(w: &mut World, n: usize) {
    for _ in 0..n {
        w.step(&[Buttons::default(), Buttons::default()]);
    }
}

fn state_text(no: i32, body: &str) -> String {
    format!("[Statedef {no}]\ntype = S\nmovetype = I\nphysics = S\nanim = 0\nctrl = 0\n{body}\n")
}

/// Give P1 (data slot 0) extra states and put it in `no`.
fn with_state(w: &mut World, no: i32, body: &str) {
    let d = w.data[0]
        .with_states("compat", &state_text(no, body))
        .unwrap();
    w.data[0] = d;
    w.change_state(0, no, None, None);
}

fn explod_by_id(w: &World, id: i32) -> &crate::explod::Explod {
    w.explods
        .iter()
        .find(|e| !e.removed && e.id == id)
        .unwrap_or_else(|| panic!("explod {id} missing"))
}

/// Screen position (x from the centre, y from the top, stage units) of a
/// screen-space explod.
fn screen_pos(e: &crate::explod::Explod) -> [f32; 2] {
    assert!(e.on_screen(), "explod {} drawn in screen space", e.id);
    e.draw_pos()
}

/// Owner report 1 / 2 (Sasuke's Kirin clouds in the middle of the screen,
/// Goku's full-screen super backgrounds wrong): `postype = back / front /
/// left / right` explods live in screen space, measured from the top of the
/// screen, in the owner's localcoord and never zoomed by FIGHTER SIZE.
#[test]
fn screen_explods_are_laid_out_on_the_screen() {
    for size in [FighterSize::None, FighterSize::Normal, FighterSize::Chibi] {
        let Some(mut w) = kfm_world(size) else { return };
        with_state(
            &mut w,
            7100,
            "[State 7100, sky]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 1\n\
             postype = back\npos = 155,-10\nbindtime = -1\nremovetime = -1\n\n\
             [State 7100, left]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 2\n\
             postype = left\npos = 20,90\nremovetime = -1\n\n\
             [State 7100, right]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 3\n\
             postype = right\npos = -20,90\nremovetime = -1\n\n\
             [State 7100, screen p1]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 4\n\
             space = screen\npos = 0,0\nremovetime = -1\n\n\
             [State 7100, stage left]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 5\n\
             space = stage\npostype = left\npos = 0,0\nremovetime = -1\n\n\
             [State 7100, body]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 6\n\
             pos = 0,-50\nremovetime = -1\n",
        );
        run(&mut w, 3);
        // KFM is a 320 fighter on a 320 stage: one screen unit per local one
        let s = w.screen_scale(w.players[0].data);
        assert_eq!(s, 1.0);
        let half = w.stage.width / 2.0;
        // P1 faces right: back = the left edge
        let sky = explod_by_id(&w, 1);
        assert_eq!(sky.postype, PosType::Left);
        assert_eq!(screen_pos(sky), [-half + 155.0, -10.0], "{size:?}");
        assert_eq!(screen_pos(explod_by_id(&w, 2)), [-half + 20.0, 90.0]);
        let right = explod_by_id(&w, 3);
        assert_eq!(right.postype, PosType::Right);
        assert_eq!(screen_pos(right), [half - 20.0, 90.0]);
        // screen-space p1: the player's position read as a screen one, the
        // ground line below the top
        let ls = w.local_scale(0);
        let p = w.players[0].pos;
        let sp = screen_pos(explod_by_id(&w, 4));
        assert!((sp[0] - p[0] * ls).abs() < 1e-3);
        assert!((sp[1] - (w.stage.zoffset + p[1] * ls)).abs() < 1e-3);
        // an explicit stage space keeps left in the stage, at the edge
        let st = explod_by_id(&w, 5);
        assert_eq!(st.resolved_space(), Space::Stage);
        assert!((st.draw_pos()[0] - w.screen_left()).abs() < 1.0);
        // a normal explod still follows the fighter's zoom
        let body = explod_by_id(&w, 6);
        assert!(!body.on_screen());
        assert!((body.draw_pos()[1] - (p[1] - 50.0) * ls).abs() < 1e-3);
    }
}

/// `GameWidth` / `ScreenWidth` are in the fighter's own localcoord (Ikemen),
/// the units its screen-space explods use: 320 for KFM on any stage size.
#[test]
fn game_width_is_in_the_fighters_units() {
    let Some(d) = kfm() else { return };
    let stage = Stage {
        width: 1280.0,
        height: 720.0,
        zoffset: 600.0,
        ..Stage::default()
    };
    let mut w = World::new(stage, 1);
    w.add_player(d.clone(), 0, 1);
    w.add_player(d, 1, 2);
    w.start_round();
    let code = |t: &str| rugen_cns::compile(t).unwrap();
    let sw = w.debug_eval(0, &code("ScreenWidth")).to_f();
    let sh = w.debug_eval(0, &code("ScreenHeight")).to_f();
    assert_eq!((sw, sh), (320.0, 180.0));
    let gw = w.debug_eval(0, &code("GameWidth")).to_f();
    assert!((gw - 320.0 / w.cam_zoom).abs() < 1e-3, "{gw}");
}

// ------------------------------------------------- community fighters

fn golden_dir() -> Option<PathBuf> {
    std::env::var("RUGEN_GOLDEN_CHARS").ok().map(PathBuf::from)
}

/// Sasuke (P1) vs Goku with `size`, past the intro.
fn community(p1: &Path, p2: &Path, size: FighterSize) -> World {
    let root = reference();
    let stage = root.join("stages/kfm.def");
    let spec = MatchSpec {
        p1,
        p2,
        stage: Some(&stage),
        data_dir: &root.join("data"),
        motif: &root.join("data/system.def"),
        zoom: crate::headless::ONLINE_ZOOM,
        movement: Movement::default(),
        round_time: None,
        ai: [0, 0],
        fighter_size: size,
        seed: 7,
    };
    let mut w = build_world(&spec).expect("world");
    skip_intro(&mut w);
    w
}

/// Sasuke's Kirin (state 1300): the "Rain" helper's clouds (explod 1811,
/// `postype = back`, pos y -10) sit at the top of the screen, not on the
/// ground line in the middle of it, with FIGHTER SIZE none or normal.
#[test]
#[ignore]
fn sasuke_kirin_clouds_are_in_the_sky() {
    let Some(dir) = golden_dir() else { return };
    let sasuke = dir.join("G6_Sasuke_Rinnegan/G6_Sasuke_Rinnegan.def");
    let goku = dir.join("Goku/Goku.def");
    for size in [FighterSize::None, FighterSize::Normal] {
        let mut w = community(&sasuke, &goku, size);
        w.players[0].power = w.players[0].power_max;
        w.change_state(0, 1300, None, None);
        let mut seen = false;
        for _ in 0..400 {
            run(&mut w, 1);
            for e in w
                .explods
                .iter()
                .filter(|e| !e.removed && e.anim.action == 1811)
            {
                let [_, y] = screen_pos(e);
                let s = w.screen_scale(w.players[e.owner].data);
                assert!((y - -10.0 * s).abs() < 1e-3, "clouds at y {y} ({size:?})");
                seen = true;
            }
        }
        assert!(seen, "Kirin made its clouds ({size:?})");
    }
}

/// Goku's Genkidama (state 3001): the portrait helper lays its cut-in out in
/// screen space at 320x240 positions (Blue Lights FX at 20,90 from the left
/// edge), unscaled by FIGHTER SIZE, which zooms Goku x1.3 at normal.
#[test]
#[ignore]
fn goku_super_cut_in_fills_the_screen() {
    let Some(dir) = golden_dir() else { return };
    let sasuke = dir.join("G6_Sasuke_Rinnegan/G6_Sasuke_Rinnegan.def");
    let goku = dir.join("Goku/Goku.def");
    for size in [FighterSize::None, FighterSize::Normal] {
        let mut w = community(&goku, &sasuke, size);
        w.players[0].power = w.players[0].power_max;
        w.change_state(0, 3001, None, None);
        run(&mut w, 25);
        let s = w.screen_scale(w.players[0].data);
        let half = w.stage.width / 2.0;
        let lights = w
            .explods
            .iter()
            .find(|e| !e.removed && e.anim.action == 3090)
            .unwrap_or_else(|| panic!("Blue Lights FX ({size:?})"));
        let [x, y] = screen_pos(lights);
        assert!((x - (-half + 20.0 * s)).abs() < 1e-3, "x {x} ({size:?})");
        assert!((y - 90.0 * s).abs() < 1e-3, "y {y} ({size:?})");
        // the cut-in hides the stage and the bars
        use crate::player::assert;
        assert!(w.players.iter().any(|p| p.asserted(assert::NOBG)));
        assert!(w.players.iter().any(|p| p.asserted(assert::NOBARDISPLAY)));
    }
}

/// Owner report 3 (Susanoo's dark copy misaligned): a helper's omitted
/// `size.*` come from the helper that creates it (MUGEN docs, Ikemen
/// `newHelper`), not from the root's constants.
#[test]
fn helpers_inherit_their_parents_size() {
    let Some(mut w) = kfm_world(FighterSize::None) else {
        return;
    };
    let text = format!(
        "{}{}{}",
        state_text(
            7200,
            "[State 7200, a]\ntype = Helper\ntrigger1 = time = 0\nstateno = 7201\nid = 71\n\
             size.xscale = .6\nsize.yscale = .5\n"
        ),
        state_text(
            7201,
            "[State 7201, b]\ntype = Helper\ntrigger1 = time = 0\nstateno = 7202\nid = 72\n"
        ),
        state_text(7202, "[State 7202, c]\ntype = Null\ntrigger1 = 1\n"),
    );
    let d = w.data[0].with_states("compat", &text).unwrap();
    w.data[0] = d;
    w.change_state(0, 7200, None, None);
    run(&mut w, 4);
    let by_id = |hid: i32| {
        w.players
            .iter()
            .find(|p| !p.removed && p.helper_index > 0 && p.helper_id == hid)
            .unwrap_or_else(|| panic!("helper {hid}"))
    };
    let (a, b) = (by_id(71), by_id(72));
    assert_eq!((a.size.xscale, a.size.yscale), (0.6, 0.5));
    assert_eq!((b.size.xscale, b.size.yscale), (0.6, 0.5));
}

/// A helper without `ownpal` shows its root's PalFX and ignores its own
/// PalFX controllers (Ikemen `getPalfx` / `palFX.Run`); with `ownpal = 1`
/// its PalFX is its own.
#[test]
fn helper_palfx_follows_ownpal() {
    let Some(mut w) = kfm_world(FighterSize::None) else {
        return;
    };
    let text = format!(
        "{}{}",
        state_text(
            7300,
            "[State 7300, shared]\ntype = Helper\ntrigger1 = time = 0\nstateno = 7301\nid = 81\n\n\
             [State 7300, own]\ntype = Helper\ntrigger1 = time = 0\nstateno = 7301\nid = 82\n\
             ownpal = 1\n\n\
             [State 7300, root]\ntype = PalFX\ntrigger1 = time = 0\ntime = 100\nadd = 0,0,200\n"
        ),
        state_text(
            7301,
            "[State 7301, white]\ntype = PalFX\ntrigger1 = 1\ntime = 100\nadd = 255,255,255\n"
        ),
    );
    let d = w.data[0].with_states("compat", &text).unwrap();
    w.data[0] = d;
    w.change_state(0, 7300, None, None);
    run(&mut w, 3);
    let idx = |hid: i32| {
        w.players
            .iter()
            .position(|p| !p.removed && p.helper_index > 0 && p.helper_id == hid)
            .unwrap_or_else(|| panic!("helper {hid}"))
    };
    let (shared, own) = (idx(81), idx(82));
    assert!(
        !w.players[shared].palfx.active(),
        "its own PalFX did nothing"
    );
    assert_eq!(
        w.drawn_palfx(shared).add,
        [0, 0, 200],
        "drawn with the root's"
    );
    assert_eq!(w.drawn_palfx(own).add, [255, 255, 255]);
}

/// HitDef `palfx.*` tints the target on a hit (the white flash of a ki
/// blast or a super).
#[test]
fn hitdef_palfx_reaches_the_target() {
    let Some(mut w) = kfm_world(FighterSize::None) else {
        return;
    };
    w.players[0].pos[0] = -20.0;
    w.players[1].pos[0] = 20.0;
    // KFM's light punch (anim 200) reaches with its Clsn1
    let text = "[Statedef 7400]\ntype = S\nmovetype = A\nphysics = S\nanim = 200\nctrl = 0\n\n\
        [State 7400, hit]\ntype = HitDef\ntrigger1 = time = 0\nattr = S, NA\ndamage = 10\n\
        animtype = light\nguardflag = MA\nhitflag = MAF\npausetime = 0,0\n\
        palfx.time = 20\npalfx.add = 255,255,255\npalfx.mul = 128,128,128\n";
    let d = w.data[0].with_states("compat", text).unwrap();
    w.data[0] = d;
    w.change_state(0, 7400, None, None);
    let mut hit = false;
    for _ in 0..12 {
        run(&mut w, 1);
        if w.players[1].palfx.active() {
            hit = true;
            break;
        }
    }
    assert!(hit, "P2 was hit and tinted");
    assert_eq!(w.players[1].palfx.add, [255, 255, 255]);
    assert_eq!(w.players[1].palfx.mul, [128, 128, 128]);
}

/// Explods follow their owner's PalFX unless `ownpal = 1`, which gives them
/// their own (`palfx.*`, Ikemen); fightfx sparks are always `ownpal`.
#[test]
fn explod_ownpal_and_palfx() {
    let Some(mut w) = kfm_world(FighterSize::None) else {
        return;
    };
    with_state(
        &mut w,
        7500,
        "[State 7500, follows]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 1\nremovetime = -1\n\n\
         [State 7500, own]\ntype = Explod\ntrigger1 = time = 0\nanim = 0\nid = 2\nremovetime = -1\n\
         ownpal = 1\npalfx.time = -1\npalfx.add = 50,60,70\n",
    );
    run(&mut w, 2);
    let a = explod_by_id(&w, 1);
    assert!(!a.ownpal);
    let b = explod_by_id(&w, 2);
    assert!(b.ownpal);
    assert!(b.palfx.active());
    assert_eq!(b.palfx.add, [50, 60, 70]);
}

/// Sasuke's Susanoo (awakening 552 -> helper 4050, `size.xscale = .6`)
/// creates its dark "Negro" copy (4460, no size) at the same scale, so the
/// two line up.
#[test]
#[ignore]
fn sasuke_susanoo_copies_line_up() {
    let Some(dir) = golden_dir() else { return };
    let sasuke = dir.join("G6_Sasuke_Rinnegan/G6_Sasuke_Rinnegan.def");
    let goku = dir.join("Goku/Goku.def");
    let mut w = community(&sasuke, &goku, FighterSize::Normal);
    w.players[0].power = w.players[0].power_max;
    w.change_state(0, 552, None, None);
    let mut seen = false;
    for _ in 0..300 {
        run(&mut w, 1);
        let body = w.players.iter().find(|p| !p.removed && p.state_no == 4050);
        let shade = w.players.iter().find(|p| !p.removed && p.state_no == 4460);
        if let (Some(b), Some(s)) = (body, shade) {
            assert_eq!(s.size.xscale, b.size.xscale);
            assert_eq!(s.size.yscale, b.size.yscale);
            assert!((b.size.xscale - 0.6).abs() < 1e-6);
            assert_eq!(s.pos, b.pos, "bound to the body");
            seen = true;
        }
    }
    assert!(seen, "Susanoo and its shade appeared");
}

/// Goku's ki blast (state 400): the glow helper (410, additive, PalFX mul
/// 356) is created before the shade (411, subtractive): the renderer draws
/// the later one behind (see the app's draw order test).
#[test]
#[ignore]
fn goku_ki_blast_glow_and_shade() {
    let Some(dir) = golden_dir() else { return };
    let sasuke = dir.join("G6_Sasuke_Rinnegan/G6_Sasuke_Rinnegan.def");
    let goku = dir.join("Goku/Goku.def");
    let mut w = community(&goku, &sasuke, FighterSize::Normal);
    w.change_state(0, 400, None, None);
    let mut seen = false;
    for _ in 0..120 {
        run(&mut w, 1);
        let glow = w
            .players
            .iter()
            .position(|p| !p.removed && p.state_no == 410);
        let shade = w
            .players
            .iter()
            .position(|p| !p.removed && p.state_no == 411);
        if let (Some(g), Some(s)) = (glow, shade) {
            use crate::explod::Trans;
            assert!(g < s, "glow queued before the shade");
            assert_eq!(w.players[g].trans, Trans::Add);
            assert_eq!(w.players[s].trans, Trans::Sub);
            assert_eq!(w.drawn_palfx(g).mul, [356; 3]);
            assert_eq!(w.drawn_palfx(s).mul, [0, 256, 512]);
            seen = true;
        }
    }
    assert!(seen, "the ki blast helpers appeared");
}
