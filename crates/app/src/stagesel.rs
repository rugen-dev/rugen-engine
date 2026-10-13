//! STAGE SELECT (`motif/ui/stages.toml`): fighter select → stage select →
//! versus → fight. Holds the installed stages, the strip under the preview
//! and the live preview of the stage under the cursor.
//!
//! Built for hundreds of stages: names and authors come from the cached
//! manifests, only the strip's visible thumbnails (plus a margin) are laid
//! out, their small `thumb.png` files decode on a worker thread and become
//! textures a few per frame, at most [`THUMB_CAP`] stay alive (least recently
//! shown ones are freed), and only the stage under the cursor is loaded for
//! the preview — once the cursor rests, also off the main thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use rugen_content::{Entry, Library, Manifest};
use rugen_render::{Renderer, TextureId};
use rugen_sim::Buttons;
use rugen_ui::Rect;
use serde_json::{Value, json};

use crate::game::{Mode, Transition};
use crate::play::FightSetup;
use crate::stage::{StageVisual, View};
use crate::versus::Portrait;

/// Strip slot pitch (thumbnail width + gap) in virtual pixels; the TOML's
/// strip tiles are `STRIP_TILE` wide.
pub const STRIP_TILE: f32 = 184.0;
const STRIP_GAP: f32 = 12.0;
const STRIP_PITCH: f32 = STRIP_TILE + STRIP_GAP;
/// Thumbnail textures kept alive.
pub const THUMB_CAP: usize = 64;
/// Thumbnails turned into textures per frame.
const THUMB_UPLOADS: usize = 4;
/// Thumbnails waiting on the decoder at most.
const THUMB_QUEUE: usize = 12;
/// Ticks the cursor rests before the preview loads (fast scrolling loads nothing).
const PREVIEW_SETTLE: u32 = 8;
/// Held ←/→: first repeat after this many ticks, then every `REPEAT_SLOW`
/// ticks, every `REPEAT_FAST` once held for `REPEAT_RAMP` ticks.
const REPEAT_DELAY: u32 = 16;
const REPEAT_SLOW: u32 = 5;
const REPEAT_FAST: u32 = 2;
const REPEAT_RAMP: u32 = 30;

/// One installed stage. Everything but the arena flag comes from the manifest.
pub struct StageItem {
    pub entry: Entry,
    pub dir: Option<PathBuf>,
    pub manifest: Option<Manifest>,
    /// Shipped with rugen (`motif/stages`).
    pub bundled: bool,
}

impl StageItem {
    pub fn def(&self) -> Option<PathBuf> {
        Some(self.manifest.as_ref()?.def_path(self.dir.as_ref()?))
    }

    pub fn name(&self) -> String {
        self.manifest
            .as_ref()
            .map(|m| m.display_name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.entry.id.clone())
            .to_uppercase()
    }

    fn author(&self) -> String {
        self.manifest
            .as_ref()
            .map(|m| m.author.trim().to_uppercase())
            .unwrap_or_default()
    }

    pub fn thumb(&self) -> Option<PathBuf> {
        let t = self.manifest.as_ref()?.thumb.as_ref()?;
        Some(self.dir.as_ref()?.join(&t.file))
    }

    /// The stage def has an `[Arena]` section (blast zones, stocks, ring-out).
    pub fn is_arena(&self) -> bool {
        self.def().is_some_and(|d| def_is_arena(&d))
    }
}

fn def_is_arena(def: &Path) -> bool {
    rugen_formats::vfs::read(def).is_ok_and(|b| {
        String::from_utf8_lossy(&b)
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("[arena]"))
    })
}

/// Installed stages (prepared from the cache; nothing is decoded), sorted by
/// name. ARENA keeps only stages with an `[Arena]` section.
pub fn load_stages(lib: &Library, mode: Mode) -> Vec<StageItem> {
    let bundled_root = lib.assets.join("motif").join("stages");
    let mut stages: Vec<StageItem> = lib
        .stages()
        .into_iter()
        .map(|entry| {
            let prepared = lib
                .prepare_stage(&entry)
                .map_err(|e| log::warn!("stage {}: {e}", entry.id))
                .ok();
            StageItem {
                bundled: entry.source.starts_with(&bundled_root),
                dir: prepared.as_ref().map(|(d, _)| d.clone()),
                manifest: prepared.map(|(_, m)| m),
                entry,
            }
        })
        .collect();
    if mode == Mode::Arena {
        stages.retain(StageItem::is_arena);
        if stages.is_empty() {
            log::warn!("arena: no stage with an [Arena] section installed");
        }
    }
    stages.sort_by_cached_key(|s| (s.name(), s.entry.id.to_ascii_lowercase()));
    stages
}

/// What the fighter select hands to the stage select.
#[derive(Debug)]
pub struct StagePlan {
    pub mode: Mode,
    /// The fight to start; its stage is filled in here.
    pub setup: FightSetup,
    pub portraits: [Portrait; 2],
    pub names: [String; 2],
    /// Both picks (manifest ids), to go back to the fighter select with them locked.
    pub picks: [String; 2],
}

/// The stage under the cursor, loaded for the preview.
struct Preview {
    item: usize,
    visual: StageVisual,
    /// Stage size, camera start and floor in stage units.
    size: [f32; 2],
    start_x: f32,
    info: Value,
}

type Loaded = (
    u64,
    usize,
    anyhow::Result<(StageVisual, [f32; 2], f32, Value)>,
);

/// A rendered strip tile (virtualised: only these exist in the data).
struct Tile {
    index: usize,
    x: f32,
}

pub struct StageSel {
    plan: Option<StagePlan>,
    items: Vec<StageItem>,
    /// 0 = RANDOM, else `items[sel - 1]`.
    pub sel: usize,
    /// Strip offset in slots (eased toward the selection).
    scroll: f32,
    strip_w: f32,
    /// Held direction (-1 / 1) and how long it has been held.
    rep_dir: i32,
    rep_ticks: u32,
    // thumbnails: worker in/out, decoded waiting for upload, alive textures
    thumb_tx: Option<Sender<(usize, PathBuf)>>,
    thumb_rx: Receiver<(usize, Option<crate::uiscreen::MemImage>)>,
    thumb_pending: HashSet<usize>,
    thumb_failed: HashSet<usize>,
    thumb_decoded: VecDeque<(usize, crate::uiscreen::MemImage)>,
    thumbs: HashMap<usize, TextureId>,
    /// Least recently shown first.
    thumb_lru: VecDeque<usize>,
    // preview
    preview: Option<Preview>,
    preview_gen: u64,
    preview_tx: Sender<Loaded>,
    preview_rx: Receiver<Loaded>,
    loading: Option<usize>,
    settle: u32,
    seed: u32,
    ticks: u64,
}

/// Small stage facts for the info panel (the preview loader reads them).
fn stage_info(def: &Path, stage: &rugen_sim::Stage, item_arena: bool, bundled: bool) -> Value {
    let text = rugen_formats::vfs::read(def)
        .map(|b| rugen_formats::text::decode(&b))
        .unwrap_or_default();
    let mut music = String::new();
    let mut section = String::new();
    for line in text.lines() {
        let l = line.split(';').next().unwrap_or("").trim();
        if let Some(s) = l.strip_prefix('[') {
            section = s.trim_end_matches(']').trim().to_ascii_lowercase();
            continue;
        }
        if section == "music"
            && let Some((k, v)) = l.split_once('=')
            && k.trim().eq_ignore_ascii_case("bgmusic")
        {
            let v = v.trim().trim_matches('"');
            if !v.is_empty() {
                music = Path::new(v)
                    .file_stem()
                    .map(|s| s.to_string_lossy().replace(['_', '-'], " ").to_uppercase())
                    .unwrap_or_default();
            }
        }
    }
    let mut tags = vec![if item_arena { "ARENA RULES" } else { "CLASSIC" }];
    if bundled {
        tags.push("BUNDLED");
    }
    // the camera travels between the bounds over one screen of stage
    let width = libm::roundf(stage.bound_right - stage.bound_left + stage.width);
    json!({
        "music": music,
        "has_music": !music.is_empty(),
        "width": if width > 0.0 { format!("{width} PX") } else { String::new() },
        "has_width": width > 0.0,
        "tags": tags.iter().map(|t| json!({ "t": t })).collect::<Vec<_>>(),
    })
}

impl StageSel {
    pub fn new(plan: StagePlan, lib: &Library) -> Self {
        let t = std::time::Instant::now();
        let items = load_stages(lib, plan.mode);
        log::info!(
            "stage select: {} stages listed in {:?}",
            items.len(),
            t.elapsed()
        );
        let (thumb_tx, worker_rx) = channel::<(usize, PathBuf)>();
        let (worker_tx, thumb_rx) = channel();
        // thumbnails decode off the main thread; the worker ends with the screen
        std::thread::Builder::new()
            .name("stage-thumbs".into())
            .spawn(move || {
                while let Ok((i, path)) = worker_rx.recv() {
                    let img = crate::uiscreen::load_image(&path).ok();
                    if worker_tx.send((i, img)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        let (preview_tx, preview_rx) = channel();
        let mut s = Self {
            plan: Some(plan),
            items,
            sel: 0,
            scroll: 0.0,
            strip_w: 1184.0,
            rep_dir: 0,
            rep_ticks: 0,
            thumb_tx: Some(thumb_tx),
            thumb_rx,
            thumb_pending: HashSet::new(),
            thumb_failed: HashSet::new(),
            thumb_decoded: VecDeque::new(),
            thumbs: HashMap::new(),
            thumb_lru: VecDeque::new(),
            preview: None,
            preview_gen: 0,
            preview_tx,
            preview_rx,
            loading: None,
            settle: 0,
            seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x2545_f491, |d| d.subsec_nanos() | 1),
            ticks: 0,
        };
        // a lone arena stage is shown by name rather than as RANDOM
        if s.items.len() == 1 && s.plan.as_ref().is_some_and(|p| p.mode == Mode::Arena) {
            s.sel = 1;
        }
        s.scroll = s.target_scroll();
        s
    }

    pub fn count(&self) -> usize {
        self.items.len() + 1
    }

    fn visible(&self) -> f32 {
        (self.strip_w + STRIP_GAP) / STRIP_PITCH
    }

    /// Strip offset that keeps the selection near the centre, clamped so the
    /// ends never show empty space.
    fn target_scroll(&self) -> f32 {
        let max = (self.count() as f32 - self.visible()).max(0.0);
        (self.sel as f32 + 0.5 - self.visible() / 2.0).clamp(0.0, max)
    }

    pub fn set_strip_width(&mut self, w: f32) {
        if w > 0.0 {
            self.strip_w = w;
        }
    }

    fn select(&mut self, i: usize) -> bool {
        let i = i.min(self.count() - 1);
        if i == self.sel {
            return false;
        }
        self.sel = i;
        self.settle = 0;
        true
    }

    /// One visible strip-width of stages.
    fn page(&self) -> usize {
        (libm::floorf(self.visible()) as usize).max(1)
    }

    /// Move by `d` slots (clamped at the ends).
    fn step(&mut self, d: i64) -> bool {
        let last = self.count() as i64 - 1;
        self.select((self.sel as i64 + d).clamp(0, last) as usize)
    }

    /// Page / Home / End / letter keys. True when the selection moved.
    pub fn key(&mut self, key: &winit::keyboard::Key) -> Option<bool> {
        use winit::keyboard::{Key, NamedKey};
        let page = self.page() as i64;
        Some(match key {
            Key::Named(NamedKey::PageUp) => self.step(-page),
            Key::Named(NamedKey::PageDown) => self.step(page),
            Key::Named(NamedKey::Home) => self.select(0),
            Key::Named(NamedKey::End) => self.select(self.count() - 1),
            Key::Character(c) => {
                let ch = c.chars().next()?.to_ascii_uppercase();
                if !ch.is_ascii_alphanumeric() {
                    return None;
                }
                // the next stage (after the cursor, wrapping) whose name starts with it
                let n = self.items.len();
                let from = self.sel; // items[sel] is the one after the cursor
                let hit = (0..n)
                    .map(|k| (from + k) % n)
                    .find(|&i| self.items[i].name().starts_with(ch))?;
                self.select(hit + 1)
            }
            _ => return None,
        })
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        if n == 0 { 0 } else { (self.seed as usize) % n }
    }

    /// Back to the fighter select with both picks still locked.
    pub fn back(&mut self) -> Transition {
        match self.plan.take() {
            Some(p) => Transition::SelectAgain(p.mode, p.picks.map(Some)),
            None => Transition::Title,
        }
    }

    /// ENTER: the versus screen with the chosen (or a random) stage.
    fn confirm(&mut self, lib_default: impl FnOnce() -> Option<PathBuf>) -> Option<Transition> {
        let i = if self.sel == 0 {
            let n = self.items.len();
            (n > 0).then(|| self.rand(n))
        } else {
            Some(self.sel - 1)
        };
        let stage = i
            .and_then(|i| self.items.get(i)?.def())
            .or_else(lib_default);
        let mut plan = self.plan.take()?;
        plan.setup.stage = stage;
        Some(Transition::Versus(plan.setup, plan.portraits))
    }

    /// One tick of input: `pressed` / `held` are player 1's buttons.
    pub fn tick(
        &mut self,
        pressed: Buttons,
        held: Buttons,
        lib: &Library,
        sfx: &mut dyn FnMut(Sfx),
    ) -> Option<Transition> {
        self.ticks += 1;
        let mut moved = false;
        // ←/→ with auto-repeat that speeds up the longer it is held
        let dir = i32::from(held.r) - i32::from(held.l);
        if dir != 0 && dir == self.rep_dir {
            self.rep_ticks += 1;
            let t = self.rep_ticks;
            let every = if t >= REPEAT_RAMP {
                REPEAT_FAST
            } else {
                REPEAT_SLOW
            };
            if t >= REPEAT_DELAY && (t - REPEAT_DELAY) % every == 0 {
                moved |= self.step(dir as i64);
            }
        } else {
            self.rep_dir = dir;
            self.rep_ticks = 0;
        }
        if pressed.l {
            moved |= self.step(-1);
        }
        if pressed.r {
            moved |= self.step(1);
        }
        // X / Z (pad West / R1): a page back / forward
        let page = self.page() as i64;
        if pressed.x {
            moved |= self.step(-page);
        }
        if pressed.z {
            moved |= self.step(page);
        }
        if moved {
            sfx(Sfx::Move);
        }
        if pressed.b {
            sfx(Sfx::Cancel);
            return Some(self.back());
        }
        if pressed.a || pressed.c || pressed.y || pressed.s {
            sfx(Sfx::Done);
            return self.confirm(|| lib.default_stage());
        }
        None
    }

    /// Ease the strip toward the selection (~120 ms) and pick the tiles to show.
    fn advance_scroll(&mut self) {
        let target = self.target_scroll();
        let d = target - self.scroll;
        // 1 - e^(-dt/τ) with τ = 40 ms at 60 Hz: ~95 % of the way in 120 ms
        self.scroll = if d.abs() < 0.002 {
            target
        } else {
            self.scroll + d * 0.34
        };
        // never more than a page behind: a fast flick stays readable
        let lag = self.visible();
        self.scroll = self.scroll.clamp(target - lag, target + lag);
    }

    fn tiles(&self) -> Vec<Tile> {
        let first = (libm::floorf(self.scroll) as i64 - 1).max(0) as usize;
        let last = (libm::ceilf(self.scroll + self.visible()) as usize + 1).min(self.count());
        (first..last)
            .map(|index| Tile {
                index,
                x: (index as f32 - self.scroll) * STRIP_PITCH,
            })
            .collect()
    }

    /// Per-frame upkeep with the renderer: thumbnails in and out of the GPU,
    /// the preview loaded once the cursor rests. Registers thumbnail textures
    /// in `textures` under `stagethumb:<i>`.
    pub fn upkeep(
        &mut self,
        r: &mut Renderer,
        textures: &mut HashMap<String, crate::uiscreen::Cached>,
    ) {
        self.advance_scroll();
        let wanted: Vec<usize> = self
            .tiles()
            .iter()
            .filter(|t| t.index > 0)
            .map(|t| t.index - 1)
            .collect();
        // decoded thumbnails
        while let Ok((i, img)) = self.thumb_rx.try_recv() {
            self.thumb_pending.remove(&i);
            match img {
                Some(img) => self.thumb_decoded.push_back((i, img)),
                None => {
                    self.thumb_failed.insert(i);
                }
            }
        }
        // upload a few (the ones still wanted), the rest wait for later frames
        let mut uploads = 0;
        while uploads < THUMB_UPLOADS {
            let Some((i, (w, h, rgba))) = self.thumb_decoded.pop_front() else {
                break;
            };
            if !wanted.contains(&i) || self.thumbs.contains_key(&i) {
                continue;
            }
            let tex = r.add_rgba(w, h, &rgba);
            self.thumbs.insert(i, tex);
            textures.insert(thumb_key(i), Some((tex, w, h, None)));
            uploads += 1;
        }
        // shown thumbnails are the most recently used
        for &i in &wanted {
            if self.thumbs.contains_key(&i) {
                self.thumb_lru.retain(|&k| k != i);
                self.thumb_lru.push_back(i);
            }
        }
        while self.thumbs.len() > THUMB_CAP {
            let Some(old) = self.thumb_lru.pop_front() else {
                break;
            };
            if let Some(tex) = self.thumbs.remove(&old) {
                textures.remove(&thumb_key(old));
                r.free_texture(tex);
            }
        }
        // ask for the missing visible ones, nearest the cursor first
        if let Some(tx) = &self.thumb_tx {
            let mut missing: Vec<usize> = wanted
                .iter()
                .copied()
                .filter(|i| {
                    !self.thumbs.contains_key(i)
                        && !self.thumb_pending.contains(i)
                        && !self.thumb_failed.contains(i)
                        && !self.thumb_decoded.iter().any(|(k, _)| k == i)
                })
                .collect();
            missing.sort_by_key(|&i| (i as i64 + 1 - self.sel as i64).abs());
            for i in missing {
                if self.thumb_pending.len() >= THUMB_QUEUE {
                    break;
                }
                let sent = self.items[i]
                    .thumb()
                    .is_some_and(|p| tx.send((i, p)).is_ok());
                if sent {
                    self.thumb_pending.insert(i);
                } else {
                    self.thumb_failed.insert(i);
                }
            }
        }
        self.upkeep_preview(r);
    }

    fn upkeep_preview(&mut self, r: &mut Renderer) {
        while let Ok((generation, item, res)) = self.preview_rx.try_recv() {
            if generation != self.preview_gen {
                // a stale load: nothing was uploaded yet, just drop it
                continue;
            }
            self.loading = None;
            match res {
                Ok((visual, size, start_x, info)) => {
                    if let Some(mut old) = self.preview.take() {
                        old.visual.free_gpu(r);
                    }
                    self.preview = Some(Preview {
                        item,
                        visual,
                        size,
                        start_x,
                        info,
                    });
                }
                Err(e) => log::warn!("stage preview: {e:#}"),
            }
        }
        if let Some(p) = self.preview.as_mut() {
            p.visual.step();
        }
        let want = self.sel.checked_sub(1);
        let shown = self.preview.as_ref().map(|p| p.item);
        if want.is_none() || want == shown || want == self.loading {
            if want.is_none()
                && let Some(mut old) = self.preview.take()
            {
                old.visual.free_gpu(r);
            }
            return;
        }
        self.settle += 1;
        if self.settle < PREVIEW_SETTLE {
            return;
        }
        let Some(i) = want else { return };
        let Some(def) = self.items[i].def() else {
            return;
        };
        let bundled = self.items[i].bundled;
        self.preview_gen += 1;
        self.loading = Some(i);
        let (generation, tx) = (self.preview_gen, self.preview_tx.clone());
        std::thread::Builder::new()
            .name("stage-preview".into())
            .spawn(move || {
                let res = (|| {
                    let visual = StageVisual::load(&def)?;
                    let stage = rugen_sim::Stage::from_def(&rugen_formats::def::Def::open(&def)?);
                    let info = stage_info(&def, &stage, def_is_arena(&def), bundled);
                    Ok((visual, [stage.width, stage.height], stage.start_x, info))
                })();
                let _ = tx.send((generation, i, res));
            })
            .ok();
    }

    /// Free every texture this screen made (leaving it).
    pub fn release(
        &mut self,
        r: &mut Renderer,
        textures: &mut HashMap<String, crate::uiscreen::Cached>,
    ) {
        for (i, tex) in self.thumbs.drain() {
            textures.remove(&thumb_key(i));
            r.free_texture(tex);
        }
        self.thumb_lru.clear();
        if let Some(mut p) = self.preview.take() {
            p.visual.free_gpu(r);
        }
        self.thumb_tx = None;
    }

    /// The live preview: the stage at its starting camera, fitted to `dst`'s height.
    pub fn draw_preview(&mut self, r: &mut Renderer, dst: Rect) {
        let Some(p) = self.preview.as_mut() else {
            return;
        };
        if p.size[1] <= 0.0 || dst.h <= 0.0 {
            return;
        }
        let k = dst.h / p.size[1];
        let screen_w = dst.w / k;
        let cam = [p.start_x, 0.0];
        let view = View {
            src: [cam[0] - screen_w / 2.0, 0.0],
            k,
            dst: [dst.x, dst.y, dst.w, dst.h],
        };
        let scale = p.size[0] / p.visual.localcoord[0];
        r.rect(
            [dst.x, dst.y],
            [dst.x + dst.w, dst.y + dst.h],
            [
                p.visual.clear_color[0] as f32,
                p.visual.clear_color[1] as f32,
                p.visual.clear_color[2] as f32,
                1.0,
            ],
        );
        p.visual.draw_view(r, 0, cam, scale, view);
        p.visual.draw_view(r, 1, cam, scale, view);
    }

    /// The data context `stages.toml` binds to.
    pub fn data(&self) -> Value {
        let plan = self.plan.as_ref();
        let mode = plan.map_or(Mode::Versus, |p| p.mode);
        let mode_name = match mode {
            Mode::Arcade => "ARCADE",
            Mode::Versus => "VS MODE",
            Mode::Training => "TRAINING",
            Mode::Tutorial => "TUTORIAL",
            Mode::Watch => "WATCH",
            Mode::Arena => "ARENA",
            Mode::Online => "ONLINE",
            Mode::Netplay | Mode::Options | Mode::Exit => "",
        };
        let names = plan.map(|p| p.names.clone()).unwrap_or_default();
        let n = self.items.len();
        let cur = match self.sel.checked_sub(1).and_then(|i| self.items.get(i)) {
            None => json!({
                "name": "RANDOM",
                "author": "ANY INSTALLED STAGE",
                "has_author": true,
                "tags": [{ "t": "SURPRISE" }],
                "music": "", "has_music": false, "width": "", "has_width": false,
                "random": true,
                "note": "A STAGE IS DRAWN AT RANDOM WHEN THE FIGHT STARTS.",
            }),
            Some(s) => {
                let author = s.author();
                let mut v = json!({
                    "name": s.name(),
                    "author": author,
                    "has_author": !author.is_empty(),
                    "tags": [],
                    "music": "", "has_music": false, "width": "", "has_width": false,
                    "random": false,
                    "note": "",
                });
                // the facts read with the preview, once it is the one shown
                if let Some(p) = self
                    .preview
                    .as_ref()
                    .filter(|p| Some(p.item) == self.sel.checked_sub(1))
                    && let (Value::Object(m), Value::Object(info)) = (&mut v, &p.info)
                {
                    for (k, val) in info {
                        m.insert(k.clone(), val.clone());
                    }
                } else if s.bundled {
                    v["tags"] = json!([{ "t": "BUNDLED" }]);
                }
                v
            }
        };
        let preview_ready = self
            .preview
            .as_ref()
            .is_some_and(|p| Some(p.item) == self.sel.checked_sub(1));
        let strip: Vec<Value> = self
            .tiles()
            .iter()
            .map(|t| {
                let (name, thumb) = match t.index.checked_sub(1) {
                    None => ("RANDOM".to_string(), String::new()),
                    Some(i) => (
                        self.items[i].name(),
                        if self.thumbs.contains_key(&i) {
                            thumb_key(i)
                        } else {
                            String::new()
                        },
                    ),
                };
                json!({
                    "x": libm::roundf(t.x),
                    "name": name,
                    "thumb": thumb,
                    "has_thumb": !thumb.is_empty(),
                    "placeholder": t.index > 0 && thumb.is_empty(),
                    "random": t.index == 0,
                    "sel": t.index == self.sel,
                    "idle": t.index != self.sel,
                })
            })
            .collect();
        // progress track under the strip
        let count = self.count() as f32;
        let vis = self.visible().min(count);
        let bar_w = (self.strip_w * vis / count).max(24.0).min(self.strip_w);
        let span = (count - vis).max(0.0);
        let bar_x = if span > 0.0 {
            (self.scroll / span).clamp(0.0, 1.0) * (self.strip_w - bar_w)
        } else {
            0.0
        };
        json!({
            "title": "STAGE",
            "sub": format!("P1 PICKS · {mode_name}"),
            "p1": names[0].to_uppercase(),
            "p2": names[1].to_uppercase(),
            "cur": cur,
            "preview_ready": preview_ready,
            "preview_wait": self.sel > 0 && !preview_ready,
            "strip": strip,
            "pos": if self.sel == 0 { "RANDOM".to_string() } else { format!("STAGE {} / {n}", self.sel) },
            "track": { "x": libm::roundf(bar_x), "w": libm::roundf(bar_w), "scrolls": span > 0.0 },
        })
    }
}

/// Texture key of stage `i`'s thumbnail.
fn thumb_key(i: usize) -> String {
    format!("stagethumb:{i}")
}

/// System sounds the stage select asks for.
pub enum Sfx {
    Move,
    Done,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{Key, NamedKey};

    /// A long list: clamped strip, pages, Home / End, letter jumps, only the
    /// visible tiles in the data, back keeps the picks.
    #[test]
    fn strip_handles_a_long_list() {
        let reference =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        if !reference.join("stages/stage0.def").exists() {
            return;
        }
        let root = std::env::temp_dir().join(format!("rugen-stagesel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (i, word) in ["apple", "birch", "cedar", "dune", "elm"]
            .iter()
            .cycle()
            .take(40)
            .enumerate()
        {
            let dir = root.join("stages").join(format!("{word}_{i:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            let def = std::fs::read_to_string(reference.join("stages/stage0.def")).unwrap();
            let def = def.replacen(
                "[Info]",
                &format!("[Info]\ndisplayname = \"{word} {i:02}\""),
                1,
            );
            std::fs::write(dir.join("stage0.def"), def).unwrap();
            std::fs::copy(reference.join("stages/stage0.sff"), dir.join("stage0.sff")).unwrap();
        }
        let lib = Library::discover(Some(&root));
        let setup = FightSetup {
            p1: PathBuf::new(),
            p2: PathBuf::new(),
            stage: None,
            ai: [0, 0],
            net: None,
            tutorial: false,
            ids: ["a".into(), "b".into()],
            mode: Some(Mode::Versus),
            fighter_size: None,
        };
        let plan = StagePlan {
            mode: Mode::Versus,
            setup,
            portraits: [
                Portrait::new("A".into(), None),
                Portrait::new("B".into(), None),
            ],
            names: ["A".into(), "B".into()],
            picks: ["a".into(), "b".into()],
        };
        let mut s = StageSel::new(plan, &lib);
        assert!(s.count() >= 41, "RANDOM + 40 stages");
        assert_eq!(s.sel, 0);
        assert_eq!(s.target_scroll(), 0.0, "clamped at the start");
        // only the tiles on screen (plus a margin) are in the data
        let strip = s.data()["strip"].as_array().unwrap().len();
        assert!(strip <= s.visible() as usize + 3, "{strip} tiles");
        assert_eq!(s.key(&Key::Named(NamedKey::End)), Some(true));
        assert_eq!(s.sel, s.count() - 1);
        let max = s.count() as f32 - s.visible();
        assert!((s.target_scroll() - max).abs() < 1e-3, "clamped at the end");
        assert_eq!(s.key(&Key::Named(NamedKey::Home)), Some(true));
        assert_eq!(s.key(&Key::Named(NamedKey::PageDown)), Some(true));
        assert_eq!(s.sel, s.page());
        // a letter jumps to the next stage whose name starts with it
        assert_eq!(s.key(&Key::Character("d".into())), Some(true));
        assert!(s.data()["cur"]["name"].as_str().unwrap().starts_with('D'));
        assert_eq!(
            s.data()["pos"],
            format!("STAGE {} / {}", s.sel, s.count() - 1)
        );
        match s.back() {
            Transition::SelectAgain(Mode::Versus, picks) => {
                assert_eq!(picks, [Some("a".to_string()), Some("b".to_string())]);
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
