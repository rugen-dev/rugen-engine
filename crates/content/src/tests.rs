use super::*;
use std::io::Write;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rugen-content-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("chars")).unwrap();
    d
}

/// A one-sprite SFF whose pixel is `c` (9000,0 portrait included).
fn tiny_sff(c: u8) -> Vec<u8> {
    let mut sff = rugen_formats::sff::Sff {
        version: Some(rugen_formats::sff::Version::V2 { patch: 1 }),
        ..Default::default()
    };
    let mut pal = vec![[0u8; 4]; 256];
    pal[1] = [200, 10, 10, 255];
    pal[2] = [10, 200, 10, 255];
    let p = sff.push_palette((1, 1), pal);
    sff.push_indexed((0, 0), 3, 2, [1, 2], p, vec![0, c, 0, 0, c, 0]);
    sff.push_indexed((9000, 0), 2, 2, [0, 0], p, vec![1, 2, 2, 1]);
    sff.write_exact()
}

fn tiny_files(c: u8) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "tiny.def",
            b"[Info]\nname = \"Tiny\"\ndisplayname = \"Tiny Guy\"\n[Files]\ncmd = tiny.cmd\ncns = tiny.cns\nsprite = tiny.sff\nanim = tiny.air\nstcommon = common1.cns\n".to_vec(),
        ),
        ("tiny.cmd", b"[Statedef -1]\n".to_vec()),
        (
            "tiny.cns",
            b"[Data]\nlife = 1000\n[Statedef 200]\nmovetype = A\n[State 200, h]\ntype = HitDef\ntrigger1 = 1\ndamage = 40\nguardflag = MA\n".to_vec(),
        ),
        ("tiny.sff", tiny_sff(c)),
        ("tiny.air", b"[Begin Action 0]\n0,0, 0,0, 10\n".to_vec()),
        ("common1.cns", b"[Statedef 0]\n".to_vec()),
        ("readme.txt", b"made by me".to_vec()),
    ]
}

fn write_folder(dir: &Path, c: u8) {
    fs::create_dir_all(dir).unwrap();
    for (n, b) in tiny_files(c) {
        fs::write(dir.join(n), b).unwrap();
    }
}

fn write_zip(path: &Path, folder: &str, c: u8) {
    let f = fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o = zip::write::SimpleFileOptions::default();
    for (n, b) in tiny_files(c).into_iter().rev() {
        z.start_file(format!("{folder}/{n}"), o).unwrap();
        z.write_all(&b).unwrap();
    }
    z.start_file(format!("{folder}/Thumbs.db"), o).unwrap();
    z.write_all(b"junk").unwrap();
    z.finish().unwrap();
}

#[test]
fn archives_become_content_files() {
    let root = tmp("import");
    write_zip(&root.join("chars/Tiny.zip"), "Tiny v2", 1);
    let lib = Library::discover(Some(&root));
    assert_eq!(lib.root, root);
    let chars = lib.chars();
    assert_eq!(chars.len(), 1);
    assert_eq!(chars[0].kind, Kind::Zip);
    let (dir, m) = lib.prepare_char(&chars[0]).unwrap();
    let rgc_path = root.join("chars/Tiny.rgc");
    assert!(rgc_path.is_file(), "converted next to the archive");
    assert!(!root.join("chars/Tiny.zip").exists());
    assert!(root.join("chars/originals/Tiny.zip").is_file(), "kept");
    assert_eq!(m.def, "tiny.def", "found inside the single folder");
    assert_eq!(m.display_name, "Tiny Guy");
    assert_eq!(m.content.as_deref(), Some(rgc_path.as_path()));
    assert_eq!(m.def_path(&dir), rgc_path.join("tiny.def"));
    assert!(m.fingerprint.is_some() && m.profile.is_some());
    assert!(m.portrait_small.is_some() && m.card.is_some());
    // nothing of the junk, everything else
    let a = rgc::Archive::open(&rgc_path).unwrap();
    assert!(a.find("readme.txt").is_some());
    assert!(a.find("Thumbs.db").is_none());
    assert_eq!(a.find("tiny.sff").unwrap().kind, rgc::SectionKind::Sprites);
    assert!(a.logic().is_some(), "logic compiled in");
    // the scanner now sees the content file only; a second prepare is cached
    let chars = lib.chars();
    assert_eq!(chars.len(), 1);
    assert_eq!(chars[0].kind, Kind::Content);
    let (dir2, m2) = lib.prepare_char(&chars[0]).unwrap();
    assert_eq!(dir, dir2);
    assert_eq!(m2.fingerprint, m.fingerprint);
    // the character plays from it
    let data = rugen_sim::CharData::load_with_data(&m.def_path(&dir), &lib.data_dir()).unwrap();
    assert_eq!(data.display_name, "Tiny Guy");
    assert_eq!(data.sff.sprites.len(), 2);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn originals_can_be_deleted_and_import_runs_on_launch() {
    let root = tmp("nokeep");
    write_zip(&root.join("chars/Tiny.zip"), "Tiny", 1);
    let mut lib = Library::discover(Some(&root));
    lib.keep_originals = false;
    let done = lib.import_new();
    assert_eq!(done.len(), 1);
    assert!(done[0].result.is_ok(), "{:?}", done[0].result);
    assert!(root.join("chars/Tiny.rgc").is_file());
    assert!(!root.join("chars/Tiny.zip").exists());
    assert!(!root.join("chars/originals").exists());
    // a second launch has nothing to do
    assert!(lib.import_new().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fingerprint_is_container_independent_and_packages_install() {
    let root = tmp("fp");
    write_folder(&root.join("chars/Folder"), 1);
    write_zip(&root.join("chars/Zipped.zip"), "Tiny v2", 1);
    let rar = std::process::Command::new("rar")
        .current_dir(root.join("chars"))
        .args(["a", "-r", "-inul", "Rarred.rar", "Folder"])
        .status()
        .is_ok_and(|s| s.success());
    let lib = Library::discover(Some(&root));
    let fp = |id: &str| {
        let e = lib.find_char(id).unwrap();
        lib.prepare_char(&e).unwrap().1.fingerprint.unwrap()
    };
    let a = fp("Folder");
    assert_eq!(fp("Zipped"), a, "zip == folder");
    if rar {
        assert_eq!(fp("Rarred"), a, "rar == folder");
    }
    // the folder is a working copy: still there, converted into the cache
    assert!(root.join("chars/Folder/tiny.def").is_file());
    let e = lib.find_char("Folder").unwrap();
    let (dir, m) = lib.prepare_char(&e).unwrap();
    assert!(
        m.content
            .as_ref()
            .unwrap()
            .starts_with(root.join("cache/rgc/chars"))
    );
    assert_eq!(m.profile.as_ref().unwrap().max_hit_damage, 40);
    let pkg = lib.package(&dir, &m).unwrap();
    // a second library ("the opponent") installs it into its net cache
    let other = root.join("other");
    fs::create_dir_all(other.join("chars")).unwrap();
    let lib2 = Library::discover(Some(&other));
    assert!(lib2.find_fingerprint(&a).is_none());
    let part = other.join("download.part");
    fs::copy(&pkg, &part).unwrap();
    let (d2, m2) = lib2.install_package(&part, &a, 1 << 20).unwrap();
    assert!(d2.starts_with(other.join("cache/net/chars")));
    assert_eq!(m2.fingerprint.as_deref(), Some(a.as_str()));
    assert_eq!(m2.display_name, "Tiny Guy");
    assert!(m2.content.as_ref().unwrap().is_file());
    assert!(lib2.find_fingerprint(&a).is_some());
    let part = other.join("download2.part");
    fs::copy(&pkg, &part).unwrap();
    assert!(
        lib2.install_package(&part, &"0".repeat(64), 1 << 20)
            .is_err()
    );
    // too big
    let part = other.join("download3.part");
    fs::copy(&pkg, &part).unwrap();
    let lib3 = Library::discover(Some(&root.join("third")));
    assert!(lib3.install_package(&part, &a, 10).is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn folders_reconvert_when_edited() {
    let root = tmp("folder");
    write_folder(&root.join("chars/T"), 1);
    let lib = Library::discover(Some(&root));
    let e = lib.find_char("T").unwrap();
    let (dir, m) = lib.prepare_char(&e).unwrap();
    let def = m.def_path(&dir);
    assert!(
        !refresh_working_copy(&def, &lib.data_dir())
            .unwrap()
            .unwrap(),
        "unchanged folder"
    );
    // edit the cns: the working copy is converted again, sprites reused
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        root.join("chars/T/tiny.cns"),
        b"[Data]\nlife = 900\n[Statedef 200]\nmovetype = A\n[State 200, h]\ntype = HitDef\ntrigger1 = 1\ndamage = 40\nguardflag = MA\n",
    )
    .unwrap();
    assert!(
        refresh_working_copy(&def, &lib.data_dir())
            .unwrap()
            .unwrap()
    );
    let data = rugen_sim::CharData::load_with_data(&def, &lib.data_dir()).unwrap();
    assert_eq!(
        data.konst("data.life"),
        Some(900.0),
        "hot reload sees the edit"
    );
    let (dir2, m2) = lib.prepare_char(&e).unwrap();
    assert_ne!(dir2, dir, "new content, new manifest");
    assert_ne!(m2.fingerprint, m.fingerprint);
    assert!(!dir.exists(), "the stale manifest went away");
    assert!(
        root.join("chars/T/tiny.cns").is_file(),
        "folders are never moved"
    );
    // an unchanged folder is not converted again
    let stamp = rugen_formats::pack::Stamp::of(m2.content.as_ref().unwrap());
    lib.prepare_char(&e).unwrap();
    assert_eq!(
        rugen_formats::pack::Stamp::of(m2.content.as_ref().unwrap()),
        stamp
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn content_files_dropped_in_load_as_is() {
    let root = tmp("drop");
    write_folder(&root.join("src/Tiny"), 2);
    let lib = Library::discover(Some(&root));
    let src = convert::Source {
        root: root.join("src/Tiny"),
        def: "tiny.def".into(),
        kind: rgc::Kind::Char,
        name: "Tiny".into(),
    };
    let report =
        convert::convert(&src, &root.join("chars/Shared.rgc"), &lib.data_dir(), None).unwrap();
    let e = lib.find_char("Shared").unwrap();
    assert_eq!(e.kind, Kind::Content);
    let (_, m) = lib.prepare_char(&e).unwrap();
    assert_eq!(m.fingerprint, report.fingerprint);
    assert!(root.join("chars/Shared.rgc").is_file(), "left where it is");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn export_gives_back_a_folder_with_the_same_fingerprint() {
    let root = tmp("export");
    write_folder(&root.join("src/Tiny"), 1);
    // a sound file and a png too
    let wav = rugen_formats::wav::write_pcm16(
        22050,
        1,
        &(0..2205)
            .map(|i| ((i as f64 * 0.3).sin() * 8000.0) as i16)
            .collect::<Vec<_>>(),
    );
    let mut snd = Vec::new();
    snd.extend_from_slice(b"ElecbyteSnd\0");
    snd.extend_from_slice(&[0, 0, 1, 0]);
    snd.extend_from_slice(&1u32.to_le_bytes());
    snd.extend_from_slice(&24u32.to_le_bytes());
    snd.extend_from_slice(&0u32.to_le_bytes());
    snd.extend_from_slice(&(wav.len() as u32).to_le_bytes());
    snd.extend_from_slice(&5u32.to_le_bytes());
    snd.extend_from_slice(&0u32.to_le_bytes());
    snd.extend_from_slice(&wav);
    fs::write(root.join("src/Tiny/tiny.snd"), &snd).unwrap();
    let def = fs::read_to_string(root.join("src/Tiny/tiny.def")).unwrap();
    fs::write(root.join("src/Tiny/tiny.def"), def + "sound = tiny.snd\n").unwrap();
    write_png(&root.join("src/Tiny/Big Art.png"), 3, 2, &[9; 24]).unwrap();
    let lib = Library::discover(Some(&root));
    let src = convert::Source {
        root: root.join("src/Tiny"),
        def: "tiny.def".into(),
        kind: rgc::Kind::Char,
        name: "Tiny".into(),
    };
    let first = root.join("a.rgc");
    let r1 = convert::convert(&src, &first, &lib.data_dir(), None).unwrap();
    let out = root.join("exported");
    let ex = export::export(&first, &out, false).unwrap();
    assert_eq!(ex.def, "tiny.def");
    assert!(out.join("readme.txt").is_file() && out.join("Big Art.png").is_file());
    // the sound came back as a WAV of the same length
    let back = rugen_formats::snd::Snd::open(out.join("tiny.snd")).unwrap();
    let w = rugen_formats::wav::parse(back.get(5, 0).unwrap()).unwrap();
    assert_eq!((w.rate, w.frames()), (24000, 2400));
    // the image is the same picture
    let (iw, ih, px) = read_png(&out.join("Big Art.png")).unwrap();
    assert_eq!((iw, ih), (3, 2));
    assert!(px.chunks(4).all(|c| c[..3] == [9, 9, 9]));
    // sprites identical
    let a = rugen_formats::sff::Sff::open(root.join("src/Tiny/tiny.sff"), true).unwrap();
    let b = rugen_formats::sff::Sff::open(out.join("tiny.sff"), true).unwrap();
    assert_eq!(a.canonical_hash(), b.canonical_hash());
    // and the exported folder converts to the same fingerprint
    let src2 = convert::Source {
        root: out.clone(),
        def: ex.def.clone(),
        kind: rgc::Kind::Char,
        name: "exported".into(),
    };
    let r2 = convert::convert(&src2, &root.join("b.rgc"), &lib.data_dir(), None).unwrap();
    assert_eq!(r1.fingerprint, r2.fingerprint);
    assert!(
        export::export(&first, &out, false).is_err(),
        "no silent overwrite"
    );
    assert!(export::export(&first, &out, true).is_ok());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn broken_content_still_converts_without_a_fingerprint() {
    let root = tmp("broken");
    write_folder(&root.join("chars/B"), 1);
    fs::write(root.join("chars/B/tiny.sff"), b"not an sff").unwrap();
    let lib = Library::discover(Some(&root));
    let e = lib.find_char("B").unwrap();
    let (_, m) = lib.prepare_char(&e).unwrap();
    assert!(m.fingerprint.is_none() && m.fair_error.is_some());
    let a = rgc::Archive::open(m.content.as_ref().unwrap()).unwrap();
    assert_eq!(
        a.find("tiny.sff").unwrap().kind,
        rgc::SectionKind::File,
        "kept as it was"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn old_caches_are_removed_once_converted() {
    let root = tmp("legacy");
    write_zip(&root.join("chars/Tiny.zip"), "Tiny", 1);
    let lib = Library::discover(Some(&root));
    let e = lib.find_char("Tiny").unwrap();
    // what a v6 engine left: an extracted copy with optimiser output
    let old = lib.legacy_dir_name(&e, false);
    write_folder(&old, 1);
    fs::create_dir_all(old.join(".rugen-opt/tiny")).unwrap();
    fs::write(old.join(".rugen-opt/tiny/sprites.rspk"), b"x").unwrap();
    fs::write(
        old.join("rugen.json"),
        r#"{"cache_version":6,"id":"Tiny","def":"tiny.def","display_name":"T","source_hash":"x",
            "verify":{"verdict":"verified","issues":[],"engine_version":"0.4.0","when":5}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("cache/packages")).unwrap();
    fs::write(root.join("cache/packages/abc.rgpk"), b"old").unwrap();
    let done = lib.import_new();
    assert!(done.iter().all(|d| d.result.is_ok()));
    assert!(!old.exists(), "extracted copy removed");
    assert!(!root.join("cache/packages").exists());
    let (_, m) = lib.prepare_char(&lib.find_char("Tiny").unwrap()).unwrap();
    assert_eq!(
        m.verify.as_ref().map(|v| v.verdict.as_str()),
        Some("verified"),
        "verdict carried over"
    );
    let _ = fs::remove_dir_all(&root);
}

/// User themes: any folder of `themes/` with a `system.def` is a theme,
/// listed after the bundled ones; bundled names and odd ids are refused.
#[test]
fn user_themes_are_listed_after_the_bundled_ones() {
    let root = tmp("themes");
    let lib = Library::portable(root.clone());
    let bundled = root.join("motif/themes/ink");
    fs::create_dir_all(bundled.join("ui")).unwrap();
    fs::write(bundled.join("system.def"), "[Files]\n").unwrap();
    for (name, def) in [
        ("neon", true),
        ("Amber", true),
        ("notes", false),
        ("ink", true),
    ] {
        let d = root.join("themes").join(name);
        fs::create_dir_all(&d).unwrap();
        if def {
            fs::write(d.join("system.def"), "[Files]\n").unwrap();
        }
    }
    assert_eq!(lib.user_themes(), ["Amber", "neon"]);
    assert_eq!(lib.themes(), ["ink", "Amber", "neon"]);
    assert_eq!(lib.motif_dir_for("neon"), root.join("themes/neon"));
    // a bundled theme always wins over a folder of the same name
    assert_eq!(lib.motif_dir_for("ink"), bundled);
    assert_eq!(lib.user_theme_dir("ink"), None);
    for bad in ["notes", "../neon", "neon/..", ".", "", "classic"] {
        assert_eq!(lib.user_theme_dir(bad), None, "{bad:?}");
    }
    let _ = fs::remove_dir_all(&root);
}
