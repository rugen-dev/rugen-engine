//! MUGEN storyboards (cutscenes): a `.def` with `[SceneDef]` and `[Scene n]`
//! groups, the `[Begin Action n]` animations its layers show, text layers,
//! sounds, music and `bg.name` backgrounds (MUGEN 1.1 `storyboard.html`,
//! timing as Ikemen GO's `storyboard.go`).
//!
//! The shell plays the screenpack's `logo.storyboard` / `intro.storyboard`
//! (system.def `[Files]`) once at launch, and in ARCADE the fighter's
//! `intro.storyboard` before the first fight and `ending.storyboard` after a
//! cleared ladder (character .def `[Arcade]`). START or Esc skips the whole
//! storyboard, a fight button the current scene. `rugen storyboard
//! <file.def>` plays one alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rugen_audio::{Audio, BankId, SoundKey};
use rugen_formats::air::{Air, Frame};
use rugen_formats::def::{Def, Section};
use rugen_formats::fnt::Font;
use rugen_formats::sff::Sff;
use rugen_formats::snd::Snd;
use rugen_render::{Camera, Draw, Renderer};

use crate::sprites::{Anim, FontGpu, SpriteSheet};
use crate::stage::StageVisual;

/// Ticks of the fade to black when a storyboard is skipped.
pub const SKIP_FADE: i32 = 12;

/// A fade at the start or the end of a scene.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fade {
    pub time: i32,
    /// RGB 0..1 (MUGEN supports black and white only; any colour works here).
    pub color: [f32; 3],
}

/// One `layerX.*` group: an animation or a text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub index: i32,
    pub anim: Option<i32>,
    pub text: Option<String>,
    /// `font = font_no, bank, align` (r, g, b only matter for TrueType fonts).
    pub font: [i32; 3],
    /// Ticks between two letters of the text (0 = all at once).
    pub text_delay: i32,
    pub offset: [f32; 2],
    pub start: i32,
    /// Last tick + 1 (0 = until the scene ends).
    pub end: i32,
}

impl Layer {
    /// Drawn (and animated) at scene tick `t`.
    pub fn visible(&self, t: i32) -> bool {
        t >= self.start && (self.end <= 0 || t < self.end)
    }

    /// The text as shown at scene tick `t` (letters typed so far).
    pub fn text_at(&self, t: i32) -> String {
        let text = self.text.as_deref().unwrap_or("");
        if self.text_delay <= 0 {
            return text.to_string();
        }
        let n = ((t - self.start).max(0) / self.text_delay + 1) as usize;
        text.chars().take(n).collect()
    }
}

/// `soundX.*`: a sound of the storyboard's `.snd` at a scene tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub group: i32,
    pub number: i32,
    pub start: i32,
    /// Percent.
    pub volume: i32,
    /// -127 (left) .. 127 (right).
    pub pan: i32,
}

/// `bgm`: what the music does when a scene starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Bgm {
    Play { file: String, looping: bool },
    Stop,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub end_time: i32,
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// `None` = the screen is not cleared (drawn on black here).
    pub clear: Option<[f32; 3]>,
    pub pos: [f32; 2],
    /// By layer number, bottom first.
    pub layers: Vec<Layer>,
    pub sounds: Vec<Cue>,
    /// `None` = the music goes on as it is.
    pub bgm: Option<Bgm>,
    /// `x1, y1, x2, y2` drawing window for the layers.
    pub window: Option<[f32; 4]>,
    /// `bg.name`: background `[<name>Def]` + `[<name> ...]` groups.
    pub bg: Option<String>,
}

/// A parsed storyboard.
#[derive(Clone, Debug, Default)]
pub struct Story {
    pub localcoord: [f32; 2],
    /// Sprite file (required by MUGEN; a storyboard of texts and colours
    /// works without one here).
    pub spr: Option<PathBuf>,
    pub snd: Option<PathBuf>,
    pub fonts: Vec<(i32, PathBuf)>,
    pub start_scene: usize,
    pub scenes: Vec<Scene>,
}

fn rgb(s: &Section, key: &str) -> Option<[f32; 3]> {
    s.i32s(key).filter(|v| v.len() >= 3).map(|v| {
        [
            v[0].clamp(0, 255) as f32 / 255.0,
            v[1].clamp(0, 255) as f32 / 255.0,
            v[2].clamp(0, 255) as f32 / 255.0,
        ]
    })
}

fn pair(s: &Section, key: &str) -> Option<[f32; 2]> {
    s.f32s(key).map(|v| {
        [
            v.first().copied().unwrap_or(0.0),
            v.get(1).copied().unwrap_or(0.0),
        ]
    })
}

/// `layer3.anim` → `(3, "anim")` for keys starting with `prefix`.
fn indexed<'a>(key: &'a str, prefix: &str) -> Option<(i32, &'a str)> {
    let rest = key.strip_prefix(prefix)?;
    let rest = rest.strip_prefix('_').unwrap_or(rest);
    let (n, field) = rest.split_once('.')?;
    Some((n.parse().ok()?, field))
}

/// A text value: quotes stripped, `\n` turned into line breaks.
fn text_value(raw: &str) -> String {
    let v = raw.trim();
    let v = v
        .strip_prefix('"')
        .map(|v| v.strip_suffix('"').unwrap_or(v))
        .unwrap_or(v);
    v.replace("\\n", "\n")
}

/// A file named by the storyboard: next to it, else in `fallback` dirs
/// (case-insensitive, inside content files too).
fn find(dir: &Path, name: &str, fallback: &[PathBuf]) -> Option<PathBuf> {
    let name = name.trim().trim_matches('"').replace('\\', "/");
    if name.is_empty() {
        return None;
    }
    std::iter::once(dir)
        .chain(fallback.iter().map(PathBuf::as_path))
        .find_map(|d| rugen_formats::vfs::resolve(d, &name))
}

impl Story {
    /// Parse a storyboard; files resolve against `dir`, then `fallback`
    /// (fonts: `font/` there too).
    pub fn parse(text: &str, dir: &Path, fallback: &[PathBuf]) -> anyhow::Result<Self> {
        let def = Def::parse(text);
        let localcoord = def
            .section("info")
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let sd = def
            .section("scenedef")
            .context("no [SceneDef] group: not a storyboard")?;
        let spr = sd.raw("spr").and_then(|n| find(dir, n, fallback));
        let snd = sd.raw("snd").and_then(|n| find(dir, n, fallback));
        let mut fonts = Vec::new();
        let font_dirs: Vec<PathBuf> = fallback
            .iter()
            .flat_map(|d| [d.join("font"), d.clone()])
            .collect();
        for i in 0..10 {
            if let Some(p) = sd
                .raw(&format!("font{i}"))
                .and_then(|n| find(dir, n, &font_dirs))
            {
                fonts.push((i, p));
            }
        }
        let start_scene = sd.i32("startscene").unwrap_or(0).max(0) as usize;
        // clearcolor, layerall.pos and window carry over from scene to scene
        let mut clear = None;
        let mut pos = [0.0, 0.0];
        let mut window = None;
        let mut scenes = Vec::new();
        for s in def.sections_named("scene") {
            if let Some(c) = s.i32s("clearcolor") {
                clear = (c.first().copied().unwrap_or(0) >= 0)
                    .then(|| rgb(s, "clearcolor"))
                    .flatten();
            }
            if let Some(p) = pair(s, "layerall.pos") {
                pos = p;
            }
            if let Some(w) = s.f32s("window").filter(|v| v.len() >= 4) {
                window = Some([w[0], w[1], w[2], w[3]]);
            }
            let fade = |k: &str| Fade {
                time: s.i32(&format!("{k}.time")).unwrap_or(0).max(0),
                color: rgb(s, &format!("{k}.col")).unwrap_or([0.0; 3]),
            };
            let mut layers: HashMap<i32, Layer> = HashMap::new();
            let mut sounds: HashMap<i32, Cue> = HashMap::new();
            for (key, value) in &s.entries {
                if let Some((n, field)) = indexed(key, "layer") {
                    let l = layers.entry(n).or_insert_with(|| Layer {
                        index: n,
                        font: [-1, 0, 0],
                        ..Default::default()
                    });
                    match field {
                        "anim" => l.anim = Some(rugen_formats::text::atoi(value)),
                        "text" => l.text = Some(text_value(value)),
                        "font" => {
                            let v: Vec<i32> =
                                value.split(',').map(rugen_formats::text::atoi).collect();
                            l.font = [
                                v.first().copied().unwrap_or(-1),
                                v.get(1).copied().unwrap_or(0),
                                v.get(2).copied().unwrap_or(0),
                            ];
                        }
                        "textdelay" => l.text_delay = rugen_formats::text::atoi(value).max(0),
                        "offset" => l.offset = pair(s, key).unwrap_or_default(),
                        "starttime" => l.start = rugen_formats::text::atoi(value),
                        "endtime" => l.end = rugen_formats::text::atoi(value),
                        _ => {}
                    }
                } else if let Some((n, field)) = indexed(key, "sound") {
                    let c = sounds.entry(n).or_insert(Cue {
                        group: -1,
                        number: 0,
                        start: 0,
                        volume: 100,
                        pan: 0,
                    });
                    match field {
                        "value" => {
                            let v: Vec<i32> =
                                value.split(',').map(rugen_formats::text::atoi).collect();
                            c.group = v.first().copied().unwrap_or(-1);
                            c.number = v.get(1).copied().unwrap_or(0);
                        }
                        "starttime" => c.start = rugen_formats::text::atoi(value),
                        "volumescale" | "volume" => {
                            c.volume = rugen_formats::text::atof(value).unwrap_or(100.0) as i32
                        }
                        "pan" => c.pan = rugen_formats::text::atoi(value).clamp(-127, 127),
                        _ => {}
                    }
                }
            }
            let mut layers: Vec<Layer> = layers
                .into_values()
                .filter(|l| l.anim.is_some() || l.text.is_some())
                .collect();
            layers.sort_by_key(|l| l.index);
            let mut sounds: Vec<Cue> = sounds.into_values().filter(|c| c.group >= 0).collect();
            sounds.sort_by_key(|c| (c.start, c.group, c.number));
            let bgm = s.raw("bgm").map(|v| {
                let file = text_value(v);
                if file.is_empty() {
                    Bgm::Stop
                } else {
                    Bgm::Play {
                        file,
                        looping: s.bool("bgm.loop").unwrap_or(false),
                    }
                }
            });
            scenes.push(Scene {
                end_time: s.i32("end.time").unwrap_or(0).max(1),
                fade_in: fade("fadein"),
                fade_out: fade("fadeout"),
                clear,
                pos,
                layers,
                sounds,
                bgm,
                window,
                bg: s
                    .raw("bg.name")
                    .map(|v| text_value(v).trim().to_string())
                    .filter(|n| !n.is_empty()),
            });
        }
        anyhow::ensure!(!scenes.is_empty(), "no [Scene] group");
        Ok(Self {
            localcoord,
            spr,
            snd,
            fonts,
            start_scene: start_scene.min(scenes.len() - 1),
            scenes,
        })
    }
}

/// What a tick of the clock asks the player to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A scene starts: reset its animations and background.
    Scene(usize),
    Music(Bgm),
    Sound(Cue),
}

/// Where a storyboard is: scene, tick in the scene, and the skip fade.
#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub scene: usize,
    /// Ticks since the scene started.
    pub t: i32,
    /// Skipped: ticks left of the fade to black.
    pub skipping: Option<i32>,
    pub done: bool,
}

impl Clock {
    pub fn new(story: &Story) -> Self {
        Self {
            scene: story.start_scene,
            t: 0,
            skipping: None,
            done: story.scenes.is_empty(),
        }
    }

    /// One tick: the scene's events at the current tick, then time moves
    /// on (to the next scene once `end.time` is reached).
    pub fn step(&mut self, story: &Story, out: &mut Vec<Event>) {
        if self.done {
            return;
        }
        if let Some(left) = self.skipping.as_mut() {
            *left -= 1;
            if *left <= 0 {
                self.done = true;
            }
            return;
        }
        let Some(scene) = story.scenes.get(self.scene) else {
            self.done = true;
            return;
        };
        if self.t == 0 {
            out.push(Event::Scene(self.scene));
            if let Some(b) = &scene.bgm {
                out.push(Event::Music(b.clone()));
            }
        }
        out.extend(
            scene
                .sounds
                .iter()
                .filter(|c| c.start == self.t)
                .map(|c| Event::Sound(*c)),
        );
        self.t += 1;
        if self.t >= scene.end_time {
            self.scene += 1;
            self.t = 0;
            if self.scene >= story.scenes.len() {
                self.done = true;
            }
        }
    }

    /// START / Esc: fade to black and end.
    pub fn skip(&mut self) {
        if !self.done && self.skipping.is_none() {
            self.skipping = Some(SKIP_FADE);
        }
    }

    /// A fight button: on to the scene's fade-out (the next scene follows).
    pub fn next_scene(&mut self, story: &Story) {
        if let Some(s) = story.scenes.get(self.scene)
            && self.skipping.is_none()
        {
            let out = (s.end_time - s.fade_out.time).max(0);
            self.t = self.t.max(out.min(s.end_time - 1));
        }
    }

    /// The screen fade at this moment: colour and opacity (0 = none).
    pub fn fade(&self, story: &Story) -> ([f32; 3], f32) {
        if let Some(left) = self.skipping {
            return ([0.0; 3], 1.0 - left.max(0) as f32 / SKIP_FADE as f32);
        }
        let Some(s) = story.scenes.get(self.scene) else {
            return ([0.0; 3], 1.0);
        };
        let out_at = s.end_time - s.fade_out.time;
        if s.fade_out.time > 0 && self.t >= out_at {
            let k = (self.t - out_at + 1) as f32 / s.fade_out.time as f32;
            return (s.fade_out.color, k.clamp(0.0, 1.0));
        }
        if s.fade_in.time > 0 && self.t < s.fade_in.time {
            let k = 1.0 - self.t as f32 / s.fade_in.time as f32;
            return (s.fade_in.color, k.clamp(0.0, 1.0));
        }
        ([0.0; 3], 0.0)
    }
}

/// The storyboards of a screenpack (system.def `[Files]`) that exist:
/// logo first, then intro. No file is opened beyond the system.def.
pub fn system_storyboards(system_def: &Path, data_dir: &Path) -> Vec<PathBuf> {
    let Ok(def) = Def::open(system_def) else {
        return Vec::new();
    };
    let Some(files) = def.section("files") else {
        return Vec::new();
    };
    let dir = system_def.parent().unwrap_or(Path::new("."));
    ["logo.storyboard", "intro.storyboard"]
        .iter()
        .filter_map(|k| files.raw(k))
        .filter_map(|n| find(dir, n, &[data_dir.to_path_buf()]))
        .collect()
}

/// A character's arcade storyboard (`[Arcade] intro.storyboard` or
/// `ending.storyboard` in its .def) when the file exists.
pub fn char_storyboard(char_def: &Path, key: &str) -> Option<PathBuf> {
    let def = Def::open(char_def).ok()?;
    let name = def.section("arcade")?.raw(key)?;
    find(char_def.parent()?, name, &[])
}

/// A storyboard on screen: its sprites, fonts, sounds and the clock.
pub struct StoryScreen {
    pub story: Story,
    pub clock: Clock,
    path: PathBuf,
    sheet: Option<SpriteSheet>,
    air: Air,
    fonts: HashMap<i32, FontGpu>,
    snd: Option<Snd>,
    bank: BankId,
    /// One per layer of the current scene.
    anims: Vec<Option<Anim>>,
    bg: Option<StageVisual>,
    /// Folders a music file may be in after the storyboard's own.
    music_dirs: Vec<PathBuf>,
    events: Vec<Event>,
}

impl StoryScreen {
    /// Load the storyboard at `path`; `data_dir` is the engine's `data/`.
    pub fn load(path: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let bytes = rugen_formats::vfs::read(path).with_context(|| path.display().to_string())?;
        let text = rugen_formats::text::decode(&bytes);
        let dir = path.parent().unwrap_or(Path::new("."));
        let root = data_dir.parent().unwrap_or(data_dir).to_path_buf();
        let fallback = vec![data_dir.to_path_buf(), root.clone()];
        let story =
            Story::parse(&text, dir, &fallback).with_context(|| path.display().to_string())?;
        let sheet = match &story.spr {
            Some(p) => Some(SpriteSheet::new(
                Sff::open(p, false).with_context(|| p.display().to_string())?,
            )),
            None => None,
        };
        let mut fonts = HashMap::new();
        for (i, p) in &story.fonts {
            match Font::open(p) {
                Ok(f) => {
                    fonts.insert(*i, FontGpu::new(f));
                }
                Err(e) => log::warn!("storyboard font {}: {e}", p.display()),
            }
        }
        let snd = story.snd.as_ref().and_then(|p| {
            Snd::open(p)
                .map_err(|e| log::warn!("storyboard sounds {}: {e}", p.display()))
                .ok()
        });
        Ok(Self {
            clock: Clock::new(&story),
            story,
            path: path.to_path_buf(),
            sheet,
            air: Air::parse(&text),
            fonts,
            snd,
            bank: rugen_audio::new_bank(),
            anims: Vec::new(),
            bg: None,
            music_dirs: vec![dir.to_path_buf(), root],
            events: Vec::new(),
        })
    }

    pub fn done(&self) -> bool {
        self.clock.done
    }

    /// START / Esc.
    pub fn skip(&mut self) {
        self.clock.skip();
    }

    /// A fight button.
    pub fn next_scene(&mut self) {
        self.clock.next_scene(&self.story);
    }

    /// One tick: scene starts, music, sounds, animations.
    pub fn tick(&mut self, audio: &mut Option<Audio>) {
        let scene_t = self.clock.t;
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        self.clock.step(&self.story, &mut events);
        for e in &events {
            match e {
                Event::Scene(i) => self.start_scene(*i),
                Event::Music(b) => self.music(b, audio),
                Event::Sound(c) => self.sound(*c, audio),
            }
        }
        self.events = events;
        // animations run while their layer is on screen
        if let Some(scene) = self.story.scenes.get(self.clock.scene) {
            for (l, a) in scene.layers.iter().zip(self.anims.iter_mut()) {
                if let Some(a) = a.as_mut()
                    && l.visible(scene_t)
                {
                    a.step(&self.air);
                }
            }
        }
        if let Some(bg) = self.bg.as_mut() {
            bg.step();
        }
    }

    fn start_scene(&mut self, i: usize) {
        let Some(scene) = self.story.scenes.get(i) else {
            return;
        };
        self.anims = scene
            .layers
            .iter()
            .map(|l| l.anim.filter(|a| self.air.get(*a).is_some()).map(Anim::new))
            .collect();
        self.bg = scene.bg.as_ref().and_then(|name| {
            StageVisual::load_prefixed(&self.path, name, self.story.spr.as_deref())
                .map_err(|e| log::warn!("storyboard bg {name}: {e:#}"))
                .ok()
        });
    }

    fn music(&self, b: &Bgm, audio: &mut Option<Audio>) {
        let Some(a) = audio.as_mut() else { return };
        match b {
            Bgm::Stop => a.stop_music(),
            Bgm::Play { file, looping } => {
                match find(&self.music_dirs[0], file, &self.music_dirs[1..]) {
                    Some(p) => a.play_music(Some(&p), 1.0, *looping),
                    None => log::debug!("storyboard music {file} not found"),
                }
            }
        }
    }

    fn sound(&self, c: Cue, audio: &mut Option<Audio>) {
        let (Some(a), Some(snd)) = (audio.as_mut(), self.snd.as_ref()) else {
            return;
        };
        let key = SoundKey {
            bank: self.bank,
            group: c.group,
            number: c.number,
        };
        if !a.is_loaded(key) {
            let Some(bytes) = snd.get(c.group, c.number) else {
                return;
            };
            a.load(key, &bytes);
        }
        a.play(key, 0, -1, c.volume, c.pan as f32 / 127.0, false);
    }

    /// Give back the textures (the storyboard is over).
    pub fn release_gpu(&mut self, r: &mut Renderer) {
        if let Some(s) = self.sheet.as_mut() {
            s.free_gpu(r);
        }
        for f in self.fonts.values_mut() {
            f.free_gpu(r);
        }
        if let Some(b) = self.bg.as_mut() {
            b.free_gpu(r);
        }
    }

    /// Draw the current tick; the camera spans the storyboard's localcoord
    /// (centred, the window's extra width filled by the clear colour).
    pub fn render(&mut self, r: &mut Renderer) -> (Camera, [f64; 4]) {
        let lc = self.story.localcoord;
        let (w, h) = r.size();
        let sw = lc[1] * w as f32 / h.max(1) as f32;
        let cam = Camera {
            screen: [sw, lc[1]],
            center: [0.0, lc[1] / 2.0],
            zoom: (sw / lc[0]).min(1.0),
        };
        let Some(scene) = self.story.scenes.get(self.clock.scene).cloned() else {
            return (cam, [0.0, 0.0, 0.0, 1.0]);
        };
        let clear = scene.clear.unwrap_or([0.0; 3]);
        let clear = [clear[0] as f64, clear[1] as f64, clear[2] as f64, 1.0];
        // storyboard pixels (top-left origin) → world (x centred)
        let at = |p: [f32; 2]| [p[0] - lc[0] / 2.0, p[1]];
        let clip = scene.window.map(|w| {
            let a = at([w[0], w[1]]);
            let b = at([w[2] + 1.0, w[3] + 1.0]);
            [a[0], a[1], b[0], b[1]]
        });
        if let Some(bg) = self.bg.as_mut() {
            bg.draw(r, 0, [0.0, 0.0], 1.0, sw, Default::default());
        }
        let t = self.clock.t;
        for (k, l) in scene.layers.iter().enumerate() {
            if !l.visible(t) {
                continue;
            }
            let pos = at([scene.pos[0] + l.offset[0], scene.pos[1] + l.offset[1]]);
            if let (Some(a), Some(sheet)) = (
                self.anims.get(k).and_then(Option::as_ref),
                self.sheet.as_mut(),
            ) && let Some(f) = a.frame(&self.air)
            {
                draw_frame(r, sheet, f, pos, clip);
            }
            if l.text.is_some()
                && let Some(font) = self.fonts.get_mut(&l.font[0])
            {
                let text = l.text_at(t);
                let step = f32::from(font.font.size[1]) + font.font.spacing[1] as f32;
                for (i, line) in text.split('\n').enumerate() {
                    font.draw_clipped(
                        r,
                        line,
                        [pos[0], pos[1] + i as f32 * step],
                        l.font[1],
                        l.font[2],
                        1.0,
                        clip,
                        1.0,
                    );
                }
            }
        }
        if let Some(bg) = self.bg.as_mut() {
            bg.draw(r, 1, [0.0, 0.0], 1.0, sw, Default::default());
        }
        let (color, alpha) = self.clock.fade(&self.story);
        if alpha > 0.0 {
            r.rect(
                [-4000.0, -4000.0],
                [4000.0, 4000.0],
                [color[0], color[1], color[2], alpha],
            );
        }
        (cam, clear)
    }
}

/// One AIR frame of a layer at `pos`, clipped to the scene window.
fn draw_frame(
    r: &mut Renderer,
    sheet: &mut SpriteSheet,
    f: &Frame,
    pos: [f32; 2],
    clip: Option<[f32; 4]>,
) {
    let Some(p) = sheet.get(r, f.group, f.number) else {
        return;
    };
    let mut d = Draw::new(
        p.tex,
        p.pal,
        [pos[0] + f.x as f32, pos[1] + f.y as f32],
        p.axis,
    );
    d.scale = [f.xscale, f.yscale];
    d.hflip = f.hflip;
    d.vflip = f.vflip;
    d.blend = crate::sprites::air_blend(f);
    d.clip = clip;
    r.draw(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    const KFM_INTRO_LIKE: &str = "\
[Info]\nlocalcoord = 320,240\n\
[SceneDef]\nspr = intro.sff\nsnd = intro.snd\nstartscene = 0\n\
[Scene 0]\nfadein.time = 60\nfadein.col = 0,0,0\nfadeout.time = 15\nfadeout.col = 255,255,255\n\
clearcolor = 255,255,255\nlayerall.pos = 160,0\nlayer0.anim = 0\nlayer0.offset = 0,32\n\
layer1.text = \"Hello\\nthere\" ;two lines\nlayer1.font = 0,1,-1\nlayer1.textdelay = 2\nlayer1.starttime = 10\n\
sound0.value = 0,1\nsound0.starttime = 3\nbgm = intro.mp3\nbgm.loop = 1\nend.time = 100\n\
[Begin Action 0]\n0,0, 0,0, -1\n\
[Scene 1]\nlayer0.anim = 10\nlayer0.endtime = 20\nbgm =\nend.time = 30\n\
[scene end]\nclearcolor = -1,0,0\nwindow = 0,0, 159,119\nbg.name = BG0\nend.time = 10\n";

    fn story() -> Story {
        Story::parse(KFM_INTRO_LIKE, Path::new("/none"), &[]).unwrap()
    }

    #[test]
    fn scenes_layers_and_carry_over() {
        let s = story();
        assert_eq!(s.localcoord, [320.0, 240.0]);
        assert_eq!(s.scenes.len(), 3);
        let a = &s.scenes[0];
        assert_eq!(a.end_time, 100);
        assert_eq!(a.fade_in.time, 60);
        assert_eq!(a.fade_out.color, [1.0, 1.0, 1.0]);
        assert_eq!(a.clear, Some([1.0, 1.0, 1.0]));
        assert_eq!(a.pos, [160.0, 0.0]);
        assert_eq!(a.layers.len(), 2);
        assert_eq!(a.layers[0].anim, Some(0));
        assert_eq!(a.layers[0].offset, [0.0, 32.0]);
        let text = &a.layers[1];
        assert_eq!(text.text.as_deref(), Some("Hello\nthere"));
        assert_eq!(text.font, [0, 1, -1]);
        assert_eq!(text.text_at(10), "H");
        assert_eq!(text.text_at(13), "He");
        assert_eq!(text.text_at(1000), "Hello\nthere");
        assert!(!text.visible(9) && text.visible(10) && text.visible(99));
        assert_eq!(
            a.bgm,
            Some(Bgm::Play {
                file: "intro.mp3".into(),
                looping: true
            })
        );
        assert_eq!(a.sounds[0].start, 3);
        // the next scenes keep clearcolor and layerall.pos, stop the music
        let b = &s.scenes[1];
        assert_eq!(b.clear, Some([1.0, 1.0, 1.0]));
        assert_eq!(b.pos, [160.0, 0.0]);
        assert_eq!(b.bgm, Some(Bgm::Stop));
        assert!(b.layers[0].visible(19) && !b.layers[0].visible(20));
        let c = &s.scenes[2];
        assert_eq!(c.clear, None, "clearcolor -1: no clear");
        assert_eq!(c.window, Some([0.0, 0.0, 159.0, 119.0]));
        assert_eq!(c.bg.as_deref(), Some("BG0"));
        assert_eq!(c.bgm, None, "music goes on");
    }

    /// Scenes last `end.time` ticks each; sounds and music fire on their
    /// tick; the fades follow the scene.
    #[test]
    fn clock_runs_scene_after_scene() {
        let s = story();
        let mut c = Clock::new(&s);
        let mut ev = Vec::new();
        c.step(&s, &mut ev);
        assert_eq!(ev[0], Event::Scene(0));
        assert!(matches!(ev[1], Event::Music(Bgm::Play { .. })));
        assert_eq!(c.fade(&s), ([0.0; 3], 1.0 - 1.0 / 60.0));
        ev.clear();
        for _ in 0..3 {
            c.step(&s, &mut ev);
        }
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert!(matches!(
            ev[0],
            Event::Sound(Cue {
                group: 0,
                number: 1,
                ..
            })
        ));
        while c.scene == 0 {
            if c.t == 99 {
                assert_eq!(c.fade(&s), ([1.0; 3], 1.0), "faded out to white");
            }
            c.step(&s, &mut ev);
        }
        let mut ticks = 100;
        while !c.done {
            c.step(&s, &mut ev);
            ticks += 1;
        }
        assert_eq!(ticks, 100 + 30 + 10);
    }

    /// START / Esc: a short fade to black, then the end. A fight button
    /// jumps to the scene's fade-out.
    #[test]
    fn skipping_ends_or_advances() {
        let s = story();
        let mut c = Clock::new(&s);
        let mut ev = Vec::new();
        c.step(&s, &mut ev);
        c.next_scene(&s);
        assert_eq!(c.t, 85, "on to the fade-out");
        for _ in 0..15 {
            c.step(&s, &mut ev);
        }
        assert_eq!((c.scene, c.t), (1, 0));
        c.skip();
        let mut n = 0;
        while !c.done {
            c.step(&s, &mut ev);
            n += 1;
        }
        assert_eq!(n, SKIP_FADE);
    }

    #[test]
    fn not_a_storyboard() {
        assert!(Story::parse("[Info]\nname = x\n", Path::new("."), &[]).is_err());
        assert!(Story::parse("[SceneDef]\nspr = a.sff\n", Path::new("."), &[]).is_err());
    }

    /// The MUGEN 1.1 samples parse: KFM's intro and ending, the KFM motif's
    /// logo and intro (with its scrolling `bg.name` background).
    #[test]
    fn mugen_samples_parse() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        let files = [
            "chars/kfm/intro.def",
            "chars/kfm/ending.def",
            "data/kfm/logo.def",
            "data/kfm/intro.def",
        ];
        for f in files {
            let p = root.join(f);
            let Ok(bytes) = std::fs::read(&p) else {
                return;
            };
            let s = Story::parse(
                &rugen_formats::text::decode(&bytes),
                p.parent().unwrap(),
                &[root.join("data")],
            )
            .unwrap();
            assert!(s.spr.is_some(), "{f}: sprites found");
            assert!(!s.scenes.is_empty());
        }
        let intro = Story::parse(
            &std::fs::read_to_string(root.join("data/kfm/intro.def")).unwrap(),
            &root.join("data/kfm"),
            &[],
        )
        .unwrap();
        assert_eq!(intro.scenes.len(), 6);
        assert_eq!(intro.scenes[0].bg.as_deref(), Some("BG0"));
        assert_eq!(intro.scenes[5].layers.len(), 3);
        assert_eq!(intro.scenes[5].layers[2].start, 90);
        let kfm = root.join("chars/kfm/kfm.def");
        assert_eq!(
            char_storyboard(&kfm, "intro.storyboard"),
            Some(root.join("chars/kfm/intro.def"))
        );
        assert_eq!(
            system_storyboards(&root.join("data/kfm/system.def"), &root.join("data")),
            vec![
                root.join("data/kfm/logo.def"),
                root.join("data/kfm/intro.def")
            ]
        );
        assert!(
            system_storyboards(&root.join("data/system.def"), &root.join("data")).is_empty(),
            "the default motif has none"
        );
    }
}
