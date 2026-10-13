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
//!
//! Boxes without `items` stack their children as siblings (`gap`, `justify`,
//! `align`, child `grow`); boxes with `items` give every element one slot in
//! which the children overlap. Widgets: `panel image text button vbox hbox
//! list grid input slider toggle particles pips choice keycap icon` (see
//! `widgets.rs`); an input with `mask` (a bind path) draws `*` while it is
//! truthy (passwords). Two players can navigate at once
//! (`set_dual`): P2's focus uses the `*_focus2` visuals and fires
//! [`Event::Action2`].
//!
//! Decoration and big-roster keys: `angle` rotates a subtree, `cut` clips it
//! to a slanted quad, `fit`/`flip`/`grey` place portraits, `outline` frames a
//! node, text with `pad` sizes itself to its text and `wrap` breaks lines
//! (`max_lines` caps them; `ellipsis` ends a line that does not fit in
//! `...`), an input with `caret` draws a blinking bar while edited, a `modal`
//! subtree keeps the focus while it is shown, a `hug` box is as long as its
//! children,
//! `when = "focus"|"blur"|"focus2"` and `w_focus`/`h_focus`/`scale_focus`
//! react to the focus of the node's scope, `shadow_focus` / `border_focus`
//! give the focused node a hard drop shadow and a thicker edge (a focused
//! choice also fills its picked option with `color_focus`), `top` draws over
//! everything, and a
//! `paged` grid scrolls whole pages (publishing `pages.<id>`), with `bleed`
//! letting cursor frames draw outside its clip.
//!
//! Responsive screens: screens are designed in 1280x720 but the layout root is
//! the real viewport ([`viewport`]: 720 high, wider on wide windows, taller on
//! narrow ones). Top-level nodes sit in the 1280x720 design frame, centred,
//! unless `dock` moves them: `left` / `right` / `center` and `top` / `bottom` /
//! `middle` pick the frame's edge on each axis, `fill-x` / `fill-y` / `fill`
//! stretch the frame to the whole viewport (a `"100%"` size fills its axis on
//! its own), and `stretch-x` / `stretch-y` / `stretch` keep the node at the
//! start but grow its pixel size by the extra room. A filled or stretched
//! node's children may dock inside it the same way.

#![forbid(unsafe_code)]

mod widgets;

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

pub const VIRTUAL_W: f32 = 1280.0;
pub const VIRTUAL_H: f32 = 720.0;

/// Virtual viewport for a window: 720 high and wider than 1280 on wide
/// windows; narrower windows keep 1280 columns and grow taller instead.
pub fn viewport(window: (u32, u32)) -> (f32, f32) {
    let aspect = window.0.max(1) as f32 / window.1.max(1) as f32;
    if aspect >= VIRTUAL_W / VIRTUAL_H {
        (VIRTUAL_H * aspect, VIRTUAL_H)
    } else {
        (VIRTUAL_W, VIRTUAL_W / aspect)
    }
}

/// Window pixel to virtual viewport coordinates.
pub fn window_to_virtual(window: (u32, u32), px: f32, py: f32) -> (f32, f32) {
    let (vw, vh) = viewport(window);
    (
        px * vw / window.0.max(1) as f32,
        py * vh / window.1.max(1) as f32,
    )
}

/// Where a node sits on one axis of the responsive frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Start,
    Middle,
    End,
    Fill,
    /// Start-aligned, and a pixel size grows by the extra room.
    Stretch,
}

/// `dock` tokens → (horizontal, vertical); `None` when the axis is not named.
fn dock_edges(dock: &str) -> (Option<Edge>, Option<Edge>) {
    let (mut h, mut v) = (None, None);
    for t in dock.split([' ', ',']).filter(|t| !t.is_empty()) {
        match t {
            "left" => h = Some(Edge::Start),
            "center" => h = Some(Edge::Middle),
            "right" => h = Some(Edge::End),
            "fill-x" => h = Some(Edge::Fill),
            "top" => v = Some(Edge::Start),
            "middle" => v = Some(Edge::Middle),
            "bottom" => v = Some(Edge::End),
            "fill-y" => v = Some(Edge::Fill),
            "fill" => (h, v) = (Some(Edge::Fill), Some(Edge::Fill)),
            "stretch-x" => h = Some(Edge::Stretch),
            "stretch-y" => v = Some(Edge::Stretch),
            "stretch" => (h, v) = (Some(Edge::Stretch), Some(Edge::Stretch)),
            "tl" => (h, v) = (Some(Edge::Start), Some(Edge::Start)),
            "tr" => (h, v) = (Some(Edge::End), Some(Edge::Start)),
            "bl" => (h, v) = (Some(Edge::Start), Some(Edge::End)),
            "br" => (h, v) = (Some(Edge::End), Some(Edge::End)),
            _ => {}
        }
    }
    (h, v)
}

/// Offset and length of a frame along one axis: `full` available, `extra` of it beyond the design size.
fn frame_axis(edge: Edge, full: f32, extra: f32) -> (f32, f32) {
    match edge {
        Edge::Start | Edge::Stretch => (0.0, full - extra),
        Edge::Middle => (extra / 2.0, full - extra),
        Edge::End => (extra, full - extra),
        Edge::Fill => (0.0, full),
    }
}

fn is_pct(d: &Option<Dim>) -> bool {
    matches!(d, Some(Dim::Pct(s)) if s.trim_end().ends_with('%'))
}

/// A size or position: a number of virtual pixels or a `"50%"` of the parent.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Dim {
    Px(f32),
    Pct(String),
}

impl Dim {
    /// Pixels; strings may bind (`"{item.dx}"`, `"{pad.x}%"`) and end in `%`.
    fn resolve<S: Scope + ?Sized>(&self, parent: f32, ctx: &S) -> f32 {
        match self {
            Dim::Px(v) => *v,
            Dim::Pct(s) => {
                let s = interpolate(s, ctx);
                let t = s.trim();
                match t.strip_suffix('%') {
                    Some(pct) => pct
                        .trim()
                        .parse::<f32>()
                        .map_or(0.0, |p| p / 100.0 * parent),
                    None => t.parse::<f32>().unwrap_or(0.0),
                }
            }
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
    /// One-line text field: focus it, press accept to edit, Enter/Esc to stop.
    Input,
    /// Horizontal slider over `value` between `min` and `max`.
    Slider,
    /// On/off switch showing `image_on` when `value` is truthy.
    Toggle,
    /// Drifting pixel sparks inside the node's rectangle.
    Particles,
    /// Level squares: `max` (or `max_from`) boxes, the first `value` filled with `color2`.
    Pips,
    /// Segmented options from the `options` array; `value` is the selected index (or string).
    Choice,
    /// A key cap showing `text` (or the built-in `glyph`); `w` is its minimum width.
    Keycap,
    /// One built-in glyph (`glyph = "up"`, ...) in `color`.
    Icon,
    /// Host-drawn content in the node's rectangle, in draw order (`id`
    /// names it: a live stage preview, ...). See [`Painter::draw_slot`].
    Slot,
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
    /// Responsive placement (see the crate docs): `left`/`center`/`right`/`fill-x`,
    /// `top`/`middle`/`bottom`/`fill-y`, `fill`, `stretch-x`/`stretch-y`/`stretch`,
    /// `tl`/`tr`/`bl`/`br`.
    pub dock: Option<String>,
    /// Solid fill `[r, g, b, a]` 0..1.
    pub color: Option<[f32; 4]>,
    pub color_focus: Option<[f32; 4]>,
    pub color_focus2: Option<[f32; 4]>,
    /// Second colour: slider fill / particle tint variation / toggle "on" fill.
    pub color2: Option<[f32; 4]>,
    /// Asset key: `ui/x.webp`, `ui/x.png` or `sff:group,number`.
    pub image: Option<String>,
    pub image_focus: Option<String>,
    pub image_focus2: Option<String>,
    /// Toggle: image shown when `value` is truthy.
    pub image_on: Option<String>,
    /// 9-slice borders `[left, top, right, bottom]` in texels.
    pub nine: Option<[u32; 4]>,
    /// Horizontal shear in virtual pixels (slanted panels).
    pub skew: Option<f32>,
    pub text: Option<String>,
    pub font: Option<i32>,
    pub bank: Option<i32>,
    pub bank_focus: Option<i32>,
    pub bank_focus2: Option<i32>,
    /// Text: `left`, `center`, `right`. Boxes: cross-axis `start`, `center`, `end`, `stretch`.
    pub align: Option<String>,
    /// Boxes: main-axis `start`, `center`, `end`, `between`.
    pub justify: Option<String>,
    /// Box child: take the remaining space along the main axis.
    pub grow: bool,
    pub scale: Option<f32>,
    pub on_press: Option<String>,
    pub on_left: Option<String>,
    pub on_right: Option<String>,
    /// Input / slider: action fired with `{value}` replaced when the value changes.
    pub on_change: Option<String>,
    /// A focusable row holding a `pips`, `choice` or `slider` part: a click
    /// (or a drag, on a slider) on that part fires this with `{value}` = the
    /// pips filled up to the cursor (1..), the choice index (0..) or the
    /// slider fraction (0..1) under it, instead of `on_press`.
    pub on_pick: Option<String>,
    /// Buttons/inputs/sliders/toggles are focusable by default; `focus = false` disables.
    pub focus: Option<bool>,
    /// Bind path; the node is skipped when the value is false/empty/0.
    pub visible: Option<String>,
    /// Bind path (truthy → drawn with the focus visuals; tabs, toggles).
    pub selected: Option<String>,
    /// Bind path of the widget value (input text, slider number, toggle flag).
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub max_len: Option<usize>,
    /// Input: bind path; while truthy the text is drawn as `*` (passwords).
    pub mask: Option<String>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
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
    /// Extra enter delay per child / item index (staggered reveals).
    pub stagger: Option<f32>,
    /// `out` (default), `linear`, `back`, `bounce`.
    pub ease: Option<String>,
    /// How the node leaves (defaults to the reverse of `enter`); `exit_time` ticks.
    pub exit: Option<String>,
    pub exit_time: Option<f32>,
    pub hover_scale: Option<f32>,
    pub alpha: Option<f32>,
    /// Tooltip shown after the node has been focused for a moment.
    pub tip: Option<String>,
    /// Particles: how many, drift speed (px/tick), pixel size.
    pub count: Option<u32>,
    pub speed: Option<f32>,
    pub size: Option<f32>,
    /// Outline drawn inside the rect (any kind; pips / choice / keycap / slider use it too).
    pub border: Option<f32>,
    pub border_color: Option<[f32; 4]>,
    pub border_color_focus: Option<[f32; 4]>,
    /// Border thickness while focused (default `border`).
    pub border_focus: Option<f32>,
    /// Hard drop shadow `[dx, dy]` drawn behind the node while it is focused
    /// (or `selected`), in `shadow_color` (default ink).
    pub shadow_focus: Option<[f32; 2]>,
    pub shadow_color: Option<[f32; 4]>,
    /// Keycap: extra thickness of the bottom edge.
    pub depth: Option<f32>,
    /// Glyph colour on choices and keycaps (`@name` labels, `glyph`).
    pub text_color: Option<[f32; 4]>,
    pub text_color_focus: Option<[f32; 4]>,
    /// Built-in glyph name for `icon` / `keycap` (may bind: `"{item.glyph}"`).
    pub glyph: Option<String>,
    /// Choice: bind path to the array of option labels.
    pub options: Option<String>,
    /// Bind path overriding `max` (pips count).
    pub max_from: Option<String>,
    /// Styled slider: knob `[w, h]`, knob and track fills.
    pub knob: Option<[f32; 2]>,
    pub knob_color: Option<[f32; 4]>,
    pub track_color: Option<[f32; 4]>,
    /// Image: repeat at its own size (times `scale`) instead of stretching.
    pub tile: bool,
    /// Use the focus visuals while an ancestor is focused.
    pub inherit_focus: bool,
    /// Blink with this period in ticks.
    pub blink: Option<f32>,
    /// Navigation group (inherited by children): up/down stay inside the group when they can.
    pub group: Option<String>,
    /// Extra space between wrapped lines (default 40% of the line height).
    pub line_gap: Option<f32>,
    // ---- transforms, image fitting, slanted clips, focus scopes, paging
    /// Rotation in degrees (clockwise, like CSS) of the node and its subtree
    /// around the node's centre. Hit-testing ignores it.
    pub angle: Option<f32>,
    /// Image: mirror horizontally.
    pub flip: bool,
    /// Image: draw in greyscale.
    pub grey: bool,
    /// Image: `stretch` (default), `contain` (letterbox) or `cover` (crop); keeps the aspect.
    pub fit: Option<String>,
    /// Image fit alignment 0..1 per axis (default 0.5; `fit_y = 1` sits on the bottom edge).
    pub fit_x: Option<f32>,
    pub fit_y: Option<f32>,
    /// Slanted clip of the node and its subtree: horizontal insets
    /// `[top-left, bottom-left, top-right, bottom-right]` in virtual pixels.
    pub cut: Option<[f32; 4]>,
    /// `focus` / `blur` (alias `idle`) / `focus2`: only laid out while the
    /// node's focus scope is (not) focused. The scope is the node itself, its
    /// descendants, its focused ancestors, a `selected` node and, inside an
    /// `items` instance, every sibling of that instance.
    pub when: Option<String>,
    /// Size / text scale used while the focus scope is focused.
    pub w_focus: Option<Dim>,
    pub h_focus: Option<Dim>,
    pub scale_focus: Option<f32>,
    /// Grid: scroll a whole page of rows at a time. A paged grid with an `id`
    /// publishes `{pages.<id>.page}` and `{pages.<id>.count}` (1-based).
    pub paged: bool,
    /// Lists / grids: let children draw this many pixels outside the clip
    /// (cursor frames, tabs).
    pub bleed: Option<f32>,
    /// Frame drawn around the node's rectangle, `outline_w` pixels thick (default 3).
    pub outline: Option<[f32; 4]>,
    pub outline_w: Option<f32>,
    /// Text: wrap words to the node's `w`. With `line_h` lines are that far
    /// apart from the top; otherwise they are `line_gap` apart (default 40% of
    /// the line height) and centred vertically in `h`.
    pub wrap: bool,
    pub line_h: Option<f32>,
    /// Draw the node and its subtree after everything else (cursor overlays).
    pub top: bool,
    /// Vertical nudge of the node's text in virtual pixels (negative = up).
    pub text_dy: Option<f32>,
    /// Text / button: a line wider than the node ends in `...` instead of
    /// running out of it (wrapped lines too).
    pub ellipsis: bool,
    /// Wrapped text: at most this many lines; the last one ends in `...`.
    pub max_lines: Option<usize>,
    /// Input: caret colour. While editing, a blinking bar follows the text
    /// (instead of `_`) and a text wider than the box shows its end.
    pub caret: Option<[f32; 4]>,
    /// Modal subtree (dialogs): while it is laid out, only focusable nodes
    /// inside it take the focus; the focus comes back when it closes.
    pub modal: bool,
    /// Box without `items` and without `w` (hbox) / `h` (vbox): as long as
    /// its shown children and gaps (plus `pad`), not the parent's size.
    pub hug: bool,
    pub children: Vec<NodeDef>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScreenDef {
    /// Clear colour.
    pub background: Option<[f32; 4]>,
    /// Ticks the leave animation takes (default 12).
    pub exit_time: Option<f32>,
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
    fn scaled(&self, s: f32) -> Rect {
        let (cx, cy) = self.center();
        Rect {
            x: cx - self.w * s / 2.0,
            y: cy - self.h * s / 2.0,
            w: self.w * s,
            h: self.h * s,
        }
    }
}

/// Rotation of a subtree: clockwise degrees around `(cx, cy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rot {
    pub deg: f32,
    pub cx: f32,
    pub cy: f32,
}

/// Image drawing options beyond position and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageFx {
    pub skew: f32,
    pub alpha: f32,
    /// Mirror horizontally.
    pub flip: bool,
    /// Greyscale.
    pub grey: bool,
}

/// A slanted clip region: left and right edges are straight lines from `y0` to `y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Quad {
    y0: f32,
    y1: f32,
    /// Left edge x at `y0` and `y1`.
    l0: f32,
    l1: f32,
    /// Right edge x at `y0` and `y1`.
    r0: f32,
    r1: f32,
}

impl Quad {
    fn of(r: Rect, cut: [f32; 4]) -> Self {
        Self {
            y0: r.y,
            y1: r.y + r.h,
            l0: r.x + cut[0],
            l1: r.x + cut[1],
            r0: r.x + r.w - cut[2],
            r1: r.x + r.w - cut[3],
        }
    }
    fn span(&self, y: f32) -> (f32, f32) {
        let t = if self.y1 > self.y0 {
            ((y - self.y0) / (self.y1 - self.y0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (
            self.l0 + (self.l1 - self.l0) * t,
            self.r0 + (self.r1 - self.r0) * t,
        )
    }
    fn bounds(&self) -> Rect {
        let x0 = self.l0.min(self.l1);
        let x1 = self.r0.max(self.r1);
        Rect {
            x: x0,
            y: self.y0,
            w: (x1 - x0).max(0.0),
            h: (self.y1 - self.y0).max(0.0),
        }
    }
}

/// Forwards to another painter, clipping images and rectangles to a [`Quad`]
/// by drawing them in thin horizontal strips.
struct CutPainter<'a> {
    inner: &'a mut dyn Painter,
    quad: Quad,
}

impl CutPainter<'_> {
    const STRIP: f32 = 2.0;

    fn strips(&self, dst: Rect, clip: Option<Rect>) -> Vec<Rect> {
        let mut out = Vec::new();
        let y_end = (dst.y + dst.h).min(self.quad.y1);
        let mut y = dst.y.max(self.quad.y0);
        while y < y_end {
            let h = Self::STRIP.min(y_end - y);
            let (l, r) = self.quad.span(y + h / 2.0);
            let mut c = Rect {
                x: l,
                y,
                w: (r - l).max(0.0),
                h,
            };
            if let Some(cl) = clip {
                c = c.intersect(&cl);
            }
            if c.w > 0.0 && c.h > 0.0 {
                out.push(c);
            }
            y += h;
        }
        out
    }
}

impl Painter for CutPainter<'_> {
    fn image_size(&mut self, key: &str) -> Option<(u32, u32)> {
        self.inner.image_size(key)
    }
    fn draw_image(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        skew: f32,
        alpha: f32,
        clip: Option<Rect>,
    ) {
        for c in self.strips(dst, clip) {
            self.inner.draw_image(key, src, dst, skew, alpha, Some(c));
        }
    }
    fn draw_image_fx(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        fx: &ImageFx,
        clip: Option<Rect>,
    ) {
        for c in self.strips(dst, clip) {
            self.inner.draw_image_fx(key, src, dst, fx, Some(c));
        }
    }
    fn draw_rect(&mut self, dst: Rect, color: [f32; 4], skew: f32, clip: Option<Rect>) {
        for c in self.strips(dst, clip) {
            self.inner.draw_rect(dst, color, skew, Some(c));
        }
    }
    fn text_width(&mut self, font: i32, text: &str, scale: f32) -> f32 {
        self.inner.text_width(font, text, scale)
    }
    fn text_height(&mut self, font: i32, scale: f32) -> f32 {
        self.inner.text_height(font, scale)
    }
    fn text_center_dy(&mut self, font: i32, scale: f32) -> f32 {
        self.inner.text_center_dy(font, scale)
    }
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
    ) {
        let b = self.quad.bounds();
        let c = Some(clip.map_or(b, |c| c.intersect(&b)));
        self.inner
            .draw_text(font, text, x, y, bank, align, scale, alpha, c);
    }
    fn set_transform(&mut self, rot: Option<Rot>) {
        self.inner.set_transform(rot);
    }
}

/// What the host draws for the engine. Coordinates are virtual pixels.
pub trait Painter {
    /// Rotate everything drawn until the next call (None = no rotation).
    fn set_transform(&mut self, _rot: Option<Rot>) {}
    /// [`draw_image`](Self::draw_image) with flip / greyscale; hosts without
    /// support fall back to the plain draw.
    fn draw_image_fx(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        fx: &ImageFx,
        clip: Option<Rect>,
    ) {
        self.draw_image(key, src, dst, fx.skew, fx.alpha, clip);
    }
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
    /// A `slot` node: whatever the host draws under that name.
    fn draw_slot(&mut self, _id: &str, _dst: Rect, _alpha: f32, _clip: Option<Rect>) {}
    fn text_width(&mut self, font: i32, text: &str, scale: f32) -> f32;
    /// Height of one text line for the font.
    fn text_height(&mut self, font: i32, scale: f32) -> f32;
    /// How far below the centre the visible glyphs land when a line is
    /// centred on [`text_height`](Self::text_height); centred labels move up by it.
    fn text_center_dy(&mut self, _font: i32, _scale: f32) -> f32 {
        0.0
    }
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub accept: bool,
    pub back: bool,
}

/// A key while a text input is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKey {
    Char(char),
    Backspace,
    Enter,
    Escape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A node fired its `on_press` / `on_left` / `on_right` / `on_change` action.
    Action(String),
    /// Same, fired by the second player's focus.
    Action2(String),
    /// Focus moved (host may play a sound).
    Moved,
    Moved2,
    /// An input's text changed: (node id, new text).
    Input(String, String),
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
    on_change: Option<String>,
    on_pick: Option<String>,
    focusable: bool,
    /// Key of the enclosing list/grid, for wheel scrolling and keep-visible.
    scroll_owner: Option<String>,
    /// Extra enter delay from `stagger`.
    delay: f32,
    /// Context the instance was built with (for `{bind}` images at draw time).
    ctx: Ctx,
    /// Slanted clip inherited from the nearest `cut` ancestor (or itself).
    quad: Option<Quad>,
    /// Rotation inherited from the nearest `angle` ancestor (or itself).
    rot: Option<Rot>,
    /// Its focus scope was focused at layout time (`*_focus` sizes, `scale_focus`).
    scope_lit: bool,
    /// Laid out next to the visible page of a paged grid: navigable, not drawn.
    offpage: bool,
    /// Drawn in the overlay pass (`top` on itself or an ancestor).
    top: bool,
    /// Inside a `modal` subtree.
    modal: bool,
}

pub struct Ui {
    def: ScreenDef,
    /// Flattened defs (preorder) for stable indices.
    defs: Vec<NodeDef>,
    data: std::rc::Rc<Value>,
    instances: Vec<Instance>,
    focus: Option<String>,
    focus2: Option<String>,
    dual: bool,
    focus_age: f32,
    scroll: HashMap<String, f32>,
    /// Edit buffers of inputs the user touched, by node id.
    inputs: HashMap<String, String>,
    /// Input currently taking keys.
    editing: Option<String>,
    /// Focus before `edit()` moved it onto an input; it comes back when the
    /// edit ends if the input itself is not focusable.
    edit_from: Option<String>,
    time: f32,
    /// Ticks since `leave()` was called.
    leaving: Option<f32>,
    mouse: Option<(f32, f32)>,
    mouse_down: bool,
    mouse_was_down: bool,
    /// Mouse position the hover focus was last taken from: a still cursor
    /// does not take the focus back from the keyboard.
    mouse_seen: Option<(f32, f32)>,
    /// The focus the mouse gave and where the cursor was: a click from the
    /// same spot activates it even when the new focus re-laid the screen
    /// out from under the cursor (a focused item that grows).
    mouse_focus: Option<(String, (f32, f32))>,
    wheel: f32,
    pending: Vec<Event>,
    /// Slanted clip / rotation of the subtree being laid out.
    cur_quad: Option<Quad>,
    cur_rot: Option<Rot>,
    cur_top: bool,
    cur_modal: bool,
    /// Focus before a modal subtree took it (restored when it closes).
    modal_return: Option<String>,
    /// Paged grids by id: (page, pages), 1-based, from the last layout.
    pages: HashMap<String, (usize, usize)>,
    /// Scrollers follow the second player's focus (it moved last).
    follow2: bool,
    /// Layout root size (virtual units), see [`viewport`].
    size: (f32, f32),
    /// Room beyond the design size the children being laid out may dock into.
    cur_extra: (f32, f32),
    /// Laying out the top-level nodes.
    cur_root: bool,
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

/// Something `{bind}` paths resolve against: plain data or a layout [`Ctx`].
pub trait Scope {
    fn bound(&self, path: &str) -> Option<&Value>;
}

impl Scope for Value {
    fn bound(&self, path: &str) -> Option<&Value> {
        lookup(self, path)
    }
}

/// Layout context: the screen data, shared, plus the innermost `items`
/// element (`item`, `index`). Cheap to clone: instances keep one each, and
/// copying the whole data per instance made big grids cost O(n²) per tick.
#[derive(Clone, Debug)]
pub struct Ctx {
    root: std::rc::Rc<Value>,
    item: Option<std::rc::Rc<(Value, Value)>>,
}

impl Ctx {
    pub fn new(root: Value) -> Self {
        Self {
            root: std::rc::Rc::new(root),
            item: None,
        }
    }

    /// The same data with `item` / `index` bound to an `items` element.
    fn with_item(&self, item: Value, index: usize) -> Self {
        Self {
            root: self.root.clone(),
            item: Some(std::rc::Rc::new((item, Value::from(index)))),
        }
    }
}

impl Scope for Ctx {
    fn bound(&self, path: &str) -> Option<&Value> {
        if let Some(layer) = &self.item {
            let mut parts = path.split('.').map(str::trim).filter(|p| !p.is_empty());
            match parts.next() {
                Some("item") => {
                    let rest = path.split_once("item").map_or("", |(_, r)| r);
                    return lookup(&layer.0, rest);
                }
                Some("index") => {
                    return if parts.next().is_none() {
                        Some(&layer.1)
                    } else {
                        None
                    };
                }
                _ => {}
            }
        }
        lookup(&self.root, path)
    }
}

fn bind_path(expr: &str) -> &str {
    expr.trim().trim_matches(['{', '}'])
}

fn visible<S: Scope + ?Sized>(ctx: &S, expr: &str) -> bool {
    let e = bind_path(expr);
    match e.strip_prefix('!') {
        Some(rest) => !truthy(ctx.bound(rest)),
        None => truthy(ctx.bound(e)),
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

fn value_num(v: Option<&Value>) -> f32 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) as f32,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(Value::Bool(b)) => f32::from(*b),
        _ => 0.0,
    }
}

/// Replace `{path}` with context values.
pub fn interpolate<S: Scope + ?Sized>(s: &str, ctx: &S) -> String {
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
                out.push_str(&ctx.bound(path).map(value_text).unwrap_or_default());
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

/// Easing curves over 0..=1.
pub fn ease(kind: &str, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        "linear" => t,
        "in" => t * t * t,
        "back" => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (t - 1.0) * (t - 1.0) * (t - 1.0) + c1 * (t - 1.0) * (t - 1.0)
        }
        "bounce" => {
            let (n1, d1) = (7.5625, 2.75);
            if t < 1.0 / d1 {
                n1 * t * t
            } else if t < 2.0 / d1 {
                let t = t - 1.5 / d1;
                n1 * t * t + 0.75
            } else if t < 2.5 / d1 {
                let t = t - 2.25 / d1;
                n1 * t * t + 0.9375
            } else {
                let t = t - 2.625 / d1;
                n1 * t * t + 0.984375
            }
        }
        _ => 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t),
    }
}

/// Greedy word wrap of `text` into lines no wider than `width`.
fn wrap_text(p: &mut dyn Painter, font: i32, scale: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let cand = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if !cur.is_empty() && p.text_width(font, &cand, scale) > width {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `text` shortened to `width` with a trailing `...` (unchanged when it fits).
fn ellipsize(p: &mut dyn Painter, font: i32, scale: f32, text: &str, width: f32) -> String {
    if p.text_width(font, text, scale) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let head: String = chars.iter().collect();
        let cand = format!("{}...", head.trim_end());
        if p.text_width(font, &cand, scale) <= width {
            return cand;
        }
    }
    String::new()
}

/// Wrapped lines limited to `max_lines` (the last kept line ends in `...`
/// when lines were dropped); with `ellipsis`, lines too wide (one long
/// word) are shortened too.
fn fit_lines(
    p: &mut dyn Painter,
    font: i32,
    scale: f32,
    mut lines: Vec<String>,
    n: &NodeDef,
    width: f32,
) -> Vec<String> {
    if let Some(max) = n.max_lines
        && lines.len() > max
        && max > 0
    {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            // drop whole words first, then characters of a single long word
            let mut words: Vec<&str> = last.split(' ').collect();
            let mut cut = format!("{}...", words.join(" "));
            while words.len() > 1 && p.text_width(font, &cut, scale) > width {
                words.pop();
                cut = format!("{}...", words.join(" "));
            }
            if p.text_width(font, &cut, scale) > width {
                cut = ellipsize(p, font, scale, &format!("{} ", words.join(" ")), width);
            }
            *last = cut;
        }
    }
    if n.ellipsis || n.max_lines.is_some() {
        for l in &mut lines {
            *l = ellipsize(p, font, scale, l, width);
        }
    }
    lines
}

fn count(n: &NodeDef) -> usize {
    n.children.iter().map(|c| 1 + count(c)).sum()
}

/// Small deterministic hash → 0..1.
fn hash01(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Candidates for spatial navigation.
#[derive(Clone, Copy)]
enum Pool<'a> {
    All,
    /// Same navigation group only.
    Group(&'a str),
    /// Nodes entirely past the current one along the direction, in the same column.
    Beyond,
}

fn is_box(k: Kind) -> bool {
    matches!(k, Kind::Vbox | Kind::Hbox | Kind::List)
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
            data: std::rc::Rc::new(data),
            instances: Vec::new(),
            focus: None,
            focus2: None,
            dual: false,
            focus_age: 0.0,
            scroll: HashMap::new(),
            inputs: HashMap::new(),
            editing: None,
            edit_from: None,
            time: 0.0,
            leaving: None,
            mouse: None,
            mouse_down: false,
            mouse_was_down: false,
            mouse_seen: None,
            mouse_focus: None,
            wheel: 0.0,
            pending: Vec::new(),
            cur_quad: None,
            cur_rot: None,
            cur_top: false,
            cur_modal: false,
            modal_return: None,
            pages: HashMap::new(),
            follow2: false,
            size: (VIRTUAL_W, VIRTUAL_H),
            cur_extra: (0.0, 0.0),
            cur_root: false,
        }
    }

    /// Layout root size in virtual units (default 1280x720); see [`viewport`].
    pub fn set_size(&mut self, w: f32, h: f32) {
        self.size = (w.max(1.0), h.max(1.0));
    }

    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    pub fn background(&self) -> [f32; 4] {
        self.def.background.unwrap_or([0.0, 0.0, 0.0, 1.0])
    }

    pub fn set_data(&mut self, data: Value) {
        self.data = std::rc::Rc::new(data);
    }
    pub fn data(&self) -> &Value {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut Value {
        std::rc::Rc::make_mut(&mut self.data)
    }

    /// Enable a second, independent focus for player 2.
    pub fn set_dual(&mut self, dual: bool) {
        self.dual = dual;
        if !dual {
            self.focus2 = None;
        }
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
    pub fn set_focus2(&mut self, key: Option<&str>) {
        self.focus2 = key.map(str::to_string);
    }

    /// Rectangle of a laid-out instance (after the last tick), virtual pixels.
    pub fn rect_of(&self, key: &str) -> Option<Rect> {
        self.find(key).map(|i| i.rect)
    }

    /// `on_press` action of the focused node, if any.
    pub fn focused_action(&self) -> Option<String> {
        self.focus
            .as_deref()
            .and_then(|f| self.find(f))
            .and_then(|i| i.on_press.clone())
    }
    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }
    pub fn focused2(&self) -> Option<&str> {
        self.focus2.as_deref()
    }

    /// Restart enter animations.
    pub fn reset_time(&mut self) {
        self.time = 0.0;
        self.leaving = None;
    }

    /// Start the leave animation; [`left`](Self::left) turns true when it ends.
    pub fn leave(&mut self) {
        if self.leaving.is_none() {
            self.leaving = Some(0.0);
        }
    }
    pub fn leaving(&self) -> bool {
        self.leaving.is_some()
    }
    pub fn left(&self) -> bool {
        self.leaving
            .is_some_and(|t| t >= self.def.exit_time.unwrap_or(12.0))
    }

    /// Queue an event from the host (e.g. a key-capture result).
    pub fn push_event(&mut self, e: Event) {
        self.pending.push(e);
    }

    // ---------------------------------------------------------------- text input

    /// True while an input node takes keyboard characters.
    pub fn text_focused(&self) -> bool {
        self.editing.is_some()
    }

    /// Current text of an input (edit buffer, else its bound value).
    pub fn input_text(&self, id: &str) -> String {
        if let Some(t) = self.inputs.get(id) {
            return t.clone();
        }
        self.defs
            .iter()
            .find(|d| d.id.as_deref() == Some(id))
            .and_then(|d| d.value.as_ref())
            .and_then(|v| lookup(&self.data, bind_path(v)))
            .map(value_text)
            .unwrap_or_default()
    }

    /// Replace an input's text from the host.
    pub fn set_input(&mut self, id: &str, text: &str) {
        self.inputs.insert(id.to_string(), text.to_string());
    }

    /// Start editing the input with this id (as if the user pressed accept on it).
    pub fn edit(&mut self, id: &str) {
        if self.focus.as_deref() != Some(id) {
            self.edit_from = self.focus.clone();
        }
        self.editing = Some(id.to_string());
        self.focus = Some(id.to_string());
    }

    /// Stop taking characters (the text stays). A focus `edit()` moved onto an
    /// input that is not focusable goes back where it was.
    pub fn stop_edit(&mut self) {
        let Some(id) = self.editing.take() else {
            return;
        };
        let from = self.edit_from.take();
        let focusable = self.find(&id).is_some_and(|i| i.focusable);
        if self.focus.as_deref() == Some(id.as_str()) && !focusable {
            self.focus = from;
            self.focus_age = 0.0;
        }
    }

    /// Feed a key to the input being edited. Returns true when consumed.
    pub fn text_key(&mut self, key: TextKey) -> bool {
        let Some(id) = self.editing.clone() else {
            return false;
        };
        let (max_len, on_press, on_change) = self
            .defs
            .iter()
            .find(|d| d.id.as_deref() == Some(&id))
            .map(|d| {
                (
                    d.max_len.unwrap_or(32),
                    d.on_press.clone(),
                    d.on_change.clone(),
                )
            })
            .unwrap_or((32, None, None));
        let mut text = self.input_text(&id);
        let changed = match key {
            TextKey::Char(c) => {
                if !c.is_control() && text.chars().count() < max_len {
                    text.push(c);
                    true
                } else {
                    false
                }
            }
            TextKey::Backspace => text.pop().is_some(),
            TextKey::Enter => {
                self.stop_edit();
                if let Some(a) = on_press {
                    let a = interpolate(&a, self.data.as_ref());
                    if !a.is_empty() {
                        self.pending.push(Event::Action(a));
                    }
                }
                false
            }
            TextKey::Escape => {
                self.stop_edit();
                false
            }
        };
        if changed {
            self.inputs.insert(id.clone(), text.clone());
            self.pending.push(Event::Input(id, text.clone()));
            if let Some(a) = on_change {
                let a = interpolate(&a.replace("{value}", &text), self.data.as_ref());
                if !a.is_empty() {
                    self.pending.push(Event::Action(a));
                }
            }
        }
        true
    }

    // ---------------------------------------------------------------- layout

    fn layout(&mut self, p: &mut dyn Painter) {
        self.instances.clear();
        let root = Rect {
            x: 0.0,
            y: 0.0,
            w: self.size.0,
            h: self.size.1,
        };
        self.cur_extra = (
            (self.size.0 - VIRTUAL_W).max(0.0),
            (self.size.1 - VIRTUAL_H).max(0.0),
        );
        self.cur_root = true;
        // the def tree is borrowed out for the walk (no per-tick copy)
        let nodes = std::mem::take(&mut self.def.node);
        // paged grids publish their page for `{pages.<id>.page}` / `.count`
        if !self.pages.is_empty() && self.data.is_object() {
            let pages: serde_json::Map<String, Value> = self
                .pages
                .iter()
                .map(|(k, (p, n))| (k.clone(), serde_json::json!({ "page": p, "count": n })))
                .collect();
            let pages = Value::Object(pages);
            if self.data.get("pages") != Some(&pages)
                && let Value::Object(m) = std::rc::Rc::make_mut(&mut self.data)
            {
                m.insert("pages".into(), pages);
            }
        }
        self.cur_quad = None;
        self.cur_rot = None;
        // shared, not copied: the data only changes through `set_data` / `data_mut`
        let ctx = Ctx {
            root: self.data.clone(),
            item: None,
        };
        let mut cursor = 0usize;
        self.cur_modal = false;
        self.layout_children(&mut cursor, &nodes, root, &ctx, "", None, 0.0, p);
        self.def.node = nodes;
        self.apply_modal();
    }

    /// A modal subtree on screen: nothing else takes the focus, and the
    /// focus moves into it (and back out once it closes).
    fn apply_modal(&mut self) {
        let modal = self.instances.iter().any(|i| i.modal && i.focusable);
        if !modal {
            if let Some(back) = self.modal_return.take()
                && self.find(&back).is_some_and(|i| i.focusable)
            {
                self.focus = Some(back);
                self.focus_age = 0.0;
            }
            return;
        }
        for i in self.instances.iter_mut().filter(|i| !i.modal) {
            i.focusable = false;
        }
        let inside = self
            .focus
            .as_deref()
            .and_then(|f| self.find(f))
            .is_some_and(|i| i.modal && i.focusable);
        if !inside {
            if self.modal_return.is_none() {
                self.modal_return = self.focus.clone();
            }
            self.editing = None;
            self.focus = self
                .instances
                .iter()
                .find(|i| i.modal && i.focusable)
                .map(|i| i.key.clone());
            self.focus_age = 0.0;
        }
    }

    /// Lay out `nodes` (siblings) inside `parent`; `cursor` walks the flattened defs in preorder.
    #[allow(clippy::too_many_arguments)]
    fn layout_children(
        &mut self,
        cursor: &mut usize,
        nodes: &[NodeDef],
        parent: Rect,
        ctx: &Ctx,
        key_prefix: &str,
        clip: Option<Rect>,
        delay: f32,
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
            // ids are global unless the node sits inside an `items` instance
            let key = match &n.id {
                Some(id) if key_prefix.contains('[') => format!("{key_prefix}{id}"),
                Some(id) => id.clone(),
                None => format!("{key_prefix}#{def_idx}"),
            };
            // focus scopes: `when` and the `*_focus` sizes
            let scoped = n.when.is_some()
                || n.w_focus.is_some()
                || n.h_focus.is_some()
                || n.scale_focus.is_some();
            let lit = scoped
                && (self.scope_focused(self.focus.as_deref(), &key, key_prefix)
                    || self.ancestor_focused(&key)
                    || n.selected.as_ref().is_some_and(|v| visible(ctx, v)));
            let shown = match n.when.as_deref() {
                None => true,
                Some("focus") => lit,
                Some("focus2") => {
                    self.dual && self.scope_focused(self.focus2.as_deref(), &key, key_prefix)
                }
                Some(_) => !lit,
            };
            if !shown {
                *cursor += subtree;
                continue;
            }
            let sized;
            let n =
                if lit && (n.w_focus.is_some() || n.h_focus.is_some() || n.scale_focus.is_some()) {
                    let mut c = n.clone();
                    c.w = n.w_focus.clone().or(c.w);
                    c.h = n.h_focus.clone().or(c.h);
                    c.scale = n.scale_focus.or(c.scale);
                    sized = c;
                    &sized
                } else {
                    n
                };
            let (frame, extra, grow) = self.dock_frame(n, parent);
            let mut rect = Self::node_rect(n, frame, ctx, p);
            rect.w += grow.0;
            rect.h += grow.1;
            let before = self.instances.len();
            let saved = (self.cur_extra, self.cur_root);
            self.cur_extra = extra;
            self.cur_root = false;
            self.place(n, def_idx, key, rect, ctx, clip, delay, cursor, subtree, p);
            (self.cur_extra, self.cur_root) = saved;
            if lit && let Some(inst) = self.instances.get_mut(before) {
                inst.scope_lit = true;
            }
        }
    }

    /// The frame a node is placed in, the room its children may dock into and
    /// how much a `stretch` node grows. Top-level nodes default to the centred
    /// design frame; nested nodes keep their parent unless they name a `dock`.
    fn dock_frame(&self, n: &NodeDef, parent: Rect) -> (Rect, (f32, f32), (f32, f32)) {
        let (dh, dv) = n.dock.as_deref().map_or((None, None), dock_edges);
        let dh = dh.or(is_pct(&n.w).then_some(Edge::Fill));
        let dv = dv.or(is_pct(&n.h).then_some(Edge::Fill));
        let (dh, dv) = if self.cur_root {
            (dh.or(Some(Edge::Middle)), dv.or(Some(Edge::Middle)))
        } else {
            (dh, dv)
        };
        let (ex, ey) = self.cur_extra;
        let (x, w) = dh.map_or((0.0, parent.w), |e| frame_axis(e, parent.w, ex));
        let (y, h) = dv.map_or((0.0, parent.h), |e| frame_axis(e, parent.h, ey));
        let room = |e: Option<Edge>, r: f32| {
            if matches!(e, Some(Edge::Fill | Edge::Stretch)) {
                r
            } else {
                0.0
            }
        };
        let grows = |e: Option<Edge>, d: &Option<Dim>, r: f32| {
            if e == Some(Edge::Stretch) && !is_pct(d) {
                r
            } else {
                0.0
            }
        };
        (
            Rect {
                x: parent.x + x,
                y: parent.y + y,
                w,
                h,
            },
            (room(dh, ex), room(dv, ey)),
            (grows(dh, &n.w, ex), grows(dv, &n.h, ey)),
        )
    }

    /// The focus is on `key`, inside it, or (within an `items` instance) on a sibling.
    fn scope_focused(&self, focus: Option<&str>, key: &str, prefix: &str) -> bool {
        let Some(f) = focus else {
            return false;
        };
        if f == key || (f.starts_with(key) && f[key.len()..].starts_with('.')) {
            return true;
        }
        match prefix.rfind("].") {
            Some(end) => f.starts_with(&prefix[..end + 2]),
            None => false,
        }
    }

    /// Height of one page of a paged grid (whole rows that fit), virtual pixels.
    fn grid_page(n: &NodeDef, rect: Rect) -> f32 {
        let pad = n.pad.unwrap_or(0.0);
        let gap = n.gap.unwrap_or(0.0);
        let inner_h = rect.h - 2.0 * pad;
        let ch = n.cell_h.unwrap_or(inner_h).max(1.0);
        let rows = libm::floorf((inner_h + gap) / (ch + gap)).max(1.0);
        rows * (ch + gap)
    }

    fn node_rect(n: &NodeDef, parent: Rect, ctx: &Ctx, p: &mut dyn Painter) -> Rect {
        let (mut w, mut h) = (
            n.w.as_ref().map(|d| d.resolve(parent.w, ctx)),
            n.h.as_ref().map(|d| d.resolve(parent.h, ctx)),
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
            let pad = n.pad.unwrap_or(0.0);
            if w.is_none() {
                w = Some(p.text_width(font, &text, scale) + 2.0 * pad);
            }
            if h.is_none() {
                let lh = n.line_h.unwrap_or_else(|| p.text_height(font, scale));
                let lines = match (n.wrap, w) {
                    (true, Some(ww)) => wrap_text(p, font, scale, &text, ww - 2.0 * pad)
                        .len()
                        .min(n.max_lines.unwrap_or(usize::MAX)),
                    _ => 1,
                };
                h = Some(lh * lines.max(1) as f32);
            }
        } else if matches!(
            n.kind,
            Kind::Button | Kind::Input | Kind::Slider | Kind::Toggle
        ) && h.is_none()
        {
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
        if matches!(n.kind, Kind::Pips | Kind::Choice | Kind::Keycap) {
            let hh = *h.get_or_insert_with(|| {
                p.text_height(n.font.unwrap_or(1), n.scale.unwrap_or(1.0)) * 2.0
            });
            if let Some(nat) = widgets::natural_w(n, ctx, hh, p) {
                // a keycap's `w` is its minimum width
                w = Some(match (n.kind, w) {
                    (Kind::Keycap, Some(min)) => nat.max(min),
                    (_, Some(given)) => given,
                    (_, None) => nat,
                });
            }
        }
        if n.hug && n.items.is_none() && matches!(n.kind, Kind::Hbox | Kind::Vbox) {
            let vertical = n.kind == Kind::Vbox;
            if (vertical && h.is_none()) || (!vertical && w.is_none()) {
                let pad = n.pad.unwrap_or(0.0);
                let probe = Rect {
                    w: w.unwrap_or(parent.w) - 2.0 * pad,
                    h: h.unwrap_or(parent.h) - 2.0 * pad,
                    ..parent
                };
                let mut len = 0.0;
                let mut shown = 0usize;
                for c in n
                    .children
                    .iter()
                    .filter(|c| c.visible.as_ref().is_none_or(|v| visible(ctx, v)))
                {
                    let r = Self::node_rect(c, probe, ctx, p);
                    len += if vertical { r.h } else { r.w };
                    shown += 1;
                }
                len += n.gap.unwrap_or(0.0) * shown.saturating_sub(1) as f32 + 2.0 * pad;
                if vertical {
                    h = Some(len);
                } else {
                    w = Some(len);
                }
            }
        }
        let w = w.unwrap_or(parent.w);
        let h = h.unwrap_or(parent.h);
        let x = n.x.as_ref().map_or(0.0, |d| d.resolve(parent.w, ctx));
        let y = n.y.as_ref().map_or(0.0, |d| d.resolve(parent.h, ctx));
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

    /// Animation offset and alpha for a node: enter tween, or the exit tween while leaving.
    fn anim(&self, n: &NodeDef, delay: f32) -> ((f32, f32), f32) {
        let kind = n.ease.as_deref().unwrap_or("out");
        let (dir, t) = match self.leaving {
            Some(lt) => {
                let dur = n.exit_time.or(self.def.exit_time).unwrap_or(12.0).max(1.0);
                let dir = n.exit.clone().or_else(|| n.enter.clone());
                if dir.is_none() {
                    return ((0.0, 0.0), 1.0);
                }
                (dir, 1.0 - ease("in", lt / dur))
            }
            None => {
                let Some(e) = &n.enter else {
                    return ((0.0, 0.0), 1.0);
                };
                let dur = n.enter_time.unwrap_or(18.0).max(1.0);
                let t = ease(
                    kind,
                    (self.time - n.enter_delay.unwrap_or(0.0) - delay) / dur,
                );
                (Some(e.clone()), t)
            }
        };
        let d = 1.0 - t;
        let off = match dir.as_deref().unwrap_or("fade") {
            "left" => (-220.0 * d, 0.0),
            "right" => (220.0 * d, 0.0),
            "up" => (0.0, -160.0 * d),
            "down" => (0.0, 160.0 * d),
            _ => (0.0, 0.0),
        };
        (off, t.clamp(0.0, 1.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        n: &NodeDef,
        def_idx: usize,
        key: String,
        rect: Rect,
        ctx: &Ctx,
        clip: Option<Rect>,
        delay: f32,
        cursor: &mut usize,
        subtree: usize,
        p: &mut dyn Painter,
    ) {
        let ((ox, oy), _) = self.anim(n, delay);
        let rect = Rect {
            x: rect.x + ox,
            y: rect.y + oy,
            ..rect
        };
        let (saved_quad, saved_rot, saved_top) = (self.cur_quad, self.cur_rot, self.cur_top);
        let saved_modal = self.cur_modal;
        self.cur_top |= n.top;
        self.cur_modal |= n.modal;
        if let Some(c) = n.cut {
            self.cur_quad = Some(Quad::of(rect, c));
        }
        if let Some(a) = n.angle
            && a != 0.0
        {
            let (cx, cy) = rect.center();
            self.cur_rot = Some(Rot { deg: a, cx, cy });
        }
        self.place_inner(n, def_idx, key, rect, ctx, clip, delay, cursor, subtree, p);
        self.cur_quad = saved_quad;
        self.cur_rot = saved_rot;
        self.cur_top = saved_top;
        self.cur_modal = saved_modal;
    }

    #[allow(clippy::too_many_arguments)]
    fn place_inner(
        &mut self,
        n: &NodeDef,
        def_idx: usize,
        key: String,
        rect: Rect,
        ctx: &Ctx,
        clip: Option<Rect>,
        delay: f32,
        cursor: &mut usize,
        subtree: usize,
        p: &mut dyn Painter,
    ) {
        let default_focus = matches!(
            n.kind,
            Kind::Button | Kind::Input | Kind::Slider | Kind::Toggle
        );
        let scroller = matches!(n.kind, Kind::List | Kind::Grid);
        let child_clip = if scroller {
            let b = n.bleed.unwrap_or(0.0);
            let grown = Rect {
                x: rect.x - b,
                y: rect.y - b,
                w: rect.w + 2.0 * b,
                h: rect.h + 2.0 * b,
            };
            Some(clip.map_or(grown, |c| c.intersect(&grown)))
        } else {
            clip
        };
        let action = |a: &Option<String>| {
            a.as_deref()
                .map(|a| interpolate(a, ctx))
                .filter(|a| !a.is_empty())
        };
        let (on_press, on_left, on_right) =
            (action(&n.on_press), action(&n.on_left), action(&n.on_right));
        // on_change keeps `{value}` for later substitution
        let on_change = n.on_change.clone().filter(|a| !a.is_empty());
        let on_pick = n.on_pick.clone().filter(|a| !a.is_empty());
        let has_action = on_press.is_some()
            || on_left.is_some()
            || on_right.is_some()
            || on_change.is_some()
            || matches!(n.kind, Kind::Input | Kind::Slider);
        let focusable = n.focus.unwrap_or(default_focus) && has_action;
        self.instances.push(Instance {
            def: def_idx,
            key: key.clone(),
            rect,
            clip,
            text: n.text.as_deref().map(|t| interpolate(t, ctx)),
            on_press,
            on_left,
            on_right,
            on_change,
            on_pick,
            focusable,
            scroll_owner: None,
            delay,
            ctx: ctx.clone(),
            quad: self.cur_quad,
            rot: self.cur_rot,
            scope_lit: false,
            offpage: false,
            top: self.cur_top,
            modal: self.cur_modal,
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
        let stagger = n.stagger.unwrap_or(0.0);
        let scroll = if scroller {
            self.scroll.get(&key).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        let start_cursor = *cursor;
        let owner = if scroller {
            Some(key.clone())
        } else {
            self.instances[me].scroll_owner.clone()
        };

        // boxes without items: one slot per child, laid out along the main axis
        if is_box(n.kind) && n.items.is_none() {
            let vertical = matches!(n.kind, Kind::Vbox | Kind::List);
            let probe = Rect {
                x: inner.x,
                y: inner.y,
                w: inner.w,
                h: inner.h,
            };
            // measure fixed children first
            let mut sizes: Vec<f32> = Vec::new();
            let mut fixed = 0.0f32;
            let mut grows = 0usize;
            let mut shown = 0usize;
            for c in &n.children {
                if let Some(v) = &c.visible
                    && !visible(ctx, v)
                {
                    sizes.push(-1.0);
                    continue;
                }
                shown += 1;
                if c.grow {
                    grows += 1;
                    sizes.push(0.0);
                    continue;
                }
                let r = Self::node_rect(c, probe, ctx, p);
                let s = if vertical { r.h } else { r.w };
                sizes.push(s);
                fixed += s;
            }
            let main = if vertical { inner.h } else { inner.w };
            let total_gap = gap * shown.saturating_sub(1) as f32;
            let free = (main - fixed - total_gap).max(0.0);
            let grow_each = if grows > 0 { free / grows as f32 } else { 0.0 };
            let used = fixed + total_gap + grow_each * grows as f32;
            let (mut pen, between) = match n.justify.as_deref().unwrap_or("start") {
                "center" => ((main - used) / 2.0, 0.0),
                "end" => (main - used, 0.0),
                "between" if shown > 1 => (0.0, (main - used) / (shown - 1) as f32),
                _ => (0.0, 0.0),
            };
            let align = n.align.as_deref().unwrap_or("stretch");
            for (i, c) in n.children.iter().enumerate() {
                let child_delay = delay + stagger * i as f32;
                if sizes[i] < 0.0 {
                    // hidden: layout_children skips it but must advance the cursor
                    self.layout_children(
                        cursor,
                        std::slice::from_ref(c),
                        probe,
                        ctx,
                        &format!("{key}."),
                        child_clip,
                        child_delay,
                        p,
                    );
                    continue;
                }
                let extent = if c.grow { grow_each } else { sizes[i] };
                // cross-axis size: stretch fills, others keep the child's own size
                let cross_rect = Self::node_rect(c, probe, ctx, p);
                let cross_size = if vertical { cross_rect.w } else { cross_rect.h };
                let cross_full = if vertical { inner.w } else { inner.h };
                let (cross_off, cross_len) = match align {
                    "start" => (0.0, cross_size.min(cross_full)),
                    "center" => (((cross_full - cross_size) / 2.0).max(0.0), cross_size),
                    "end" => ((cross_full - cross_size).max(0.0), cross_size),
                    _ => (0.0, cross_full),
                };
                let slot = if vertical {
                    Rect {
                        x: inner.x + cross_off,
                        y: inner.y + pen - scroll,
                        w: cross_len,
                        h: extent,
                    }
                } else {
                    Rect {
                        x: inner.x + pen,
                        y: inner.y + cross_off,
                        w: extent,
                        h: cross_len,
                    }
                };
                let before = self.instances.len();
                self.layout_children(
                    cursor,
                    std::slice::from_ref(c),
                    slot,
                    ctx,
                    &format!("{key}."),
                    child_clip,
                    child_delay,
                    p,
                );
                for inst in &mut self.instances[before..] {
                    if inst.scroll_owner.is_none() {
                        inst.scroll_owner = owner.clone();
                    }
                }
                pen += extent + gap + between;
            }
            *cursor = start_cursor + subtree;
            if scroller {
                let content = pen - gap - between;
                let max = (content - inner.h).max(0.0);
                let cur = self.scroll.get(&key).copied().unwrap_or(0.0);
                self.scroll.insert(key, cur.clamp(0.0, max));
            }
            return;
        }

        // items: instance the children once per element (children overlap in the slot)
        // item contexts are built lazily: paged grids skip most of them
        let items: Option<&[Value]> =
            n.items
                .as_ref()
                .map(|path| match ctx.bound(bind_path(path)) {
                    Some(Value::Array(a)) => a.as_slice(),
                    _ => &[],
                });
        let item_count = items.map_or(1, <[Value]>::len);
        let item_ctx = |i: usize| -> (String, Ctx) {
            match items {
                Some(a) => (format!("{key}[{i}]."), ctx.with_item(a[i].clone(), i)),
                None => (format!("{key}."), ctx.clone()),
            }
        };
        let mut pen_x = inner.x;
        let mut pen_y = inner.y - scroll;
        let mut content_h = 0.0f32;
        let items_start = self.instances.len();
        let cols = n.columns.unwrap_or(1).max(1) as usize;
        let cw = n.cell_w.unwrap_or(inner.w / cols as f32);
        let ch = n.cell_h.unwrap_or(inner.h);
        // paged grids lay out the visible page plus one row on each side
        // (navigable, not drawn); the rest is skipped
        let page_rows = (n.kind == Kind::Grid && n.paged).then(|| {
            let per = libm::roundf(Self::grid_page(n, rect) / (ch + gap)).max(1.0) as usize;
            let first = libm::roundf(scroll / (ch + gap)).max(0.0) as usize;
            (first, per)
        });
        for i in 0..item_count {
            let offpage = match page_rows {
                Some((first, per)) => {
                    let row = i / cols;
                    let prefix = format!("{key}[{i}].");
                    let held = [self.focus.as_deref(), self.focus2.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|f| f.starts_with(prefix.as_str()));
                    if !held && (row + 1 < first || row > first + per) {
                        if n.kind == Kind::Grid {
                            content_h = ((i / cols) as f32 + 1.0) * (ch + gap) - gap;
                        }
                        continue;
                    }
                    row < first || row >= first + per
                }
                None => false,
            };
            let (prefix, c) = item_ctx(i);
            let (prefix, c) = (&prefix, &c);
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
            self.layout_children(
                cursor,
                &n.children,
                slot,
                c,
                prefix,
                child_clip,
                delay + stagger * i as f32,
                p,
            );
            let mut extent = 0.0f32;
            for inst in &mut self.instances[before..] {
                inst.offpage |= offpage;
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
            // an hbox of items centres each item across (`align = "center"`)
            if n.kind == Kind::Hbox && n.align.as_deref() == Some("center") {
                let insts = &mut self.instances[before..];
                let top = insts.iter().map(|i| i.rect.y).fold(f32::MAX, f32::min);
                let bottom = insts
                    .iter()
                    .map(|i| i.rect.y + i.rect.h)
                    .fold(f32::MIN, f32::max);
                if top <= bottom {
                    let dy = inner.y + (inner.h - (bottom - top)) / 2.0 - top;
                    for inst in insts {
                        inst.rect.y += dy;
                    }
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
        // `justify = "center" | "end"` moves a row / column of items as a whole
        if !scroller && item_count > 0 {
            let used = match n.kind {
                Kind::Hbox => pen_x - gap - inner.x,
                Kind::Vbox => pen_y - gap - inner.y,
                _ => -1.0,
            };
            let main = if n.kind == Kind::Hbox {
                inner.w
            } else {
                inner.h
            };
            let off = match n.justify.as_deref() {
                Some("end") if used >= 0.0 => (main - used).max(0.0),
                Some("center") if used >= 0.0 => ((main - used) / 2.0).max(0.0),
                _ => 0.0,
            };
            if off > 0.0 {
                for inst in &mut self.instances[items_start..] {
                    if n.kind == Kind::Hbox {
                        inst.rect.x += off;
                    } else {
                        inst.rect.y += off;
                    }
                }
            }
        }
        *cursor = start_cursor + subtree;
        if n.kind == Kind::Grid && n.paged {
            let page = Self::grid_page(n, rect);
            let rows = item_count.div_ceil(cols);
            let per_page = (libm::roundf(page / (ch + gap)).max(1.0)) as usize;
            let pages = rows.div_ceil(per_page).max(1);
            let max = (pages - 1) as f32 * page;
            let cur = self.scroll.get(&key).copied().unwrap_or(0.0);
            let cur = cur.clamp(0.0, max);
            if let Some(id) = &n.id {
                let at = libm::roundf(cur / page) as usize + 1;
                self.pages.insert(id.clone(), (at.min(pages), pages));
            }
            self.scroll.insert(key, cur);
        } else if scroller {
            let max = (content_h - inner.h).max(0.0);
            let cur = self.scroll.get(&key).copied().unwrap_or(0.0);
            self.scroll.insert(key, cur.clamp(0.0, max));
        }
    }

    // ---------------------------------------------------------------- input

    /// Advance one tick with the first player's navigation.
    pub fn tick(&mut self, nav: Nav, p: &mut dyn Painter) -> Vec<Event> {
        self.tick2(nav, Nav::default(), p)
    }

    /// Hit rectangle of an instance (the focused one grows by `hover_scale`).
    /// The `pips` / `choice` / `slider` part of focusable row `row` under
    /// `(mx, my)` (laid out after it, inside its rectangle).
    fn part_at(&self, row: &Instance, mx: f32, my: f32) -> Option<&Instance> {
        let start = self.instances.iter().position(|i| i.key == row.key)? + 1;
        let r = row.rect;
        self.instances[start..]
            .iter()
            .take_while(|i| {
                i.rect.x >= r.x - 0.5
                    && i.rect.y >= r.y - 0.5
                    && i.rect.x + i.rect.w <= r.x + r.w + 0.5
                    && i.rect.y + i.rect.h <= r.y + r.h + 0.5
            })
            .find(|i| {
                matches!(
                    self.defs[i.def].kind,
                    Kind::Pips | Kind::Choice | Kind::Slider
                ) && i.rect.contains(mx, my)
            })
    }

    fn hit_rect(&self, i: &Instance) -> Rect {
        let n = &self.defs[i.def];
        let focused = self.focus.as_deref() == Some(&i.key);
        match (focused, n.hover_scale) {
            (true, Some(hs)) => i.rect.scaled(hs),
            _ => i.rect,
        }
    }

    /// Advance one tick, apply both players' navigation and the mouse, return events.
    pub fn tick2(&mut self, nav: Nav, nav2: Nav, p: &mut dyn Painter) -> Vec<Event> {
        self.time += 1.0;
        if let Some(t) = self.leaving.as_mut() {
            *t += 1.0;
        }
        self.layout(p);
        let laid_out_for = (self.focus.clone(), self.focus2.clone());
        let mut events = std::mem::take(&mut self.pending);
        // a leaving screen ignores input
        if self.leaving.is_some() {
            self.wheel = 0.0;
            self.mouse_was_down = self.mouse_down;
            return events;
        }
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
                    let paged = self
                        .find(&k)
                        .map(|o| (&self.defs[o.def], o.rect))
                        .filter(|(d, _)| d.kind == Kind::Grid && d.paged)
                        .map(|(d, r)| Self::grid_page(d, r));
                    let next = match paged {
                        Some(page) => cur - self.wheel.signum() * page,
                        None => cur - self.wheel * 48.0,
                    };
                    self.scroll.insert(k, next.max(0.0));
                }
            }
            let hovered = self
                .instances
                .iter()
                .rev()
                .find(|i| {
                    i.focusable
                        && !i.offpage
                        && self.hit_rect(i).contains(mx, my)
                        && i.clip.is_none_or(|c| c.contains(mx, my))
                })
                .cloned();
            let click = self.mouse_down && !self.mouse_was_down;
            // the cursor has not moved since it focused an item that then
            // moved away from under it: it still points at that item
            let hovered = hovered.or_else(|| {
                let (key, at) = self.mouse_focus.as_ref()?;
                (*at == (mx, my) && self.focus.as_ref() == Some(key))
                    .then(|| self.find(key).cloned())
                    .flatten()
            });
            let moved = self.mouse_seen != Some((mx, my));
            // a click on a row's pips / choice / slider part picks under the
            // cursor (a slider part follows a drag too)
            let picked = hovered
                .as_ref()
                .filter(|_| self.mouse_down)
                .and_then(|h| Some((h, h.on_pick.as_ref()?)))
                .and_then(|(h, a)| {
                    let part = self.part_at(h, mx, my)?;
                    let slider = self.defs[part.def].kind == Kind::Slider;
                    if !(click || (slider && moved)) {
                        return None;
                    }
                    let v = widgets::pick_value(&self.defs[part.def], &part.ctx, part.rect, mx, p)?;
                    Some(interpolate(&a.replace("{value}", &v), &h.ctx))
                });
            if let Some(h) = hovered.filter(|_| moved || self.mouse_down) {
                if self.focus.as_deref() != Some(&h.key) {
                    self.focus = Some(h.key.clone());
                    self.focus_age = 0.0;
                    events.push(Event::Moved);
                }
                self.mouse_focus = Some((h.key.clone(), (mx, my)));
                let kind = self.defs[h.def].kind;
                match kind {
                    Kind::Slider if self.mouse_down => {
                        let frac = ((mx - h.rect.x) / h.rect.w.max(1.0)).clamp(0.0, 1.0);
                        if let Some(a) = self.slider_change(&h, frac) {
                            events.push(Event::Action(a));
                        }
                    }
                    Kind::Input if click => {
                        self.editing = Some(h.key.clone());
                    }
                    _ if picked.is_some() => {
                        events.extend(picked.map(Event::Action));
                    }
                    _ if click && h.on_pick.is_some() && self.part_at(&h, mx, my).is_some() => {}
                    _ if click => {
                        if let Some(a) = h.on_press.clone() {
                            events.push(Event::Action(a));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.wheel = 0.0;
        self.mouse_was_down = self.mouse_down;
        self.mouse_seen = self.mouse;

        if self.focus.as_deref().is_none_or(|f| self.find(f).is_none()) {
            self.focus = self
                .instances
                .iter()
                .find(|i| i.focusable && !i.offpage)
                .map(|i| i.key.clone());
            self.focus_age = 0.0;
        }
        if self
            .editing
            .as_deref()
            .is_some_and(|e| self.find(e).is_none())
        {
            self.editing = None;
        }
        // keyboard navigation is frozen while an input takes characters
        let nav = if self.editing.is_some() {
            Nav::default()
        } else {
            nav
        };
        let before = self.focus.clone();
        if let Some(mut ev) = self.navigate(nav, false) {
            events.append(&mut ev);
        }
        if self.focus != before {
            self.focus_age = 0.0;
            self.follow2 = false;
        } else {
            self.focus_age += 1.0;
        }
        if self.dual {
            if self
                .focus2
                .as_deref()
                .is_none_or(|f| self.find(f).is_none())
            {
                self.focus2 = self
                    .instances
                    .iter()
                    .filter(|i| i.focusable && !i.offpage)
                    .nth(1)
                    .or_else(|| self.instances.iter().find(|i| i.focusable))
                    .map(|i| i.key.clone());
            }
            let before2 = self.focus2.clone();
            if let Some(mut ev) = self.navigate(nav2, true) {
                events.append(&mut ev);
            }
            if self.focus2 != before2 && before2.is_some() {
                self.follow2 = true;
            }
        }
        if nav.back {
            events.push(Event::Back);
        }
        // keep the focused item (of whoever moved last) visible inside its scroller
        let follow = if self.follow2 {
            self.focus2.clone()
        } else {
            self.focus.clone()
        };
        if let Some(i) = follow.and_then(|f| self.find(&f).cloned())
            && let Some(owner) = i.scroll_owner.clone()
            && let Some(o) = self.find(&owner).cloned()
        {
            let s = self.scroll.get(&owner).copied().unwrap_or(0.0);
            let top = i.rect.y - o.rect.y;
            let bottom = top + i.rect.h;
            let od = &self.defs[o.def];
            if od.kind == Kind::Grid && od.paged {
                // snap to the page holding the focused cell
                let page = Self::grid_page(od, o.rect);
                let content_top = i.rect.y - (o.rect.y + od.pad.unwrap_or(0.0)) + s;
                let want = libm::floorf((content_top + 0.5) / page) * page;
                self.scroll.insert(owner, want.max(0.0));
            } else if top < 0.0 {
                self.scroll.insert(owner, (s + top).max(0.0));
            } else if bottom > o.rect.h {
                self.scroll.insert(owner, s + bottom - o.rect.h);
            }
        }
        // focus-scoped nodes (`when`, `*_focus` sizes) follow the new focus this frame
        if (self.focus.clone(), self.focus2.clone()) != laid_out_for {
            self.layout(p);
        }
        events
    }

    /// Apply one player's navigation to its focus.
    fn navigate(&mut self, nav: Nav, second: bool) -> Option<Vec<Event>> {
        let focus = if second {
            self.focus2.clone()
        } else {
            self.focus.clone()
        };
        let ci = focus.and_then(|f| self.find(&f).cloned())?;
        let mut events = Vec::new();
        let wrap = |a: String| {
            if second {
                Event::Action2(a)
            } else {
                Event::Action(a)
            }
        };
        let moved = if second { Event::Moved2 } else { Event::Moved };
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
        let kind = self.defs[ci.def].kind;
        if nav.left && ci.on_left.is_some() {
            events.push(wrap(ci.on_left.clone().unwrap_or_default()));
        } else if nav.right && ci.on_right.is_some() {
            events.push(wrap(ci.on_right.clone().unwrap_or_default()));
        } else if kind == Kind::Slider && (nav.left || nav.right) {
            let n = &self.defs[ci.def];
            let (min, max, step) = (
                n.min.unwrap_or(0.0),
                n.max.unwrap_or(1.0),
                n.step.unwrap_or(0.1),
            );
            let cur = self.slider_value(&ci);
            let next = (cur + if nav.left { -step } else { step }).clamp(min, max);
            let frac = if max > min {
                (next - min) / (max - min)
            } else {
                0.0
            };
            if let Some(a) = self.slider_change(&ci, frac) {
                events.push(wrap(a));
            }
        } else if let Some((dx, dy)) = dir
            && let Some(next) = self.nearest(&ci, dx, dy)
        {
            if second {
                self.focus2 = Some(next);
            } else {
                self.focus = Some(next);
            }
            events.push(moved);
        }
        if nav.accept {
            if kind == Kind::Input && !second {
                self.editing = Some(ci.key.clone());
            } else if let Some(a) = ci.on_press.clone() {
                events.push(wrap(a));
            }
        }
        Some(events)
    }

    fn slider_value(&self, i: &Instance) -> f32 {
        let n = &self.defs[i.def];
        n.value
            .as_ref()
            .map(|v| value_num(i.ctx.bound(bind_path(v))))
            .unwrap_or(0.0)
    }

    /// `on_change` action for a slider set to `frac` of its range, snapped to `step`.
    fn slider_change(&self, i: &Instance, frac: f32) -> Option<String> {
        let n = &self.defs[i.def];
        let (min, max, step) = (
            n.min.unwrap_or(0.0),
            n.max.unwrap_or(1.0),
            n.step.unwrap_or(0.0),
        );
        let mut v = min + frac * (max - min);
        if step > 0.0 {
            v = min + libm::floorf((v - min) / step + 0.5) * step;
        }
        let v = v.clamp(min.min(max), max.max(min));
        let text = if step > 0.0 && step.fract() == 0.0 && min.fract() == 0.0 {
            format!("{}", v as i64)
        } else {
            format!("{v:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        };
        let a = i.on_change.as_deref()?;
        let a = interpolate(&a.replace("{value}", &text), &i.ctx);
        (!a.is_empty()).then_some(a)
    }

    fn find(&self, key: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.key == key)
    }

    /// True when `key` sits inside the focused node (`<focus>.child`, `<focus>[i]...`).
    fn ancestor_focused(&self, key: &str) -> bool {
        self.focus.as_deref().is_some_and(|f| {
            key.len() > f.len()
                && key.starts_with(f)
                && matches!(key.as_bytes()[f.len()], b'.' | b'[')
        })
    }

    /// Navigation group of an instance: its own `group`, else the nearest ancestor's.
    fn group_of(&self, inst: &Instance) -> Option<&str> {
        if let Some(g) = self.defs[inst.def].group.as_deref() {
            return Some(g);
        }
        self.instances
            .iter()
            .filter(|a| {
                a.key.len() < inst.key.len()
                    && inst.key.starts_with(&a.key)
                    && matches!(inst.key.as_bytes()[a.key.len()], b'.' | b'[')
                    && self.defs[a.def].group.is_some()
            })
            .max_by_key(|a| a.key.len())
            .and_then(|a| self.defs[a.def].group.as_deref())
    }

    /// Spatial navigation: nearest focusable instance in direction `(dx, dy)`.
    /// Up/down prefer the focused node's `group` and only leave it when it has nothing there.
    fn nearest(&self, from: &Instance, dx: f32, dy: f32) -> Option<String> {
        if dy != 0.0
            && let Some(g) = self.group_of(from)
        {
            // leave the group only for nodes clearly beyond this one
            return self
                .nearest_in(from, dx, dy, Pool::Group(g))
                .or_else(|| self.nearest_in(from, dx, dy, Pool::Beyond));
        }
        self.nearest_in(from, dx, dy, Pool::All)
    }

    fn nearest_in(&self, from: &Instance, dx: f32, dy: f32, pool: Pool<'_>) -> Option<String> {
        let (fx, fy) = from.rect.center();
        let mut best: Option<(f32, &Instance)> = None;
        for i in self.instances.iter().filter(|i| {
            i.focusable
                && i.key != from.key
                && match pool {
                    Pool::All => true,
                    Pool::Beyond => {
                        let gap = (from.rect.h + i.rect.h) / 2.0;
                        let overlap = i.rect.x < from.rect.x + from.rect.w
                            && from.rect.x < i.rect.x + i.rect.w;
                        overlap && (i.rect.center().1 - from.rect.center().1) * dy >= gap
                    }
                    Pool::Group(g) => self.group_of(i) == Some(g),
                }
        }) {
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
        let mut tip: Option<(String, Rect)> = None;
        let order = instances
            .iter()
            .filter(|i| !i.top)
            .chain(instances.iter().filter(|i| i.top));
        for inst in order {
            if inst.offpage {
                continue;
            }
            // fully outside the scroller: skip (also keeps off-screen thumbnails unloaded)
            if let Some(c) = inst.clip
                && (inst.rect.x + inst.rect.w <= c.x
                    || inst.rect.y + inst.rect.h <= c.y
                    || inst.rect.x >= c.x + c.w
                    || inst.rect.y >= c.y + c.h)
            {
                continue;
            }
            p.set_transform(inst.rot);
            match inst.quad {
                Some(quad) => {
                    let mut cut = CutPainter { inner: p, quad };
                    self.draw_one(&mut cut, inst, &mut tip);
                }
                None => self.draw_one(p, inst, &mut tip),
            }
        }
        p.set_transform(None);
        if let Some((text, r)) = tip {
            let (font, scale) = (1, 2.0);
            let w = p.text_width(font, &text, scale) + 16.0;
            let h = p.text_height(font, scale) + 10.0;
            let mut x = r.x + r.w / 2.0 - w / 2.0;
            x = x.clamp(0.0, (self.size.0 - w).max(0.0));
            let y = if r.y + r.h + h + 6.0 > self.size.1 {
                r.y - h - 6.0
            } else {
                r.y + r.h + 6.0
            };
            let bg = Rect { x, y, w, h };
            p.draw_rect(bg, [0.0, 0.0, 0.0, 0.85], 0.0, None);
            p.draw_rect(
                Rect {
                    x: bg.x,
                    y: bg.y + bg.h - 1.0,
                    w: bg.w,
                    h: 1.0,
                },
                [0.9, 0.2, 0.25, 1.0],
                0.0,
                None,
            );
            let th = p.text_height(font, scale);
            let ink = p.text_center_dy(font, scale);
            p.draw_text(
                font,
                &text,
                bg.x + bg.w / 2.0,
                bg.y + (bg.h + th) / 2.0 - ink,
                0,
                0,
                scale,
                1.0,
                None,
            );
        }
        self.instances = instances;
    }

    /// `src` texels and destination of an image drawn with the node's `fit`.
    fn fit_image(
        p: &mut dyn Painter,
        key: &str,
        n: &NodeDef,
        rect: Rect,
    ) -> (Option<[u32; 4]>, Rect) {
        let mode = n.fit.as_deref().unwrap_or("stretch");
        if mode == "stretch" {
            return (None, rect);
        }
        let Some((iw, ih)) = p.image_size(key) else {
            return (None, rect);
        };
        if iw == 0 || ih == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
            return (None, rect);
        }
        let (fx, fy) = (n.fit_x.unwrap_or(0.5), n.fit_y.unwrap_or(0.5));
        let (iwf, ihf) = (iw as f32, ih as f32);
        if mode == "cover" {
            let s = (rect.w / iwf).max(rect.h / ihf);
            let (sw, sh) = ((rect.w / s).min(iwf), (rect.h / s).min(ihf));
            let mut sx = (iwf - sw) * fx;
            if n.flip {
                // flipped images crop from the mirrored side
                sx = iwf - sw - sx;
            }
            let sy = (ihf - sh) * fy;
            let src = [
                sx.max(0.0) as u32,
                sy.max(0.0) as u32,
                (sw as u32).max(1),
                (sh as u32).max(1),
            ];
            (Some(src), rect)
        } else {
            let s = (rect.w / iwf).min(rect.h / ihf);
            let (w, h) = (iwf * s, ihf * s);
            // the slack is split on whole units so the picture sits square in its box
            (
                None,
                Rect {
                    x: rect.x + libm::roundf((rect.w - w) * fx),
                    y: rect.y + libm::roundf((rect.h - h) * fy),
                    w,
                    h,
                },
            )
        }
    }

    /// Draw one laid-out instance.
    fn draw_one(&self, p: &mut dyn Painter, inst: &Instance, tip: &mut Option<(String, Rect)>) {
        let n = &self.defs[inst.def];
        let focused = self.focus.as_deref() == Some(&inst.key);
        let focused2 = self.dual && self.focus2.as_deref() == Some(&inst.key);
        let selected = n.selected.as_ref().is_some_and(|s| visible(&inst.ctx, s));
        let lit = focused || selected;
        let mut rect = inst.rect;
        let (_, t) = self.anim(n, inst.delay);
        let alpha = n.alpha.unwrap_or(1.0) * t;
        if focused && let Some(hs) = n.hover_scale {
            rect = rect.scaled(hs);
        }
        let skew = n.skew.unwrap_or(0.0);
        let clip = inst.clip;
        if n.kind == Kind::Slot {
            p.draw_slot(n.id.as_deref().unwrap_or(""), rect, alpha, clip);
            return;
        }
        if n.kind == Kind::Particles {
            self.particles(p, n, rect, alpha, clip);
            return;
        }
        let inside = self.ancestor_focused(&inst.key);
        if let Some(period) = n.blink
            && period > 0.0
            && self.time % period >= period / 2.0
        {
            return;
        }
        let lit = lit || (n.inherit_focus && inside);
        if lit
            && n.kind != Kind::Choice
            && let Some([dx, dy]) = n.shadow_focus
        {
            let c = n.shadow_color.unwrap_or(widgets::INK);
            let r = Rect {
                x: rect.x + dx,
                y: rect.y + dy,
                ..rect
            };
            p.draw_rect(r, [c[0], c[1], c[2], c[3] * alpha], skew, clip);
        }
        match n.kind {
            Kind::Pips => {
                widgets::draw_pips(p, n, &inst.ctx, rect, alpha, clip);
                return;
            }
            Kind::Choice => {
                widgets::draw_choice(p, n, &inst.ctx, rect, lit, alpha, clip);
                return;
            }
            Kind::Keycap => {
                widgets::draw_keycap(
                    p,
                    n,
                    &inst.ctx,
                    inst.text.as_deref(),
                    rect,
                    lit,
                    alpha,
                    clip,
                );
                return;
            }
            Kind::Icon => {
                widgets::draw_icon(p, n, &inst.ctx, rect, lit, alpha, clip);
                return;
            }
            Kind::Slider
                if n.knob.is_some() || n.knob_color.is_some() || n.track_color.is_some() =>
            {
                let (min, max) = (n.min.unwrap_or(0.0), n.max.unwrap_or(1.0));
                let v = self.slider_value(inst);
                let frac = if max > min {
                    ((v - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                widgets::draw_slider(p, n, frac, rect, lit, alpha, clip);
                return;
            }
            _ => {}
        }
        let color = if lit {
            n.color_focus.or(n.color)
        } else {
            n.color
        };
        if let Some(c) = color {
            p.draw_rect(rect, [c[0], c[1], c[2], c[3] * alpha], skew, clip);
        }
        let on = n
            .value
            .as_ref()
            .is_some_and(|v| truthy(inst.ctx.bound(bind_path(v))));
        let image = if n.kind == Kind::Toggle && on && n.image_on.is_some() {
            n.image_on.as_ref()
        } else if focused2 && n.image_focus2.is_some() {
            n.image_focus2.as_ref()
        } else if lit {
            n.image_focus.as_ref().or(n.image.as_ref())
        } else {
            n.image.as_ref()
        };
        if let Some(img) = image {
            let key = interpolate(img, &inst.ctx);
            match n.nine {
                Some(b) => Self::nine_slice(p, &key, b, rect, alpha, clip),
                None if n.tile => {
                    widgets::draw_tiled(p, &key, rect, n.scale.unwrap_or(1.0), alpha, clip)
                }
                None => {
                    let fx = ImageFx {
                        skew,
                        alpha,
                        flip: n.flip,
                        grey: n.grey,
                    };
                    let (src, dst) = Self::fit_image(p, &key, n, rect);
                    p.draw_image_fx(&key, src, dst, &fx, clip);
                }
            }
        }
        if let Some(b) = n.border {
            let b = if lit { n.border_focus.unwrap_or(b) } else { b };
            let edge = if lit {
                n.border_color_focus.or(n.border_color)
            } else {
                n.border_color
            }
            .unwrap_or(widgets::INK);
            widgets::draw_border(p, rect, b, n.depth.unwrap_or(0.0), edge, alpha, clip);
        }
        if let Some(c) = n.outline {
            let t = n.outline_w.unwrap_or(3.0);
            let c = [c[0], c[1], c[2], c[3] * alpha];
            for r in [
                Rect { h: t, ..rect },
                Rect {
                    y: rect.y + rect.h - t,
                    h: t,
                    ..rect
                },
                Rect { w: t, ..rect },
                Rect {
                    x: rect.x + rect.w - t,
                    w: t,
                    ..rect
                },
            ] {
                p.draw_rect(r, c, 0.0, clip);
            }
        }
        // the second player's focus tints over the image
        if focused2 && let Some(c) = n.color_focus2 {
            p.draw_rect(rect, [c[0], c[1], c[2], c[3] * alpha], skew, clip);
        }
        if n.kind == Kind::Slider {
            let frac = {
                let (min, max) = (n.min.unwrap_or(0.0), n.max.unwrap_or(1.0));
                let v = self.slider_value(inst);
                if max > min {
                    ((v - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            let fill = n.color2.unwrap_or([0.85, 0.2, 0.25, 1.0]);
            let track_h = (rect.h * 0.3).max(2.0);
            let track = Rect {
                x: rect.x,
                y: rect.y + (rect.h - track_h) / 2.0,
                w: rect.w,
                h: track_h,
            };
            p.draw_rect(track, [0.0, 0.0, 0.0, 0.5 * alpha], 0.0, clip);
            p.draw_rect(
                Rect {
                    w: track.w * frac,
                    ..track
                },
                [fill[0], fill[1], fill[2], fill[3] * alpha],
                0.0,
                clip,
            );
            let knob = (rect.h * 0.6).max(4.0);
            p.draw_rect(
                Rect {
                    x: rect.x + track.w * frac - knob / 2.0,
                    y: rect.y + (rect.h - knob) / 2.0,
                    w: knob,
                    h: knob,
                },
                if lit {
                    [1.0, 1.0, 1.0, alpha]
                } else {
                    [0.85, 0.85, 0.9, alpha]
                },
                0.0,
                clip,
            );
        }
        if n.kind == Kind::Toggle && n.image.is_none() && n.image_on.is_none() {
            let fill = if on {
                n.color2.unwrap_or([0.3, 0.8, 0.4, 1.0])
            } else {
                [0.25, 0.25, 0.3, 1.0]
            };
            let knob = rect.h.max(4.0);
            p.draw_rect(
                rect,
                [fill[0], fill[1], fill[2], fill[3] * alpha],
                0.0,
                clip,
            );
            p.draw_rect(
                Rect {
                    x: if on { rect.x + rect.w - knob } else { rect.x },
                    y: rect.y,
                    w: knob,
                    h: knob,
                },
                [1.0, 1.0, 1.0, alpha],
                0.0,
                clip,
            );
        }
        // a placeholder reads as a hint, not as typed text
        let mut alpha = alpha;
        let text = match n.kind {
            Kind::Input => {
                let id = n.id.clone().unwrap_or_else(|| inst.key.clone());
                let mut t = self.input_text(&id);
                if n.mask.as_ref().is_some_and(|m| visible(&inst.ctx, m)) {
                    t = "*".repeat(t.chars().count());
                }
                let editing = self.editing.as_deref() == Some(&inst.key);
                if let Some(caret) = n.caret.filter(|_| editing) {
                    self.draw_caret_input(p, n, &t, rect, caret, alpha, clip);
                    return;
                }
                if editing && (self.time as i64 / 20) % 2 == 0 {
                    t.push('_');
                }
                if t.is_empty() && !editing {
                    alpha *= 0.45;
                    n.placeholder.clone()
                } else {
                    Some(t)
                }
            }
            Kind::Text | Kind::Button | Kind::Toggle | Kind::Slider => inst.text.clone(),
            _ => None,
        };
        if let Some(text) = text
            && !text.is_empty()
        {
            let font = n.font.unwrap_or(1);
            let scale = if lit || inst.scope_lit {
                n.scale_focus.or(n.scale)
            } else {
                n.scale
            }
            .unwrap_or(1.0);
            let bank = if focused2 && n.bank_focus2.is_some() {
                n.bank_focus2
            } else if lit {
                n.bank_focus.or(n.bank)
            } else {
                n.bank
            }
            .unwrap_or(0);
            let default_align = match n.kind {
                Kind::Button => "center",
                _ => "left",
            };
            let inset = n.pad.unwrap_or(0.0);
            let (align, x) = match n.align.as_deref().unwrap_or(default_align) {
                "right" => (-1, rect.x + rect.w - inset),
                "center" => (0, rect.x + rect.w / 2.0),
                _ => (1, rect.x + inset),
            };
            let th = p.text_height(font, scale);
            let ink = p.text_center_dy(font, scale);
            let y = rect.y + (rect.h + th) / 2.0 - ink + n.text_dy.unwrap_or(0.0);
            let text_clip = if (n.w.is_some() && n.kind == Kind::Text) || n.kind == Kind::Input {
                Some(clip.map_or(rect, |c| c.intersect(&rect)))
            } else {
                clip
            };
            if n.wrap && n.kind == Kind::Text && n.line_h.is_none() {
                let lines = widgets::wrap_lines(p, font, scale, &text, rect.w - inset);
                let lines = fit_lines(p, font, scale, lines, n, rect.w - inset);
                let step = th + n.line_gap.unwrap_or(th * 0.4);
                let total = step * lines.len() as f32 - (step - th);
                let top = rect.y + ((rect.h - total) / 2.0).max(0.0) - ink;
                for (i, line) in lines.iter().enumerate() {
                    p.draw_text(
                        font,
                        line,
                        x,
                        top + th + step * i as f32,
                        bank,
                        align,
                        scale,
                        alpha,
                        clip,
                    );
                }
                return;
            }
            if n.wrap && n.kind == Kind::Text {
                let lines = wrap_text(p, font, scale, &text, rect.w - 2.0 * inset);
                let lines = fit_lines(p, font, scale, lines, n, rect.w - 2.0 * inset);
                let lh = n.line_h.unwrap_or(th);
                let top = rect.y + (rect.h - lh * lines.len() as f32) / 2.0;
                for (i, line) in lines.iter().enumerate() {
                    let ly = top + lh * i as f32 + (lh + th) / 2.0 - ink;
                    p.draw_text(
                        font,
                        line,
                        x + skew / 2.0,
                        ly,
                        bank,
                        align,
                        scale,
                        alpha,
                        clip,
                    );
                }
                return;
            }
            let text = if n.ellipsis {
                ellipsize(p, font, scale, &text, rect.w - 2.0 * inset)
            } else {
                text
            };
            p.draw_text(
                font,
                &text,
                x + skew / 2.0,
                y,
                bank,
                align,
                scale,
                alpha,
                text_clip,
            );
        }
        if focused
            && self.focus_age > 30.0
            && self.leaving.is_none()
            && let Some(t) = &n.tip
        {
            let t = interpolate(t, &inst.ctx);
            if !t.is_empty() {
                *tip = Some((t, rect));
            }
        }
    }

    /// Deterministic drifting sparks: each particle's path is a function of its index and time.
    fn particles(
        &self,
        p: &mut dyn Painter,
        n: &NodeDef,
        rect: Rect,
        alpha: f32,
        clip: Option<Rect>,
    ) {
        let count = n.count.unwrap_or(40);
        let speed = n.speed.unwrap_or(0.4);
        let size = n.size.unwrap_or(2.0);
        let c1 = n.color.unwrap_or([1.0, 0.6, 0.2, 0.8]);
        let c2 = n.color2.unwrap_or(c1);
        for i in 0..count {
            let (h1, h2, h3, h4) = (
                hash01(i),
                hash01(i + 1000),
                hash01(i + 2000),
                hash01(i + 3000),
            );
            let period = rect.h + size;
            let rise = self.time * speed * (0.5 + h3);
            let y = rect.y + rect.h - ((h2 * period + rise) % period);
            let wobble = libm::sinf(self.time * 0.03 + h4 * std::f32::consts::TAU) * 8.0;
            let x = rect.x + h1 * rect.w + wobble;
            let flicker = 0.6 + 0.4 * libm::sinf(self.time * 0.1 + h1 * std::f32::consts::TAU);
            let mix = h4;
            let col = [
                c1[0] + (c2[0] - c1[0]) * mix,
                c1[1] + (c2[1] - c1[1]) * mix,
                c1[2] + (c2[2] - c1[2]) * mix,
                (c1[3] + (c2[3] - c1[3]) * mix) * alpha * flicker,
            ];
            let s = size * (0.5 + h3);
            let r = Rect {
                x: libm::floorf(x / s) * s,
                y: libm::floorf(y / s) * s,
                w: s,
                h: s,
            };
            if r.y < rect.y || r.y + r.h > rect.y + rect.h {
                continue;
            }
            p.draw_rect(r, col, 0.0, clip);
        }
    }

    /// An input being edited with a `caret`: its text (the end of it when
    /// it is wider than the box) and a blinking bar after it.
    #[allow(clippy::too_many_arguments)]
    fn draw_caret_input(
        &self,
        p: &mut dyn Painter,
        n: &NodeDef,
        text: &str,
        rect: Rect,
        caret: [f32; 4],
        alpha: f32,
        clip: Option<Rect>,
    ) {
        let font = n.font.unwrap_or(1);
        let scale = n.scale_focus.or(n.scale).unwrap_or(1.0);
        let bank = n.bank_focus.or(n.bank).unwrap_or(0);
        let inset = n.pad.unwrap_or(0.0);
        let th = p.text_height(font, scale);
        let bar_w = (2.0 * scale).max(2.0);
        let room = (rect.w - 2.0 * inset - bar_w - 2.0).max(0.0);
        // keep the end (where the caret is) in view
        let mut shown: &str = text;
        while !shown.is_empty() && p.text_width(font, shown, scale) > room {
            let mut it = shown.chars();
            it.next();
            shown = it.as_str();
        }
        let w = p.text_width(font, shown, scale);
        let (align, x, end) = match n.align.as_deref().unwrap_or("left") {
            "right" => (
                -1,
                rect.x + rect.w - inset - bar_w - 2.0,
                rect.x + rect.w - inset - bar_w,
            ),
            "center" => (
                0,
                rect.x + rect.w / 2.0 - bar_w / 2.0,
                rect.x + rect.w / 2.0 + w / 2.0,
            ),
            _ => (1, rect.x + inset, rect.x + inset + w + 2.0),
        };
        let ink = p.text_center_dy(font, scale);
        let y = rect.y + (rect.h + th) / 2.0 - ink + n.text_dy.unwrap_or(0.0);
        let inner = Some(clip.map_or(rect, |c| c.intersect(&rect)));
        if !shown.is_empty() {
            p.draw_text(font, shown, x, y, bank, align, scale, alpha, inner);
        }
        if (self.time as i64 / 20) % 2 == 0 {
            let bar_h = (th * 0.8).min(rect.h - 4.0).max(4.0);
            p.draw_rect(
                Rect {
                    x: end,
                    y: rect.y + (rect.h - bar_h) / 2.0,
                    w: bar_w,
                    h: bar_h,
                },
                [caret[0], caret[1], caret[2], caret[3] * alpha],
                0.0,
                inner,
            );
        }
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
        rects: Vec<Rect>,
    }
    impl TestPainter {
        fn new() -> Self {
            Self {
                drawn: Vec::new(),
                rects: Vec::new(),
            }
        }
    }
    impl Painter for TestPainter {
        fn image_size(&mut self, _: &str) -> Option<(u32, u32)> {
            Some((64, 32))
        }
        fn draw_image(
            &mut self,
            key: &str,
            _: Option<[u32; 4]>,
            dst: Rect,
            _: f32,
            _: f32,
            _: Option<Rect>,
        ) {
            self.drawn.push(format!("img:{key}"));
            self.rects.push(dst);
        }
        fn draw_rect(&mut self, dst: Rect, _: [f32; 4], _: f32, _: Option<Rect>) {
            self.drawn.push("rect".into());
            self.rects.push(dst);
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
            "profile": {"name": "Axel", "volume": 0.5, "sound": true}
        })
    }

    fn nav_down() -> Nav {
        Nav {
            down: true,
            ..Default::default()
        }
    }
    fn nav_accept() -> Nav {
        Nav {
            accept: true,
            ..Default::default()
        }
    }

    #[test]
    fn items_instance_children_and_binds() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.is_empty());
        ui.draw(&mut p);
        assert!(p.drawn.contains(&"text:ARCADE".to_string()));
        assert!(p.drawn.contains(&"text:Hello Axel".to_string()));
        let ev = ui.tick(nav_down(), &mut p);
        assert_eq!(ev, vec![Event::Moved]);
        let ev = ui.tick(nav_accept(), &mut p);
        assert_eq!(ev, vec![Event::Action("mode:versus".into())]);
    }

    #[test]
    fn mouse_hover_and_click() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        // third button spans y = 180 + 2 * (52 + 8) .. + 52
        ui.mouse_move(Some((100.0, 180.0 + 120.0 + 10.0)));
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Moved]);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("quit".into())]);
    }

    /// A click on a row's pips or slider part picks under the cursor; a
    /// slider part follows the drag.
    #[test]
    fn mouse_picks_on_row_parts() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "button"
id = "lvl"
x = 0
y = 0
w = 300
h = 30
on_press = "cycle"
on_pick = "pick:{id}:{value}"
  [[node.children]]
  kind = "pips"
  x = 10
  y = 5
  h = 20
  gap = 5
  max = 4
  value = "lvl"
[[node]]
kind = "button"
id = "vol"
x = 0
y = 40
w = 300
h = 30
on_press = "cycle"
on_pick = "pick:vol:{value}"
  [[node.children]]
  kind = "slider"
  x = 10
  y = 10
  w = 200
  h = 10
  min = 0
  max = 1
  value = "v"
  focus = false
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, serde_json::json!({ "lvl": 1, "v": 0.2, "id": "lvl" }));
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        // third pip: x = 10 + 2 * 25 .. + 20
        ui.mouse_move(Some((62.0, 12.0)));
        ui.tick(Nav::default(), &mut p);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("pick:lvl:3".into())]);
        ui.mouse_button(false);
        ui.tick(Nav::default(), &mut p);
        // off the part, a click is the row's own press
        ui.mouse_move(Some((250.0, 12.0)));
        ui.tick(Nav::default(), &mut p);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("cycle".into())]);
        ui.mouse_button(false);
        // the slider: press, then drag
        ui.mouse_move(Some((110.0, 55.0)));
        ui.tick(Nav::default(), &mut p);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("pick:vol:0.500".into())]);
        ui.mouse_move(Some((160.0, 55.0)));
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("pick:vol:0.750".into())]);
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.is_empty(), "held still: nothing new");
    }

    /// The focus the mouse gives can move the item away from under the
    /// cursor (a focused item that grows re-lays the screen out): a click
    /// without moving still presses it.
    #[test]
    fn click_follows_the_item_the_mouse_focused() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "button"
id = "a"
x = 10
y = 10
w = 100
h = 30
on_press = "a"
[[node]]
kind = "button"
id = "b"
x = 10
y = "{by}"
w = 100
h = 30
on_press = "b"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, serde_json::json!({ "by": 100 }));
        ui.set_focus(Some("a"));
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.mouse_move(Some((50.0, 110.0)));
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Moved]);
        assert_eq!(ui.focused(), Some("b"));
        // b moves away from the resting cursor
        ui.set_data(serde_json::json!({ "by": 200 }));
        ui.tick(Nav::default(), &mut p);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("b".into())]);
        ui.mouse_button(false);
        ui.tick(Nav::default(), &mut p);
        // once the keys move the focus, that spot means nothing
        ui.set_focus(Some("a"));
        ui.tick(Nav::default(), &mut p);
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn focus_shadow_follows_the_focus() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "vbox"
x = 10
y = 10
w = 100
gap = 20
  [[node.children]]
  kind = "button"
  id = "a"
  h = 30
  color = [1.0, 1.0, 1.0, 1.0]
  on_press = "a"
  shadow_focus = [6, 6]
  [[node.children]]
  kind = "button"
  id = "b"
  h = 30
  color = [1.0, 1.0, 1.0, 1.0]
  on_press = "b"
  shadow_focus = [6, 6]
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        ui.set_focus(Some("a"));
        ui.tick(Nav::default(), &mut TestPainter::new());
        assert_eq!(ui.focused(), Some("a"));
        let mut p = TestPainter::new();
        ui.draw(&mut p);
        let shadows: Vec<_> = p.rects.iter().filter(|r| r.x == 16.0).collect();
        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].y, 16.0);
        // hovering the second button moves the focus and its shadow
        ui.mouse_move(Some((50.0, 70.0)));
        ui.tick(Nav::default(), &mut TestPainter::new());
        assert_eq!(ui.focused(), Some("b"));
        let mut p = TestPainter::new();
        ui.draw(&mut p);
        let shadows: Vec<_> = p.rects.iter().filter(|r| r.x == 16.0).collect();
        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].y, 66.0);
    }

    #[test]
    fn still_mouse_does_not_take_focus_back() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        // the cursor rests on the third button, then the keyboard moves up
        ui.mouse_move(Some((100.0, 180.0 + 120.0 + 10.0)));
        ui.tick(Nav::default(), &mut p);
        let up = Nav {
            up: true,
            ..Default::default()
        };
        assert_eq!(ui.tick(up, &mut p), vec![Event::Moved]);
        let keyboard = ui.focused().map(str::to_string);
        for _ in 0..3 {
            assert!(ui.tick(Nav::default(), &mut p).is_empty());
        }
        assert_eq!(ui.focused().map(str::to_string), keyboard);
        // moving the mouse takes it again
        ui.mouse_move(Some((101.0, 180.0 + 120.0 + 10.0)));
        assert_eq!(ui.tick(Nav::default(), &mut p), vec![Event::Moved]);
    }

    /// Item scopes shadow `item` / `index` and fall back to the shared data.
    #[test]
    fn ctx_item_scope() {
        let root = Ctx::new(serde_json::json!({ "title": "T", "item": "outer", "list": [1] }));
        assert_eq!(interpolate("{item}", &root), "outer");
        let c = root.with_item(serde_json::json!({ "name": "kfm" }), 3);
        assert_eq!(interpolate("{title} {item.name} {index}", &c), "T kfm 3");
        assert_eq!(interpolate("{ item . name }", &c), "kfm");
        assert!(visible(&c, "list") && !visible(&c, "!item.name"));
        let inner = c.with_item(Value::from("x"), 0);
        assert_eq!(interpolate("{item}{index}", &inner), "x0");
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
        let mut p = TestPainter::new();
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
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        for _ in 0..3 {
            ui.tick(nav_down(), &mut p);
        }
        let ev = ui.tick(nav_accept(), &mut p);
        assert_eq!(ev, vec![Event::Action("pick:12".into())]);
        assert!(ui.scroll["roster"] > 0.0, "grid scrolled: {:?}", ui.scroll);
    }

    #[test]
    fn item_rows_justify_end_and_centre_across() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "hbox"
x = 0
y = 0
w = 300
h = 40
gap = 10
justify = "end"
align = "center"
items = "keys"
[[node.children]]
kind = "panel"
id = "k"
w = 50
h = 20
"#,
        )
        .unwrap();
        let keys = serde_json::json!({ "keys": [1, 2] });
        let mut ui = Ui::new(def, keys);
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        let first = ui
            .instances
            .iter()
            .find(|i| i.key.ends_with("[0].k"))
            .unwrap()
            .rect;
        let last = ui
            .instances
            .iter()
            .find(|i| i.key.ends_with("[1].k"))
            .unwrap()
            .rect;
        // 50 + 10 + 50 = 110 wide, flush right; 20 high in a 40 row
        assert_eq!((first.x, last.x + last.w), (190.0, 300.0));
        assert_eq!(first.y, 10.0);
    }

    #[test]
    fn boxes_stack_siblings_with_gap_grow_and_justify() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "vbox"
id = "col"
x = 0
y = 0
w = 200
h = 300
gap = 10
[[node.children]]
kind = "panel"
id = "a"
h = 40
[[node.children]]
kind = "panel"
id = "b"
grow = true
[[node.children]]
kind = "panel"
id = "c"
h = 50

[[node]]
kind = "hbox"
id = "row"
x = 0
y = 400
w = 400
h = 50
justify = "between"
align = "center"
[[node.children]]
kind = "panel"
id = "l"
w = 100
h = 20
[[node.children]]
kind = "panel"
id = "r"
w = 100
h = 20
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        let a = ui.rect_of("a").unwrap();
        let b = ui.rect_of("b").unwrap();
        let c = ui.rect_of("c").unwrap();
        assert_eq!((a.y, a.h), (0.0, 40.0));
        assert_eq!(b.y, 50.0);
        assert_eq!(b.h, 300.0 - 40.0 - 50.0 - 20.0);
        assert_eq!(c.y, 250.0);
        assert_eq!(a.w, 200.0, "stretch fills the cross axis");
        let l = ui.rect_of("l").unwrap();
        let r = ui.rect_of("r").unwrap();
        assert_eq!(l.x, 0.0);
        assert_eq!(r.x, 300.0);
        assert_eq!(l.y, 400.0 + 15.0, "centred on the cross axis");
    }

    #[test]
    fn dock_follows_the_viewport() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "panel"
id = "mid"
x = 600
y = 100
w = 80
h = 20
[[node]]
kind = "panel"
id = "left"
dock = "left top"
x = 40
y = 10
w = 80
h = 20
[[node]]
kind = "panel"
id = "right"
dock = "right bottom"
x = 1160
y = 690
w = 80
h = 20
[[node]]
kind = "panel"
id = "band"
dock = "bottom"
w = "100%"
y = 660
h = 60
[[node.children]]
kind = "panel"
id = "end"
dock = "right"
x = 1200
w = 80
h = 60
[[node]]
kind = "panel"
id = "wide"
dock = "stretch-x"
x = 360
y = 100
w = 880
h = 40
[[node.children]]
kind = "panel"
id = "inner"
dock = "stretch-x"
x = 20
w = 840
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.rect_of("mid").unwrap().x, 600.0, "16:9 is unchanged");
        assert_eq!(ui.rect_of("right").unwrap().x, 1160.0);
        let (vw, vh) = viewport((2520, 1080));
        assert_eq!((vw, vh), (1680.0, 720.0));
        ui.set_size(vw, vh);
        ui.tick(Nav::default(), &mut p);
        let r = |k: &str| ui.rect_of(k).unwrap();
        assert_eq!((r("mid").x, r("mid").y), (800.0, 100.0), "centred frame");
        assert_eq!(r("left").x, 40.0);
        assert_eq!(r("right").x, 1560.0);
        assert_eq!((r("band").x, r("band").w), (0.0, 1680.0));
        assert_eq!(r("end").x, 1600.0, "docks inside a filled parent");
        assert_eq!((r("wide").x, r("wide").w), (360.0, 1280.0));
        assert_eq!((r("inner").x, r("inner").w), (380.0, 1240.0));
        let (vw, vh) = viewport((1024, 768));
        assert_eq!((vw, vh), (1280.0, 960.0));
        ui.set_size(vw, vh);
        ui.tick(Nav::default(), &mut p);
        let r = |k: &str| ui.rect_of(k).unwrap();
        assert_eq!(r("mid").y, 220.0, "taller windows centre vertically");
        assert_eq!(r("left").y, 10.0);
        assert_eq!(r("right").y, 930.0);
        assert_eq!(r("band").y, 900.0);
        assert_eq!(window_to_virtual((1024, 768), 512.0, 384.0), (640.0, 480.0));
    }

    #[test]
    fn masked_inputs_draw_stars() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "input"
id = "pw"
x = 0
y = 0
w = 200
h = 40
mask = "!show"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, serde_json::json!({ "show": false }));
        ui.set_input("pw", "hunter2");
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        assert!(
            p.drawn.contains(&"text:*******".to_string()),
            "{:?}",
            p.drawn
        );
        assert!(!p.drawn.iter().any(|d| d.contains("hunter2")));
        assert_eq!(ui.input_text("pw"), "hunter2", "only the drawing is masked");
        ui.set_data(serde_json::json!({ "show": true }));
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        assert!(
            p.drawn.contains(&"text:hunter2".to_string()),
            "{:?}",
            p.drawn
        );
    }

    #[test]
    fn long_text_ends_in_an_ellipsis_and_wraps_stop_at_max_lines() {
        // the test painter's text is 8 px per character
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "text"
text = "{name}"
w = 80
ellipsis = true

[[node]]
kind = "text"
text = "{motd}"
y = 40
w = 80
wrap = true
max_lines = 2
"#,
        )
        .unwrap();
        let data = serde_json::json!({
            "name": "A VERY LONG SERVER NAME",
            "motd": "ONE TWO THREE FOUR FIVE SIX SEVEN",
        });
        let mut ui = Ui::new(def, data);
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        assert!(
            p.drawn.contains(&"text:A VERY...".to_string()),
            "{:?}",
            p.drawn
        );
        assert!(
            p.drawn.contains(&"text:ONE TWO".to_string()),
            "{:?}",
            p.drawn
        );
        assert!(
            p.drawn.contains(&"text:THREE...".to_string()),
            "{:?}",
            p.drawn
        );
        assert!(!p.drawn.iter().any(|d| d.contains("SEVEN")));
        // two lines tall, not four
        let r = ui.instances[1].rect;
        assert_eq!(r.h, 24.0);
    }

    #[test]
    fn caret_inputs_show_the_end_of_long_text_and_a_bar() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "input"
id = "name"
w = 60
h = 20
caret = [1.0, 0.0, 0.0, 1.0]
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        ui.set_input("name", "ABCDEFGHIJ");
        ui.edit("name");
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        // 60 px minus the bar fits 7 characters: the end stays visible
        assert!(
            p.drawn.contains(&"text:DEFGHIJ".to_string()),
            "{:?}",
            p.drawn
        );
        assert!(!p.drawn.iter().any(|d| d.contains('_')));
        assert!(p.drawn.contains(&"rect".to_string()), "caret bar");
    }

    #[test]
    fn a_modal_keeps_the_focus_until_it_closes() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "button"
id = "under"
y = 0
w = 100
h = 20
on_press = "under"

[[node]]
kind = "button"
id = "below"
y = 300
w = 100
h = 20
on_press = "below"

[[node]]
kind = "panel"
visible = "dialog"
modal = true
y = 100
w = 300
h = 100

  [[node.children]]
  kind = "button"
  id = "ok"
  w = 100
  h = 20
  on_press = "ok"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, serde_json::json!({ "dialog": false }));
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.focused(), Some("under"));
        ui.set_data(serde_json::json!({ "dialog": true }));
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.focused(), Some("ok"), "the dialog takes the focus");
        ui.tick(nav_down(), &mut p);
        ui.tick(nav_down(), &mut p);
        assert_eq!(ui.focused(), Some("ok"), "nothing behind it is reachable");
        let ev = ui.tick(nav_accept(), &mut p);
        assert!(ev.contains(&Event::Action("ok".into())));
        ui.set_data(serde_json::json!({ "dialog": false }));
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.focused(), Some("under"), "the focus comes back");
    }

    #[test]
    fn hug_boxes_size_to_their_children() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "hbox"
w = 400
h = 20
justify = "end"
gap = 10

  [[node.children]]
  kind = "hbox"
  hug = true
  gap = 4

    [[node.children.children]]
    kind = "text"
    text = "ESC"

    [[node.children.children]]
    kind = "text"
    text = "BACK"

  [[node.children]]
  kind = "hbox"
  hug = true
  visible = "never"

    [[node.children.children]]
    kind = "text"
    text = "HIDDEN"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        // ESC (24) + 4 + BACK (32) = 60 wide, pushed to the right edge
        let r = ui.instances[1].rect;
        assert_eq!((r.x, r.w), (340.0, 60.0));
        assert_eq!(ui.instances[3].rect.x, 368.0);
    }

    #[test]
    fn input_edits_and_emits() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "input"
id = "code"
x = 0
y = 0
w = 200
h = 40
max_len = 5
placeholder = "CODE"
on_change = "typed:{value}"
on_press = "join"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        assert!(!ui.text_focused());
        ui.tick(nav_accept(), &mut p);
        assert!(ui.text_focused(), "accept starts editing");
        // navigation keys are ignored while editing
        assert!(ui.tick(nav_down(), &mut p).is_empty());
        for c in "ab".chars() {
            ui.text_key(TextKey::Char(c));
        }
        ui.text_key(TextKey::Backspace);
        let ev = ui.tick(Nav::default(), &mut p);
        assert_eq!(
            ev,
            vec![
                Event::Input("code".into(), "a".into()),
                Event::Action("typed:a".into()),
                Event::Input("code".into(), "ab".into()),
                Event::Action("typed:ab".into()),
                Event::Input("code".into(), "a".into()),
                Event::Action("typed:a".into()),
            ]
        );
        for c in "bcdefg".chars() {
            ui.text_key(TextKey::Char(c));
        }
        assert_eq!(ui.input_text("code"), "abcde", "max_len caps the text");
        ui.text_key(TextKey::Enter);
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.contains(&Event::Action("join".into())));
        assert!(!ui.text_focused());
    }

    #[test]
    fn search_box_gives_the_focus_back() {
        // a hidden (focus = false) search input opened by a hotkey: ending the
        // edit puts the cursor back on the cell it left, arrows work again
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "button"
id = "a"
x = 0
y = 0
w = 50
h = 50
on_press = "a"

[[node]]
kind = "button"
id = "b"
x = 100
y = 0
w = 50
h = 50
on_press = "b"

[[node]]
kind = "input"
id = "search"
x = 0
y = 100
w = 200
h = 20
focus = false
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        let right = Nav {
            right: true,
            ..Nav::default()
        };
        ui.tick(right, &mut p);
        assert_eq!(ui.focused(), Some("b"));
        ui.edit("search");
        assert!(ui.text_focused());
        assert!(
            ui.tick(
                Nav {
                    left: true,
                    ..Nav::default()
                },
                &mut p
            )
            .is_empty()
        );
        ui.text_key(TextKey::Escape);
        assert!(!ui.text_focused());
        assert_eq!(ui.focused(), Some("b"), "the focus returns to the cell");
        ui.tick(
            Nav {
                left: true,
                ..Nav::default()
            },
            &mut p,
        );
        assert_eq!(ui.focused(), Some("a"), "arrows move again");
        // stop_edit (arrows leave the box) keeps the text
        ui.edit("search");
        ui.text_key(TextKey::Char('k'));
        ui.stop_edit();
        assert_eq!(ui.focused(), Some("a"));
        assert_eq!(ui.input_text("search"), "k");
    }

    #[test]
    fn slider_toggle_and_selected() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "slider"
id = "vol"
x = 0
y = 0
w = 200
h = 20
min = 0
max = 1
step = 0.1
value = "profile.volume"
on_change = "set:volume:{value}"
[[node]]
kind = "toggle"
id = "snd"
x = 0
y = 100
w = 60
h = 20
value = "profile.sound"
on_press = "toggle:sound"
[[node]]
kind = "button"
id = "tab"
x = 0
y = 200
w = 60
h = 20
text = "TAB"
color = [0,0,0,1]
color_focus = [1,1,1,1]
selected = "profile.sound"
on_press = "tab"
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        let ev = ui.tick(
            Nav {
                right: true,
                ..Default::default()
            },
            &mut p,
        );
        assert_eq!(ev, vec![Event::Action("set:volume:0.6".into())]);
        // mouse drag to the start of the track
        ui.mouse_move(Some((1.0, 10.0)));
        ui.mouse_button(true);
        let ev = ui.tick(Nav::default(), &mut p);
        assert!(ev.contains(&Event::Action("set:volume:0".into())), "{ev:?}");
        ui.mouse_button(false);
        ui.mouse_move(None);
        ui.draw(&mut p);
        assert!(p.drawn.contains(&"text:TAB".to_string()));
    }

    #[test]
    fn dual_focus_fires_second_player_actions() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        ui.set_dual(true);
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        assert_ne!(ui.focused(), ui.focused2(), "P2 starts on a different item");
        let ev = ui.tick2(Nav::default(), nav_down(), &mut p);
        assert_eq!(ev, vec![Event::Moved2]);
        let ev = ui.tick2(Nav::default(), nav_accept(), &mut p);
        assert_eq!(ev, vec![Event::Action2("quit".into())]);
        let ev = ui.tick2(nav_accept(), Nav::default(), &mut p);
        assert_eq!(ev, vec![Event::Action("mode:arcade".into())]);
    }

    #[test]
    fn leave_animation_finishes() {
        let def = ScreenDef::parse(MENU).unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.leave();
        assert!(!ui.left());
        for _ in 0..12 {
            let ev = ui.tick(nav_accept(), &mut p);
            assert!(ev.is_empty(), "input ignored while leaving");
        }
        assert!(ui.left());
    }

    #[test]
    fn particles_draw_inside_their_rect() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "particles"
x = 100
y = 100
w = 200
h = 100
count = 30
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, data());
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        assert!(!p.rects.is_empty());
        for r in &p.rects {
            assert!(r.x >= 80.0 && r.x <= 320.0 && r.y >= 100.0 && r.y + r.h <= 200.0);
        }
    }

    #[test]
    fn widgets_size_themselves_and_bind_positions() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "pips"
id = "pips"
h = 20
gap = 5
max_from = "row.max"
value = "row.on"
[[node]]
kind = "choice"
id = "choice"
y = 100
h = 30
pad = 10
options = "row.opts"
value = "row.sel"
[[node]]
kind = "keycap"
id = "cap"
y = 200
w = 60
h = 30
text = "{row.cap}"
[[node]]
kind = "icon"
id = "ico"
x = "{row.dx}"
y = "50%"
w = 14
h = 14
glyph = "up"
"#,
        )
        .unwrap();
        let data = serde_json::json!({ "row": { "max": 8, "on": 3, "opts": ["0", "1", "@inf"], "sel": 2, "cap": "ENTER", "dx": 42 } });
        let mut ui = Ui::new(def, data);
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.rect_of("pips").unwrap().w, 8.0 * 20.0 + 7.0 * 5.0);
        // "0" and "1" are 8 px wide + 2*10 pad; the glyph is 15 px + pad; borders overlap
        let c = ui.rect_of("choice").unwrap();
        assert_eq!(c.w, 28.0 + 28.0 + 35.0 - 2.0 * 3.0);
        // the keycap grows past its minimum width for long labels
        assert_eq!(ui.rect_of("cap").unwrap().w, 40.0 + 16.0 + 6.0);
        let i = ui.rect_of("ico").unwrap();
        assert_eq!((i.x, i.y), (42.0, 360.0));
        ui.draw(&mut p);
        assert!(p.drawn.contains(&"text:ENTER".to_string()));
        assert!(p.drawn.contains(&"text:1".to_string()));
    }

    #[test]
    fn groups_keep_vertical_navigation_and_when_follows_focus() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "vbox"
id = "tabs"
x = 0
y = 0
w = 200
h = 300
group = "tabs"
items = "tabs"
gap = 10
[[node.children]]
kind = "button"
id = "tab"
h = 40
on_press = "tab:{index}"
[[node]]
kind = "button"
id = "head"
x = 450
y = 0
w = 100
h = 40
on_press = "head"
[[node]]
kind = "vbox"
id = "rows"
x = 400
y = 100
w = 400
h = 300
group = "rows"
items = "rows"
[[node.children]]
kind = "button"
id = "row"
h = 40
on_press = "row:{index}"
  [[node.children.children]]
  kind = "text"
  text = "MARK{index}"
  when = "focus"
"#,
        )
        .unwrap();
        let data = serde_json::json!({ "tabs": [1, 2, 3], "rows": [1, 2] });
        let mut ui = Ui::new(def, data);
        let mut p = TestPainter::new();
        ui.tick(Nav::default(), &mut p);
        ui.set_focus(Some("rows[1].row"));
        // nothing below in the group and nothing focusable lower down: stays
        ui.tick(nav_down(), &mut p);
        assert_eq!(ui.focused(), Some("rows[1].row"));
        ui.tick(
            Nav {
                up: true,
                ..Default::default()
            },
            &mut p,
        );
        assert_eq!(ui.focused(), Some("rows[0].row"), "stays in the rows group");
        p.drawn.clear();
        ui.draw(&mut p);
        assert!(p.drawn.contains(&"text:MARK0".to_string()));
        assert!(!p.drawn.contains(&"text:MARK1".to_string()));
        // leaving the group: only a node above in the same column, never the tab column
        let up = Nav {
            up: true,
            ..Default::default()
        };
        ui.tick(up, &mut p);
        assert_eq!(ui.focused(), Some("head"));
    }
}

#[cfg(test)]
mod ink_tests {
    use super::*;

    struct Rec {
        clips: Vec<Option<Rect>>,
        dsts: Vec<Rect>,
        rot: Option<Rot>,
        rots: Vec<Option<Rot>>,
    }
    impl Painter for Rec {
        fn set_transform(&mut self, rot: Option<Rot>) {
            self.rot = rot;
        }
        fn image_size(&mut self, _: &str) -> Option<(u32, u32)> {
            Some((100, 200))
        }
        fn draw_image(
            &mut self,
            _: &str,
            _: Option<[u32; 4]>,
            d: Rect,
            _: f32,
            _: f32,
            c: Option<Rect>,
        ) {
            self.dsts.push(d);
            self.clips.push(c);
            self.rots.push(self.rot);
        }
        fn draw_rect(&mut self, d: Rect, _: [f32; 4], _: f32, c: Option<Rect>) {
            self.dsts.push(d);
            self.clips.push(c);
            self.rots.push(self.rot);
        }
        fn text_width(&mut self, _: i32, t: &str, s: f32) -> f32 {
            t.len() as f32 * 8.0 * s
        }
        fn text_height(&mut self, _: i32, s: f32) -> f32 {
            12.0 * s
        }
        #[allow(clippy::too_many_arguments)]
        fn draw_text(
            &mut self,
            _: i32,
            _: &str,
            _: f32,
            _: f32,
            _: i32,
            _: i32,
            _: f32,
            _: f32,
            _: Option<Rect>,
        ) {
        }
    }
    fn rec() -> Rec {
        Rec {
            clips: vec![],
            dsts: vec![],
            rot: None,
            rots: vec![],
        }
    }
    fn down() -> Nav {
        Nav {
            down: true,
            ..Default::default()
        }
    }

    #[test]
    fn paged_grid_snaps_to_pages_and_publishes_them() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "grid"
id = "cells"
w = 400
h = 200
columns = 4
cell_w = 100
cell_h = 100
paged = true
items = "cells"
[[node.children]]
kind = "button"
w = 90
h = 90
on_press = "pick:{index}"
"#,
        )
        .unwrap();
        let cells: Vec<Value> = (0..20).map(|i| serde_json::json!({ "i": i })).collect();
        let mut ui = Ui::new(def, serde_json::json!({ "cells": cells }));
        let mut p = rec();
        ui.tick(Nav::default(), &mut p);
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.data()["pages"]["cells"]["count"], 3);
        assert_eq!(ui.data()["pages"]["cells"]["page"], 1);
        ui.tick(down(), &mut p);
        assert_eq!(ui.scroll["cells"], 0.0, "row 2 is still on page 1");
        ui.tick(down(), &mut p);
        assert_eq!(ui.scroll["cells"], 200.0, "row 3 opens page 2");
        ui.tick(down(), &mut p);
        ui.tick(down(), &mut p);
        ui.tick(Nav::default(), &mut p);
        ui.tick(Nav::default(), &mut p);
        assert_eq!(ui.scroll["cells"], 400.0, "last page holds a single row");
        assert_eq!(ui.data()["pages"]["cells"]["page"], 3);
    }

    #[test]
    fn focus_scope_grows_the_item_and_shows_its_description() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "vbox"
id = "menu"
w = 400
gap = 10
items = "menu"
[[node.children]]
kind = "button"
id = "b"
w = 300
h = 40
w_focus = 360
h_focus = 60
text = "{item.label}"
on_press = "{item.action}"
[[node.children]]
kind = "text"
id = "desc"
y = 64
text = "{item.desc}"
when = "focus"
"#,
        )
        .unwrap();
        let data = serde_json::json!({ "menu": [
            {"label": "A", "action": "a", "desc": "first"},
            {"label": "B", "action": "b", "desc": "second"},
        ]});
        let mut ui = Ui::new(def, data);
        let mut p = rec();
        ui.tick(Nav::default(), &mut p);
        ui.tick(Nav::default(), &mut p);
        let a = ui.rect_of("menu[0].b").unwrap();
        let b = ui.rect_of("menu[1].b").unwrap();
        assert_eq!((a.w, a.h), (360.0, 60.0));
        assert_eq!((b.w, b.h), (300.0, 40.0));
        assert!(ui.rect_of("menu[0].desc").is_some());
        assert!(ui.rect_of("menu[1].desc").is_none());
        // description (y 64 + 12) pushes the next item down
        assert_eq!(b.y, 64.0 + 12.0 + 10.0);
    }

    #[test]
    fn cut_clips_in_slanted_strips_and_angle_rotates_the_subtree() {
        let def = ScreenDef::parse(
            r#"
[[node]]
kind = "panel"
x = 100
y = 100
w = 200
h = 100
angle = -3
cut = [0, 0, 0, 50]
color = [1, 1, 1, 1]
[[node.children]]
kind = "image"
image = "x"
w = 200
h = 100
fit = "contain"
fit_y = 1
"#,
        )
        .unwrap();
        let mut ui = Ui::new(def, Value::Null);
        let mut p = rec();
        ui.tick(Nav::default(), &mut p);
        ui.draw(&mut p);
        assert_eq!(p.clips.len(), 100, "50 strips per draw");
        let first = p.clips[0].unwrap();
        let last = p.clips[49].unwrap();
        assert!((first.x + first.w - 299.5).abs() < 0.6, "{first:?}");
        assert!((last.x + last.w - 250.5).abs() < 0.6, "{last:?}");
        // contain: 100x200 image in 200x100 -> 50x100, centred, on the bottom
        assert_eq!(
            p.dsts[50],
            Rect {
                x: 175.0,
                y: 100.0,
                w: 50.0,
                h: 100.0
            }
        );
        assert!(
            p.rots
                .iter()
                .all(|r| r.is_some_and(|r| r.deg == -3.0 && r.cx == 200.0))
        );
        assert_eq!(p.rot, None, "transform reset after drawing");
    }
}
