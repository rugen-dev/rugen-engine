//! system.def: the motif shared by the title, select and versus screens
//! (fonts, system.sff / system.snd, menu and select-grid layout).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rugen_formats::def::{Def, Section};
use rugen_formats::fnt::Font;
use rugen_formats::sff::Sff;
use rugen_formats::snd::Snd;

use crate::sprites::{FontGpu, FontRef, SpriteSheet};

pub type SndRef = Option<(i32, i32)>;

fn pos2(s: &Section, k: &str, d: [f32; 2]) -> [f32; 2] {
    s.f32s(k)
        .filter(|v| v.len() >= 2)
        .map_or(d, |v| [v[0], v[1]])
}

fn spr(s: &Section, k: &str) -> Option<(i32, i32)> {
    s.i32s(k).filter(|v| v.len() >= 2).map(|v| (v[0], v[1]))
}

fn snd(s: &Section, k: &str) -> SndRef {
    spr(s, k)
}

#[derive(Clone, Debug)]
pub struct TitleInfo {
    pub menu_pos: [f32; 2],
    pub item_font: Option<FontRef>,
    pub active_font: Option<FontRef>,
    pub spacing: [f32; 2],
    /// (key, label) in display order.
    pub items: Vec<(String, String)>,
    pub visible_items: usize,
    pub box_cursor: Option<[f32; 4]>,
    /// rugen: panel sprites drawn behind each item / the selected item.
    pub item_bg: Option<(i32, i32)>,
    pub active_bg: Option<(i32, i32)>,
    pub bg_offset: [f32; 2],
    pub move_snd: SndRef,
    pub done_snd: SndRef,
    pub cancel_snd: SndRef,
}

#[derive(Clone, Debug)]
pub struct SelectInfo {
    pub rows: usize,
    pub columns: usize,
    pub wrapping: bool,
    pub pos: [f32; 2],
    pub cell_size: [f32; 2],
    pub cell_spacing: f32,
    pub cell_bg: Option<(i32, i32)>,
    #[allow(dead_code)]
    pub cell_random: Option<(i32, i32)>,
    pub cursor_start: [[i32; 2]; 2],
    pub cursor_active: [Option<(i32, i32)>; 2],
    pub cursor_done: [Option<(i32, i32)>; 2],
    pub move_snd: [SndRef; 2],
    pub done_snd: [SndRef; 2],
    pub portrait_offset: [f32; 2],
    pub portrait_scale: [f32; 2],
    pub title_offset: [f32; 2],
    pub title_font: Option<FontRef>,
    pub face_offset: [[f32; 2]; 2],
    pub face_scale: [[f32; 2]; 2],
    pub face_facing: [f32; 2],
    pub name_offset: [[f32; 2]; 2],
    pub name_font: [Option<FontRef>; 2],
    pub stage_pos: [f32; 2],
    pub stage_active_font: Option<FontRef>,
    pub stage_active2_font: Option<FontRef>,
    pub stage_done_font: Option<FontRef>,
    pub stage_move_snd: SndRef,
    pub stage_done_snd: SndRef,
    pub cancel_snd: SndRef,
}

#[derive(Clone, Debug)]
pub struct VsInfo {
    #[allow(dead_code)]
    pub time: i32,
    pub match_text: String,
    pub match_offset: [f32; 2],
    pub match_font: Option<FontRef>,
    pub face_offset: [[f32; 2]; 2],
    pub face_scale: [[f32; 2]; 2],
    pub face_facing: [f32; 2],
    pub name_offset: [[f32; 2]; 2],
    pub name_font: [Option<FontRef>; 2],
}

pub struct SystemData {
    pub def_path: PathBuf,
    pub sff_path: PathBuf,
    pub sheet: SpriteSheet,
    pub fonts: HashMap<i32, FontGpu>,
    pub snd: Option<Snd>,
    pub title: TitleInfo,
    pub select: SelectInfo,
    pub versus: VsInfo,
    pub localcoord: [f32; 2],
}

impl SystemData {
    /// `motif` holds the screenpack; files missing there come from `data_dir`.
    pub fn load(motif: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let chain = crate::themes::screenpack_chain(motif, data_dir);
        let dirs: Vec<&Path> = chain.iter().map(std::path::PathBuf::as_path).collect();
        let def_path = rugen_sim::chardata::resolve_in(&dirs, "system.def")?;
        let def = Def::open(&def_path).with_context(|| def_path.display().to_string())?;
        let files = def.section("files").context("system.def has no [Files]")?;
        let sff_name = files.raw("spr").unwrap_or("system.sff").trim();
        let sff_path = rugen_sim::chardata::resolve_in(&dirs, sff_name)?;
        let sff = Sff::open(&sff_path, false).with_context(|| sff_path.display().to_string())?;
        let sounds = files
            .raw("snd")
            .and_then(|n| rugen_sim::chardata::resolve_in(&dirs, n.trim()).ok())
            .and_then(|p| Snd::open(p).ok());
        let root = data_dir.parent().unwrap_or(data_dir);
        let mut fonts = HashMap::new();
        for i in 1..=10 {
            let Some(name) = files.raw(&format!("font{i}")) else {
                continue;
            };
            let name = name.trim();
            let mut candidates: Vec<std::path::PathBuf> = Vec::new();
            for d in &chain {
                candidates.push(d.join(name));
                candidates.push(d.join("font").join(name));
            }
            candidates.push(root.join("font").join(name));
            candidates.push(root.join(name));
            let Some(path) = candidates.iter().find(|p| p.exists()) else {
                log::warn!("system.def: font{i} = {name} not found");
                continue;
            };
            match Font::open(path) {
                Ok(f) => {
                    fonts.insert(i, FontGpu::new(f));
                }
                Err(e) => log::warn!("font {name}: {e}"),
            }
        }
        let empty = Section {
            name: String::new(),
            rest: String::new(),
            line: 0,
            entries: Default::default(),
        };
        // section names with spaces: `[Title Info]` parses as name "title", rest "Info"
        let sec = |n: &str| {
            def.sections
                .iter()
                .find(|s| {
                    let full = format!("{} {}", s.name, s.rest);
                    full.trim().eq_ignore_ascii_case(n)
                })
                .unwrap_or(&empty)
        };
        let ti = sec("title info");
        let si = sec("select info");
        let vs = sec("vs screen");
        let localcoord = def
            .section("info")
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);

        // menu items appear in file order
        let mut items = Vec::new();
        for (k, v) in &ti.entries {
            if let Some(key) = k.strip_prefix("menu.itemname.") {
                let label = v.trim().trim_matches('"').to_string();
                if !label.is_empty() {
                    items.push((key.to_string(), label));
                }
            }
        }
        if items.is_empty() {
            items = [
                ("arcade", "ARCADE"),
                ("versus", "VS MODE"),
                ("training", "TRAINING"),
                ("watch", "WATCH"),
                ("exit", "EXIT"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        }
        let title = TitleInfo {
            menu_pos: pos2(ti, "menu.pos", [159.0, 158.0]),
            item_font: FontRef::parse(ti, "menu.item.font"),
            active_font: FontRef::parse(ti, "menu.item.active.font"),
            spacing: pos2(ti, "menu.item.spacing", [0.0, 13.0]),
            items,
            visible_items: ti.i32("menu.window.visibleitems").unwrap_or(5).max(1) as usize,
            box_cursor: ti
                .f32s("menu.boxcursor.coords")
                .filter(|v| v.len() >= 4 && ti.i32("menu.boxcursor.visible").unwrap_or(1) != 0)
                .map(|v| [v[0], v[1], v[2], v[3]]),
            item_bg: spr(ti, "menu.item.bg.spr"),
            active_bg: spr(ti, "menu.item.active.bg.spr"),
            bg_offset: pos2(ti, "menu.item.bg.offset", [0.0, 0.0]),
            move_snd: snd(ti, "cursor.move.snd"),
            done_snd: snd(ti, "cursor.done.snd"),
            cancel_snd: snd(ti, "cancel.snd"),
        };
        let cell = |k: &str| {
            si.i32s(k)
                .filter(|v| v.len() >= 2)
                .map_or([0, 0], |v| [v[0], v[1]])
        };
        let select = SelectInfo {
            rows: si.i32("rows").unwrap_or(2).max(1) as usize,
            columns: si.i32("columns").unwrap_or(5).max(1) as usize,
            wrapping: si.i32("wrapping").unwrap_or(0) != 0,
            pos: pos2(si, "pos", [90.0, 170.0]),
            cell_size: pos2(si, "cell.size", [27.0, 27.0]),
            cell_spacing: si.f32("cell.spacing").unwrap_or(2.0),
            cell_bg: spr(si, "cell.bg.spr"),
            cell_random: spr(si, "cell.random.spr"),
            cursor_start: [cell("p1.cursor.startcell"), cell("p2.cursor.startcell")],
            cursor_active: [
                spr(si, "p1.cursor.active.spr"),
                spr(si, "p2.cursor.active.spr"),
            ],
            cursor_done: [spr(si, "p1.cursor.done.spr"), spr(si, "p2.cursor.done.spr")],
            move_snd: [snd(si, "p1.cursor.move.snd"), snd(si, "p2.cursor.move.snd")],
            done_snd: [snd(si, "p1.cursor.done.snd"), snd(si, "p2.cursor.done.snd")],
            portrait_offset: pos2(si, "portrait.offset", [0.0, 0.0]),
            portrait_scale: pos2(si, "portrait.scale", [1.0, 1.0]),
            title_offset: pos2(si, "title.offset", [159.0, 13.0]),
            title_font: FontRef::parse(si, "title.font"),
            face_offset: [
                pos2(si, "p1.face.offset", [18.0, 13.0]),
                pos2(si, "p2.face.offset", [301.0, 13.0]),
            ],
            face_scale: [
                pos2(si, "p1.face.scale", [1.0, 1.0]),
                pos2(si, "p2.face.scale", [1.0, 1.0]),
            ],
            face_facing: [
                si.f32("p1.face.facing").unwrap_or(1.0),
                si.f32("p2.face.facing").unwrap_or(-1.0),
            ],
            name_offset: [
                pos2(si, "p1.name.offset", [10.0, 162.0]),
                pos2(si, "p2.name.offset", [309.0, 162.0]),
            ],
            name_font: [
                FontRef::parse(si, "p1.name.font"),
                FontRef::parse(si, "p2.name.font"),
            ],
            stage_pos: pos2(si, "stage.pos", [160.0, 237.0]),
            stage_active_font: FontRef::parse(si, "stage.active.font"),
            stage_active2_font: FontRef::parse(si, "stage.active2.font"),
            stage_done_font: FontRef::parse(si, "stage.done.font"),
            stage_move_snd: snd(si, "stage.move.snd"),
            stage_done_snd: snd(si, "stage.done.snd"),
            cancel_snd: snd(si, "cancel.snd"),
        };
        let versus = VsInfo {
            time: vs.i32("time").unwrap_or(150),
            match_text: vs
                .raw("match.text")
                .map(|t| t.trim().trim_matches('"').to_string())
                .unwrap_or_default(),
            match_offset: pos2(vs, "match.offset", [159.0, 12.0]),
            match_font: FontRef::parse(vs, "match.font"),
            face_offset: [
                pos2(vs, "p1.offset", [20.0, 31.0]),
                pos2(vs, "p2.offset", [299.0, 31.0]),
            ],
            face_scale: [
                pos2(vs, "p1.scale", [1.0, 1.0]),
                pos2(vs, "p2.scale", [1.0, 1.0]),
            ],
            face_facing: [
                vs.f32("p1.facing").unwrap_or(1.0),
                vs.f32("p2.facing").unwrap_or(-1.0),
            ],
            name_offset: [
                pos2(vs, "p1.name.offset", [78.0, 190.0]),
                pos2(vs, "p2.name.offset", [241.0, 190.0]),
            ],
            name_font: [
                FontRef::parse(vs, "p1.name.font"),
                FontRef::parse(vs, "p2.name.font"),
            ],
        };
        Ok(Self {
            def_path,
            sff_path,
            sheet: SpriteSheet::new(sff),
            fonts,
            snd: sounds,
            title,
            select,
            versus,
            localcoord,
        })
    }

    /// Draw text with one of the system fonts at a system.def (top-left origin) position.
    pub fn text(
        &mut self,
        r: &mut rugen_render::Renderer,
        f: Option<FontRef>,
        line: &str,
        pos: [f32; 2],
    ) {
        let Some(f) = f else { return };
        match self.fonts.get_mut(&f.index) {
            Some(font) => {
                font.draw(
                    r,
                    line,
                    [pos[0] - self.localcoord[0] / 2.0, pos[1]],
                    f.bank,
                    f.align,
                    1.0,
                );
            }
            None => log::warn!("system.def: font{} is not loaded (text {line:?})", f.index),
        }
    }
}
