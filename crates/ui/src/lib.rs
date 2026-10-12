//! Data-driven menus. A screen is a TOML file describing a tree of widgets;
//! the host supplies a JSON-like data context (`profile`, `news`, `menu`...)
//! and a [`Painter`] that draws images, rectangles and text. Layout happens in
//! a virtual 1280x720 space; the host maps it to the window.
//!
//! ```toml
//! [[node]]
//! kind = "vbox"
//! x = 60
//! y = 180
//! w = 360
//! gap = 8
//! items = "menu"            # one copy of the children per element of `menu`
//! [[node.children]]
//! kind = "button"
//! h = 52
//! image = "ui/button.webp"
//! image_focus = "ui/button_lit.webp"
//! text = "{item.label}"
//! on_press = "{item.action}"
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

pub const VIRTUAL_W: f32 = 1280.0;
pub const VIRTUAL_H: f32 = 720.0;

/// A size or position: a number of virtual pixels or a `"50%"` of the parent.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Dim {
    Px(f32),
    Pct(String),
}

impl Dim {
    fn resolve(&self, parent: f32) -> f32 {
        match self {
            Dim::Px(v) => *v,
            Dim::Pct(s) => s
                .trim()
                .trim_end_matches('%')
                .parse::<f32>()
                .map_or(0.0, |p| p / 100.0 * parent),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Panel,
    Image,
    Text,
    Button,
    Vbox,
    Hbox,
    /// Vertical scrolling list of children (use with `items`).
    List,
    /// Scrolling grid of fixed-size cells (use with `items`, `columns`, `cell_w`, `cell_h`).
    Grid,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct NodeDef {
    pub id: Option<String>,
    pub kind: Kind,
    pub x: Option<Dim>,
    pub y: Option<Dim>,
    pub w: Option<Dim>,
    pub h: Option<Dim>,
    /// `tl` (default), `tr`, `bl`, `br`, `center`, `top`, `bottom`, `left`, `right`.
    pub anchor: Option<String>,
    /// Solid fill `[r, g, b, a]` 0..1.
    pub color: Option<[f32; 4]>,
    pub color_focus: Option<[f32; 4]>,
    /// Asset key: `ui/x.webp`, `ui/x.png` or `sff:group,number`.
    pub image: Option<String>,
    pub image_focus: Option<String>,
    /// 9-slice borders `[left, top, right, bottom]` in texels.
    pub nine: Option<[u32; 4]>,
    /// Horizontal shear in virtual pixels (slanted panels).
    pub skew: Option<f32>,
    pub text: Option<String>,
    pub font: Option<i32>,
    pub bank: Option<i32>,
    pub bank_focus: Option<i32>,
    /// `left`, `center`, `right`.
    pub align: Option<String>,
    pub scale: Option<f32>,
    pub on_press: Option<String>,
    pub on_left: Option<String>,
    pub on_right: Option<String>,
    /// Buttons are focusable by default; set `focus = false` to disable.
    pub focus: Option<bool>,
    /// Bind path; the node is skipped when the value is false/empty/0.
    pub visible: Option<String>,
    /// Bind path to an array: children are instanced once per element (`item`, `index`).
    pub items: Option<String>,
    pub gap: Option<f32>,
    pub pad: Option<f32>,
    pub columns: Option<u32>,
    pub cell_w: Option<f32>,
    pub cell_h: Option<f32>,
    /// `fade`, `left`, `right`, `up`, `down`: how the node enters the screen.
    pub enter: Option<String>,
    pub enter_time: Option<f32>,
    pub enter_delay: Option<f32>,
    pub hover_scale: Option<f32>,
    pub alpha: Option<f32>,
    pub children: Vec<NodeDef>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScreenDef {
    /// Clear colour.
    pub background: Option<[f32; 4]>,
    pub node: Vec<NodeDef>,
}

impl ScreenDef {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

/// Resolved rectangle in virtual pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        Rect {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0.0),
            h: (y1 - y0).max(0.0),
        }
    }
}

/// What the host draws for the engine. Coordinates are virtual pixels.
pub trait Painter {
    fn image_size(&mut self, key: &str) -> Option<(u32, u32)>;
    /// Draw `src` texels (None = whole image) of `key` into `dst`, sheared by `skew`.
    fn draw_image(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        skew: f32,
        alpha: f32,
        clip: Option<Rect>,
    );
    fn draw_rect(&mut self, dst: Rect, color: [f32; 4], skew: f32, clip: Option<Rect>);
    fn text_width(&mut self, font: i32, text: &str, scale: f32) -> f32;
    /// Height of one text line for the font.
    fn text_height(&mut self, font: i32, scale: f32) -> f32;
    /// `align`: -1 right, 0 centre, 1 left; `(x, y)` is the text's bottom line / anchor.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        font: i32,
        text: &str,
        x: f32,
        y: f32,
        bank: i32,
        align: i32,
        scale: f32,
        alpha: f32,
        clip: Option<Rect>,
    );
}

/// Navigation input for one tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub accept: bool,
    pub back: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A node fired its `on_press` / `on_left` / `on_right` action.
    Action(String),
    /// Focus moved (host may play a sound).
    Moved,
    Back,
}

/// One laid-out widget instance for this frame.
#[derive(Clone, Debug)]
struct Instance {
    def: usize,
    /// Stable key: id or def index, prefixed by the item path.
    key: String,
    rect: Rect,
    clip: Option<Rect>,
    text: Option<String>,
    on_press: Option<String>,
    on_left: Option<String>,
    on_right: Option<String>,
    focusable: bool,
    /// Key of the enclosing list/grid, for wheel scrolling and keep-visible.
    scroll_owner: Option<String>,
    /// Context the instance was built with (for `{bind}` images at draw time).
    ctx: Value,
}

pub struct Ui {
    def: ScreenDef,
    /// Flattened defs (preorder) for stable indices.
    defs: Vec<NodeDef>,
    data: Value,
    instances: Vec<Instance>,
    focus: Option<String>,
    scroll: HashMap<String, f32>,
    time: f32,
    mouse: Option<(f32, f32)>,
    mouse_down: bool,
    mouse_was_down: bool,
    wheel: f32,
    pending: Vec<Event>,
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// `profile.name`, `news.0.title`, `item.label`; a leading `!` negates (for `visible`).
pub fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = root;
    for part in path.split('.') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn visible(ctx: &Value, expr: &str) -> bool {
    let e = expr.trim().trim_matches(['{', '}']);
    match e.strip_prefix('!') {
        Some(rest) => !truthy(lookup(ctx, rest)),
        None => truthy(lookup(ctx, e)),
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Replace `{path}` with context values.
pub fn interpolate(s: &str, ctx: &Value) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                let path = &after[..j];
                out.push_str(&lookup(ctx, path).map(value_text).unwrap_or_default());
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

fn count(n: &NodeDef) -> usize {
    n.children.iter().map(|c| 1 + count(c)).sum()
}

impl Ui {
    pub fn new(def: ScreenDef, data: Value) -> Self {
        let mut defs = Vec::new();
        fn flatten(nodes: &[NodeDef], out: &mut Vec<NodeDef>) {
            for n in nodes {
                let mut copy = n.clone();
                let children = std::mem::take(&mut copy.children);
                out.push(copy);
                flatten(&children, out);
            }
        }
        flatten(&def.node, &mut defs);
        Self {
            def,
            defs,
            data,
            instances: Vec::new(),
            focus: None,
            scroll: HashMap::new(),
            time: 0.0,
            mouse: None,
            mouse_down: false,
            mouse_was_down: false,
            wheel: 0.0,
            pending: Vec::new(),
        }
    }

    pub fn background(&self) -> [f32; 4] {
        self.def.background.unwrap_or([0.0, 0.0, 0.0, 1.0])
    }

    pub fn set_data(&mut self, data: Value) {
        self.data = data;
    }
    pub fn data(&self) -> &Value {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut Value {
        &mut self.data
    }

    /// Mouse position in virtual pixels (None when outside the window).
    pub fn mouse_move(&mut self, pos: Option<(f32, f32)>) {
        self.mouse = pos;
    }
    pub fn mouse_button(&mut self, down: bool) {
        self.mouse_down = down;
    }
    pub fn mouse_wheel(&mut self, lines: f32) {
        self.wheel += lines;
    }

    /// Focus the node with this key (ids of instanced children are `<list>[i].<id>`).
    pub fn set_focus(&mut self, key: Option<&str>) {
        self.focus = key.map(str::to_string);
    }
    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// Restart enter animations.
    pub fn reset_time(&mut self) {
        self.time = 0.0;
    }

    /// Queue an event from the host (e.g. a key-capture result).
    pub fn push_event(&mut self, e: Event) {
        self.pending.push(e);
    }

    // ---------------------------------------------------------------- layout

    fn layout(&mut self, p: &mut dyn Painter) {
        self.instances.clear();
        let root = Rect {
            x: 0.0,
            y: 0.0,
            w: VIRTUAL_W,
            h: VIRTUAL_H,
        };
        let nodes = self.def.node.clone();
        let ctx = self.data.clone();
        let mut cursor = 0usize;
        self.layout_children(&mut cursor, &nodes, root, &ctx, "", None, p);
    }

    /// Lay out `nodes` (siblings) inside `parent`; `cursor` walks the flattened defs in preorder.
    #[allow(clippy::too_many_arguments)]
    fn layout_children(
        &mut self,
        cursor: &mut usize,
        nodes: &[NodeDef],
        parent: Rect,
        ctx: &Value,
        key_prefix: &str,
        clip: Option<Rect>,
        p: &mut dyn Painter,
    ) {
        for n in nodes {
            let def_idx = *cursor;
            *cursor += 1;
            let subtree = count(n);
            if let Some(v) = &n.visible
                && !visible(ctx, v)
            {
                *cursor += subtree;
                continue;
            }
            let key = match &n.id {
                Some(id) => format!("{key_prefix}{id}"),
                None => format!("{key_prefix}#{def_idx}"),
            };
            let rect = Self::node_rect(n, parent, ctx, p);
            self.place(n, def_idx, key, rect, ctx, clip, cursor, subtree, p);
        }
    }

    fn node_rect(n: &NodeDef, parent: Rect, ctx: &Value, p: &mut dyn Painter) -> Rect {
        let (mut w, mut h) = (
            n.w.as_ref().map(|d| d.resolve(parent.w)),
            n.h.as_ref().map(|d| d.resolve(parent.h)),
        );
        // text sizes itself; buttons fill the parent's width unless sized
        if n.kind == Kind::Text {
            let text = n
                .text
                .as_deref()
                .map(|t| interpolate(t, ctx))
                .unwrap_or_default();
            let font = n.font.unwrap_or(1);
            let scale = n.scale.unwrap_or(1.0);
            if w.is_none() {
                w = Some(p.text_width(font, &text, scale));
            }
            if h.is_none() {
                h = Some(p.text_height(font, scale));
            }
        } else if n.kind == Kind::Button && h.is_none() {
            let font = n.font.unwrap_or(1);
            h = Some(p.text_height(font, n.scale.unwrap_or(1.0)) * 2.0);
        }
        if n.kind == Kind::Image
            && (w.is_none() || h.is_none())
            && let Some(img) = &n.image
            && let Some((iw, ih)) = p.image_size(&interpolate(img, ctx))
        {
            let s = n.scale.unwrap_or(1.0);
            match (w, h) {
                (None, None) => {
                    w = Some(iw as f32 * s);
                    h = Some(ih as f32 * s);
                }
                (Some(ww), None) => h = Some(ww * ih as f32 / iw.max(1) as f32),
                (None, Some(hh)) => w = Some(hh * iw as f32 / ih.max(1) as f32),
                _ => {}
            }
        }
        let w = w.unwrap_or(parent.w);
        let h = h.unwrap_or(parent.h);
        let x = n.x.as_ref().map_or(0.0, |d| d.resolve(parent.w));
        let y = n.y.as_ref().map_or(0.0, |d| d.resolve(parent.h));
        let (ax, ay) = match n.anchor.as_deref().unwrap_or("tl") {
            "tr" => (parent.w - w - x, y),
            "bl" => (x, parent.h - h - y),
            "br" => (parent.w - w - x, parent.h - h - y),
            "center" => ((parent.w - w) / 2.0 + x, (parent.h - h) / 2.0 + y),
            "top" => ((parent.w - w) / 2.0 + x, y),
            "bottom" => ((parent.w - w) / 2.0 + x, parent.h - h - y),
            "left" => (x, (parent.h - h) / 2.0 + y),
            "right" => (parent.w - w - x, (parent.h - h) / 2.0 + y),
            _ => (x, y),
        };
        Rect {
            x: parent.x + ax,
            y: parent.y + ay,
            w,
            h,
        }
    }

    fn enter_offset(&self, n: &NodeDef) -> ((f32, f32), f32) {
        let Some(e) = &n.enter else {
            return ((0.0, 0.0), 1.0);
        };
        let dur = n.enter_time.unwrap_or(18.0).max(1.0);
        let t = ease_out((self.time - n.enter_delay.unwrap_or(0.0)) / dur);
        let d = 1.0 - t;
        let off = match e.as_str() {
            "left" => (-220.0 * d, 0.0),
            "right" => (220.0 * d, 0.0),
            "up" => (0.0, -160.0 * d),
            "down" => (0.0, 160.0 * d),
            _ => (0.0, 0.0),
        };
        (off, t)
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        n: &NodeDef,
        def_idx: usize,
        key: String,
        rect: Rect,
        ctx: &Value,
        clip: Option<Rect>,
        cursor: &mut usize,
        subtree: usize,
        p: &mut dyn Painter,
    ) {
        let ((ox, oy), _) = self.enter_offset(n);
        let rect = Rect {
            x: rect.x + ox,
            y: rect.y + oy,
            ..rect
        };
        let focusable = n.focus.unwrap_or(n.kind == Kind::Button)
            && (n.on_press.is_some() || n.on_left.is_some() || n.on_right.is_some());
        let scroller = matches!(n.kind, Kind::List | Kind::Grid);
        let child_clip = if scroller {
            Some(clip.map_or(rect, |c| c.intersect(&rect)))
        } else {
            clip
        };
        self.instances.push(Instance {
            def: def_idx,
            key: key.clone(),
            rect,
            clip,
            text: n.text.as_deref().map(|t| interpolate(t, ctx)),
            on_press: n.on_press.as_deref().map(|a| interpolate(a, ctx)),
            on_left: n.on_left.as_deref().map(|a| interpolate(a, ctx)),
            on_right: n.on_right.as_deref().map(|a| interpolate(a, ctx)),
            focusable,
            scroll_owner: None,
            ctx: ctx.clone(),
        });
        let me = self.instances.len() - 1;
        if n.children.is_empty() {
            *cursor += subtree;
            return;
        }
        let pad = n.pad.unwrap_or(0.0);
        let inner = Rect {
            x: rect.x + pad,
            y: rect.y + pad,
            w: rect.w - 2.0 * pad,
            h: rect.h - 2.0 * pad,
        };
        let gap = n.gap.unwrap_or(0.0);
        let scroll = if scroller {
            self.scroll.get(&key).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        // items: instance the children once per element
        let item_ctxs: Vec<(String, Value)> = match &n.items {
            Some(path) => match lookup(ctx, path.trim().trim_matches(['{', '}'])) {
                Some(Value::Array(a)) => a
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let mut c = ctx.clone();
                        if let Value::Object(m) = &mut c {
                            m.insert("item".into(), v.clone());
                            m.insert("index".into(), Value::from(i));
                        }
                        (format!("{key}[{i}]."), c)
                    })
                    .collect(),
                _ => Vec::new(),
            },
            None => vec![(format!("{key}."), ctx.clone())],
        };
        let start_cursor = *cursor;
        let mut pen_x = inner.x;
        let mut pen_y = inner.y - scroll;
        let mut content_h = 0.0f32;
        let cols = n.columns.unwrap_or(1).max(1) as usize;
        let cw = n.cell_w.unwrap_or(inner.w / cols as f32);
        let ch = n.cell_h.unwrap_or(inner.h);
        for (i, (prefix, c)) in item_ctxs.iter().enumerate() {
            *cursor = start_cursor;
            let slot = match n.kind {
                Kind::Vbox | Kind::List => Rect {
                    x: inner.x,
                    y: pen_y,
                    w: inner.w,
                    h: inner.h,
                },
                Kind::Hbox => Rect {
                    x: pen_x,
                    y: inner.y,
                    w: inner.w,
                    h: inner.h,
                },
                Kind::Grid => Rect {
                    x: inner.x + (i % cols) as f32 * (cw + gap),
                    y: inner.y + (i / cols) as f32 * (ch + gap) - scroll,
                    w: cw,
                    h: ch,
                },
                _ => inner,
            };
            let before = self.instances.len();
            self.layout_children(cursor, &n.children, slot, c, prefix, child_clip, p);
            let mut extent = 0.0f32;
            let owner = if scroller {
                Some(key.clone())
            } else {
                self.instances[me].scroll_owner.clone()
            };
            for inst in &mut self.instances[before..] {
                if inst.scroll_owner.is_none() {
                    inst.scroll_owner = owner.clone();
                }
                match n.kind {
                    Kind::Vbox | Kind::List => {
                        extent = extent.max(inst.rect.y + inst.rect.h - pen_y)
                    }
                    Kind::Hbox => extent = extent.max(inst.rect.x + inst.rect.w - pen_x),
                    _ => {}
                }
            }
            match n.kind {
                Kind::Vbox | Kind::List => {
                    pen_y += extent + gap;
                    content_h = pen_y + scroll - inner.y - gap;
                }
                Kind::Hbox => pen_x += extent + gap,
                Kind::Grid => content_h = ((i / cols) as f32 + 1.0) * (ch + gap) - gap,
                _ => {}
            }
        }
        *cursor = start_cursor + subtree;
        if scroller {
            let max = (content_h - inner.h).max(0.0);
            let cur = self.scroll.get(&key).copied().unwrap_or(0.0);
            self.scroll.insert(key, cur.clamp(0.0, max));
        }
    }

    // ---------------------------------------------------------------- input

    /// Advance one tick, apply navigation and mouse, return events.
    pub fn tick(&mut self, nav: Nav, p: &mut dyn Painter) -> Vec<Event> {
        self.time += 1.0;
        self.layout(p);
        let mut events = std::mem::take(&mut self.pending);
        if let Some((mx, my)) = self.mouse {
            if self.wheel != 0.0 {
                let owner = self
                    .instances
                    .iter()
                    .rev()
                    .find(|i| i.rect.contains(mx, my) && i.scroll_owner.is_some())
                    .and_then(|i| i.scroll_owner.clone())
                    .or_else(|| {
                        self.instances
                            .iter()
                            .rev()
                            .find(|i| {
                                i.rect.contains(mx, my)
                                    && matches!(self.defs[i.def].kind, Kind::List | Kind::Grid)
                            })
                            .map(|i| i.key.clone())
                    });
                if let Some(k) = owner {
                    let cur = self.scroll.get(&k).copied().unwrap_or(0.0);
                    self.scroll.insert(k, (cur - self.wheel * 48.0).max(0.0));
                }
            }
            let hovered = self
                .instances
                .iter()
                .rev()
                .find(|i| {
                    i.focusable
                        && i.rect.contains(mx, my)
                        && i.clip.is_none_or(|c| c.contains(mx, my))
                })
                .map(|i| i.key.clone());
            if let Some(h) = hovered {
                if self.focus.as_deref() != Some(&h) {
                    self.focus = Some(h.clone());
                    events.push(Event::Moved);
                }
                if self.mouse_down
                    && !self.mouse_was_down
                    && let Some(a) = self.find(&h).and_then(|i| i.on_press.clone())
                {
                    events.push(Event::Action(a));
                }
            }
        }
        self.wheel = 0.0;
        self.mouse_was_down = self.mouse_down;

        if self.focus.as_deref().is_none_or(|f| self.find(f).is_none()) {
            self.focus = self
                .instances
                .iter()
                .find(|i| i.focusable)
                .map(|i| i.key.clone());
        }
        let dir = if nav.up {
            Some((0.0, -1.0))
        } else if nav.down {
            Some((0.0, 1.0))
        } else if nav.left {
            Some((-1.0, 0.0))
        } else if nav.right {
            Some((1.0, 0.0))
        } else {
            None
        };
        if let Some(ci) = self.focus.clone().and_then(|f| self.find(&f).cloned()) {
            if nav.left && ci.on_left.is_some() {
                events.push(Event::Action(ci.on_left.clone().unwrap_or_default()));
            } else if nav.right && ci.on_right.is_some() {
                events.push(Event::Action(ci.on_right.clone().unwrap_or_default()));
            } else if let Some((dx, dy)) = dir
                && let Some(next) = self.nearest(&ci, dx, dy)
            {
                self.focus = Some(next);
                events.push(Event::Moved);
            }
            if nav.accept
                && let Some(a) = ci.on_press.clone()
            {
                events.push(Event::Action(a));
            }
        }
        if nav.back {
            events.push(Event::Back);
        }
        // keep the focused item visible inside its scroller
        if let Some(i) = self.focus.clone().and_then(|f| self.find(&f).cloned())
            && let Some(owner) = i.scroll_owner.clone()
            && let Some(o) = self.find(&owner).cloned()
        {
            let s = self.scroll.get(&owner).copied().unwrap_or(0.0);
            let top = i.rect.y - o.rect.y;
            let bottom = top + i.rect.h;
            if top < 0.0 {
                self.scroll.insert(owner, (s + top).max(0.0));
            } else if bottom > o.rect.h {
                self.scroll.insert(owner, s + bottom - o.rect.h);
            }
        }
        events
    }

    fn find(&self, key: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.key == key)
    }

    /// Spatial navigation: nearest focusable instance in direction `(dx, dy)`.
    fn nearest(&self, from: &Instance, dx: f32, dy: f32) -> Option<String> {
        let (fx, fy) = from.rect.center();
        let mut best: Option<(f32, &Instance)> = None;
        for i in self
            .instances
            .iter()
            .filter(|i| i.focusable && i.key != from.key)
        {
            let (cx, cy) = i.rect.center();
            let (vx, vy) = (cx - fx, cy - fy);
            let along = vx * dx + vy * dy;
            if along <= 1.0 {
                continue;
            }
            let across = (vx * dy).abs() + (vy * dx).abs();
            let score = along + across * 2.5;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        best.map(|(_, i)| i.key.clone())
    }

    // ---------------------------------------------------------------- draw

    pub fn draw(&mut self, p: &mut dyn Painter) {
        if self.instances.is_empty() {
            self.layout(p);
        }
        let instances = std::mem::take(&mut self.instances);
        for inst in &instances {
            let n = &self.defs[inst.def];
            let focused = self.focus.as_deref() == Some(&inst.key);
            let mut rect = inst.rect;
            let (_, t) = self.enter_offset(n);
            let alpha = n.alpha.unwrap_or(1.0) * t;
            if focused && let Some(hs) = n.hover_scale {
                let (cx, cy) = rect.center();
                rect = Rect {
                    x: cx - rect.w * hs / 2.0,
                    y: cy - rect.h * hs / 2.0,
                    w: rect.w * hs,
                    h: rect.h * hs,
                };
            }
            let skew = n.skew.unwrap_or(0.0);
            let clip = inst.clip;
            let color = if focused {
                n.color_focus.or(n.color)
            } else {
                n.color
            };
            if let Some(c) = color {
                p.draw_rect(rect, [c[0], c[1], c[2], c[3] * alpha], skew, clip);
            }
            let image = if focused {
                n.image_focus.as_ref().or(n.image.as_ref())
            } else {
                n.image.as_ref()
            };
            if let Some(img) = image {
                let key = interpolate(img, &inst.ctx);
                match n.nine {
                    Some(b) => Self::nine_slice(p, &key, b, rect, alpha, clip),
                    None => p.draw_image(&key, None, rect, skew, alpha, clip),
                }
            }
            if let Some(text) = &inst.text
                && matches!(n.kind, Kind::Text | Kind::Button)
                && !text.is_empty()
            {
                let font = n.font.unwrap_or(1);
                let scale = n.scale.unwrap_or(1.0);
                let bank = if focused {
                    n.bank_focus.or(n.bank)
                } else {
                    n.bank
                }
                .unwrap_or(0);
                let default_align = if n.kind == Kind::Button {
                    "center"
                } else {
                    "left"
                };
                let (align, x) = match n.align.as_deref().unwrap_or(default_align) {
                    "right" => (-1, rect.x + rect.w),
                    "center" => (0, rect.x + rect.w / 2.0),
                    _ => (1, rect.x),
                };
                let th = p.text_height(font, scale);
                let y = rect.y + (rect.h + th) / 2.0;
                p.draw_text(
                    font,
                    text,
                    x + skew / 2.0,
                    y,
                    bank,
                    align,
                    scale,
                    alpha,
                    clip,
                );
            }
        }
        self.instances = instances;
    }

    fn nine_slice(
        p: &mut dyn Painter,
        key: &str,
        b: [u32; 4],
        r: Rect,
        alpha: f32,
        clip: Option<Rect>,
    ) {
        let Some((iw, ih)) = p.image_size(key) else {
            return;
        };
        let (l, t, rr, bb) = (b[0], b[1], b[2], b[3]);
        let (lf, tf, rf, bf) = (l as f32, t as f32, rr as f32, bb as f32);
        let mid_w = iw.saturating_sub(l + rr);
        let mid_h = ih.saturating_sub(t + bb);
        let xs = [
            (0u32, l, r.x, lf),
            (l, mid_w, r.x + lf, r.w - lf - rf),
            (iw.saturating_sub(rr), rr, r.x + r.w - rf, rf),
        ];
        let ys = [
            (0u32, t, r.y, tf),
            (t, mid_h, r.y + tf, r.h - tf - bf),
            (ih.saturating_sub(bb), bb, r.y + r.h - bf, bf),
        ];
        for (sy, sh, dy, dh) in ys {
            for (sx, sw, dx, dw) in xs {
                if sw == 0 || sh == 0 || dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                p.draw_image(
                    key,
                    Some([sx, sy, sw, sh]),
                    Rect {
                        x: dx,
                        y: dy,
                        w: dw,
                        h: dh,
                    },
                    0.0,
                    alpha,
                    clip,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPainter {
        drawn: Vec<String>,
    }
    impl Painter for TestPainter {
        fn image_size(&mut self, _: &str) -> Option<(u32, u32)> {
            Some((64, 32))
        }
        fn draw_image(
            &mut self,
            key: &str,
            _: Option<[u32; 4]>,
            _: Rect,
            _: f32,
            _: f32,
            _: Option<Rect>,
        ) {
            self.drawn.push(format!("img:{key}"));
        }
        fn draw_rect(&mut self, _: Rect, _: [f32; 4], _: f32, _: Option<Rect>) {
            self.drawn.push("rect".into());
        }
        fn text_width(&mut self, _: i32, text: &str, scale: f32) -> f32 {
            text.len() as f32 * 8.0 * scale
        }
        fn text_height(&mut self, _: i32, scale: f32) -> f32 {
            12.0 * scale
        }
        #[allow(clippy::too_many_arguments)]
        fn draw_text(
            &mut self,
            _: i32,
            text: &str,
            _: f32,
            _: f32,
            _: i32,
            _: i32,
            _: f32,
            _: f32,
            _: Option<Rect>,
        ) {
            self.drawn.push(format!("text:{text}"));
        }
    }

    const MENU: &str = r#"
[[node]]
kind = "vbox"
id = "menu"
x = 60
y = 180
w = 360
gap = 8
items = "menu"
[[node.children]]
kind = "button"
h = 52
text = "{item.label}"
on_press = "{item.action}"

[[node]]
kind = "text"
text = "Hello {profile.name}"
anchor = "tr"
x = 20
y = 20
"#;

    fn data() -> Value {
        serde_json::json!({
            "menu": [
                {"label": "ARCADE", "action": "mode:arcade"},
                {"label": "VS MODE", "action": "mode:versus"},
                {"label": "EXIT", "action": "quit"}
            ],
            "profile": {"name": "Axel"}
        })
    }

    #[test]
    fn items_instance_children_and_binds() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter { drawn: Vec::new() };
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.is_empty());
        ui.draw(&mut p);
        assert!(p.drawn.contains(&"text:ARCADE".to_string()));
        assert!(p.drawn.contains(&"text:Hello Axel".to_string()));
        let ev = ui.tick(
            Nav {
                down: true,
                ..Default::default()
            },
            &mut p,
        );
        assert_eq!(ev, vec![Event::Moved]);
        let ev = ui.tick(
            Nav {
                accept: true,
                ..Default::default()
            },
            &mut p,
        );
        assert_eq!(ev, vec![Event::Action("mode:versus".into())]);
    }

    #[test]
    fn mouse_hover_and_click() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter { drawn: Vec::new() };
        ui.tick(Nav::default(), &mut p);
        // third button spans y = 180 + 2 * (52 + 8) .. + 52
        ui.mouse_move(Some((100.0, 180.0 + 120.0 + 10.0)));
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Moved]);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("quit".into())]);
    }

    #[test]
    fn visible_and_percent() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "panel"
w = "50%"
h = 100
color = [1,0,0,1]
visible = "profile.name"
[[node]]
kind = "panel"
w = 10
h = 10
color = [0,1,0,1]
visible = "profile.missing"
[[node]]
kind = "panel"
w = 10
h = 10
visible = "!profile.missing"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter { drawn: Vec::new() };
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.instances.len(), 2);
        assert_eq!(ui.instances[0].rect.w, 640.0);
    }

    #[test]
    fn grid_scrolls_to_keep_focus_visible() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "grid"
id = "roster"
x = 0
y = 0
w = 400
h = 100
columns = 4
cell_w = 100
cell_h = 100
items = "cells"
[[node.children]]
kind = "button"
w = 90
h = 90
on_press = "pick:{index}"
"#,
        )
        .unwrap();
        let cells: Vec<Value> = (0..16).map(|i| serde_json::json!({ "i": i })).collect();
        let mut ui = Ui::new(def, serde_json::json!({ "cells": cells }));
        let mut p = TestPainter { drawn: Vec::new() };
        ui.tick(Nav::default(), &mut p);
        for _ in 0..3 {
            ui.tick(
                Nav {
                    down: true,
                    ..Default::default()
                },
                &mut p,
            );
        }
        let ev = ui.tick(
            Nav {
                accept: true,
                ..Default::default()
            },
            &mut p,
        );
        assert_eq!(ev, vec![Event::Action("pick:12".into())]);
        assert!(ui.scroll["roster"] > 0.0, "grid scrolled: {:?}", ui.scroll);
    }
}
