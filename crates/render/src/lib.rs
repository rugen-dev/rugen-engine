//! wgpu renderer: 2D sprites in a logical coordinate space with camera zoom.
//! Indexed sprites stay 8-bit on the GPU and are resolved through a palette
//! texture, so palette swaps never touch pixel data.

#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no suitable GPU adapter")]
    NoAdapter,
    #[error("surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("surface not supported by adapter")]
    SurfaceConfig,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    screen: [f32; 2],
    center: [f32; 2],
    zoom: f32,
    _pad: f32,
    _pad2: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    fx_mul: [f32; 4],
    fx_add: [f32; 4],
    /// Texel rectangle of an atlas page sampled as if it were the whole
    /// texture (`[x, y, w, h]`, `w == 0` = the texture itself); see [`Draw::rect`].
    rect: [u16; 4],
}

/// No atlas rectangle: sample the texture itself.
const WHOLE: [u16; 4] = [0; 4];

/// How a draw combines with what is already on screen. Alphas are MUGEN
/// style 0..=255: `Add { src, dst }` gives `dst*dst/255 + src*src/255`,
/// `Sub` gives `dst*dst/255 - src*src/255`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Blend {
    #[default]
    Alpha,
    Add {
        src: u8,
        dst: u8,
    },
    Sub {
        src: u8,
        dst: u8,
    },
}

impl Blend {
    fn pipe_index(self) -> usize {
        match self {
            Blend::Alpha => 0,
            Blend::Add { .. } => 1,
            Blend::Sub { .. } => 2,
        }
    }
    fn src_scale(self) -> f32 {
        match self {
            Blend::Alpha => 1.0,
            Blend::Add { src, .. } | Blend::Sub { src, .. } => src as f32 / 255.0,
        }
    }
    /// Destination scale for the shader: -1 means ordinary alpha blending.
    fn dst_scale(self) -> f32 {
        match self {
            Blend::Alpha => -1.0,
            Blend::Add { dst, .. } | Blend::Sub { dst, .. } => dst as f32 / 255.0,
        }
    }
}

/// Palette effect (MUGEN PalFX) applied per draw, in 0..=1 units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub mul: [f32; 3],
    pub add: [f32; 3],
    /// 1 = full colour, 0 = greyscale.
    pub color: f32,
    pub invert: bool,
}

impl Default for Tint {
    fn default() -> Self {
        Self {
            mul: [1.0; 3],
            add: [0.0; 3],
            color: 1.0,
            invert: false,
        }
    }
}

impl Tint {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// Clip one quad (`[c0, c1, c2, c3]`, corners in order) to `r`: an
/// axis-aligned quad is trimmed with its texture coordinates, one wholly
/// outside collapses to nothing, a rotated one partly outside is kept whole.
fn clip_quad(q: &mut [Vertex], r: [f32; 4]) {
    let xs = q.iter().map(|v| v.pos[0]);
    let ys = q.iter().map(|v| v.pos[1]);
    let (x0, x1) = (
        xs.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
    );
    let (y0, y1) = (
        ys.clone().fold(f32::MAX, f32::min),
        ys.fold(f32::MIN, f32::max),
    );
    if x0 >= r[0] && x1 <= r[2] && y0 >= r[1] && y1 <= r[3] {
        return;
    }
    if x1 <= r[0] || x0 >= r[2] || y1 <= r[1] || y0 >= r[3] {
        for v in q.iter_mut() {
            v.pos = [r[0], r[1]];
        }
        return;
    }
    let (c0, c1, c3) = (q[0], q[1], q[3]);
    let aligned = c0.pos[1] == c1.pos[1] && c0.pos[0] == c3.pos[0] && c0.pos[0] < c1.pos[0];
    if !aligned || c3.pos[1] <= c0.pos[1] {
        return;
    }
    // u follows x, v follows y on an unrotated quad
    let (ua, ub) = (c0.uv[0], c1.uv[0]);
    let (va, vb) = (c0.uv[1], c3.uv[1]);
    let (px0, px1, py0, py1) = (c0.pos[0], c1.pos[0], c0.pos[1], c3.pos[1]);
    for v in q.iter_mut() {
        let x = v.pos[0].clamp(r[0], r[2]);
        let y = v.pos[1].clamp(r[1], r[3]);
        v.uv = [
            ua + (ub - ua) * (x - px0) / (px1 - px0),
            va + (vb - va) * (y - py0) / (py1 - py0),
        ];
        v.pos = [x, y];
    }
}

/// Quads the vertex / index buffers start with (a fight draws 60–150, a
/// text-heavy menu a few hundred); they double when a frame needs more.
const INITIAL_QUADS: u32 = 1024;

struct Run {
    tex: usize,
    pal: Option<usize>,
    blend: Blend,
    /// First quad of the run (4 vertices, 6 indices each).
    start: u32,
}

/// Vertices per quad in the vertex buffer (two triangles share two corners
/// through the index buffer).
const QUAD_VERTS: usize = 4;
const QUAD_INDICES: usize = 6;

/// Index pattern of quad `q`: two triangles over its four corners.
fn quad_indices(q: u32) -> [u32; QUAD_INDICES] {
    let b = q * QUAD_VERTS as u32;
    [b, b + 1, b + 2, b, b + 2, b + 3]
}

/// Camera over the logical space. `screen` is the logical viewport
/// (e.g. 320x240 for a classic char), `center` where it looks, `zoom` 1 = native.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub screen: [f32; 2],
    pub center: [f32; 2],
    pub zoom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaletteId(usize);

enum Kind {
    Indexed {
        view: wgpu::TextureView,
    },
    Rgba {
        bind: wgpu::BindGroup,
    },
    /// Freed slot, reused by the next upload; draws with it are skipped.
    Free,
}

struct Tex {
    kind: Kind,
    width: u32,
    height: u32,
}

/// One sprite draw. Positions are logical pixels; `pos` is where the sprite's
/// axis lands, `axis` the sprite's own axis offset (SFF `offset`).
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub tex: TextureId,
    pub pal: Option<PaletteId>,
    pub pos: [f32; 2],
    pub axis: [f32; 2],
    pub scale: [f32; 2],
    pub hflip: bool,
    pub vflip: bool,
    pub color: [f32; 4],
    pub blend: Blend,
    /// Rotation in degrees around `pos`, counter-clockwise on screen (MUGEN AngleDraw).
    pub angle: f32,
    pub tint: Tint,
    /// Texel sub-rectangle `[x, y, w, h]` to draw instead of the whole texture.
    pub src: Option<[u32; 4]>,
    /// Logical-space clip rectangle `[min x, min y, max x, max y]` (unrotated draws only).
    pub clip: Option<[f32; 4]>,
    /// Horizontal shear: the top edge is shifted right by `skew` logical pixels
    /// relative to the bottom edge (slanted UI panels).
    pub skew: f32,
    /// Apply `clip` before `angle` (clip in the draw's unrotated frame), so
    /// rotated UI subtrees keep their clipping.
    pub clip_local: bool,
    /// Draw this texel rectangle `[x, y, w, h]` of an indexed atlas page as
    /// if it were a texture of its own (a glyph of a font page): the quad
    /// takes its size, `uv` runs 0..1 over it and the shader clamps inside
    /// it, so the pixels match a standalone texture exactly while every
    /// glyph of a page shares one draw call. Unlike `src`, which trims `uv`.
    pub rect: Option<[u32; 4]>,
}

impl Draw {
    pub fn new(tex: TextureId, pal: Option<PaletteId>, pos: [f32; 2], axis: [f32; 2]) -> Self {
        Self {
            tex,
            pal,
            pos,
            axis,
            scale: [1.0, 1.0],
            hflip: false,
            vflip: false,
            color: [1.0; 4],
            blend: Blend::Alpha,
            angle: 0.0,
            tint: Tint::default(),
            src: None,
            clip: None,
            skew: 0.0,
            clip_local: false,
            rect: None,
        }
    }
}

/// A textured quad with free corners (MUGEN background elements: parallax
/// trapezoids, sheared floors). `corners` are top-left, top-right,
/// bottom-right, bottom-left in logical coordinates and take the whole
/// texture (`uv` = 0..1 across, top to bottom). `strips` > 1 cuts it into
/// horizontal bands so a trapezoid maps its texture row by row, as MUGEN's
/// raster parallax does, instead of bending along a diagonal.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub tex: TextureId,
    pub pal: Option<PaletteId>,
    pub corners: [[f32; 2]; 4],
    pub color: [f32; 4],
    pub tint: Tint,
    pub blend: Blend,
    /// `[min x, min y, max x, max y]`.
    pub clip: Option<[f32; 4]>,
    pub strips: u32,
}

/// Clip a convex polygon (positions with texture coordinates) to a
/// rectangle (Sutherland-Hodgman, coordinates interpolated along edges).
fn clip_polygon(pts: Vec<([f32; 2], [f32; 2])>, r: [f32; 4]) -> Vec<([f32; 2], [f32; 2])> {
    let mut out = pts;
    // (axis, bound, keep below the bound)
    for (axis, bound, below) in [
        (0, r[0], false),
        (0, r[2], true),
        (1, r[1], false),
        (1, r[3], true),
    ] {
        if out.is_empty() {
            break;
        }
        let inside = |p: &([f32; 2], [f32; 2])| {
            if below {
                p.0[axis] <= bound
            } else {
                p.0[axis] >= bound
            }
        };
        let cut = |a: &([f32; 2], [f32; 2]), b: &([f32; 2], [f32; 2])| {
            let t = (bound - a.0[axis]) / (b.0[axis] - a.0[axis]);
            let l = |x: f32, y: f32| x + (y - x) * t;
            (
                [l(a.0[0], b.0[0]), l(a.0[1], b.0[1])],
                [l(a.1[0], b.1[0]), l(a.1[1], b.1[1])],
            )
        };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (&input[i], &input[(i + 1) % input.len()]);
            match (inside(a), inside(b)) {
                (true, true) => out.push(*b),
                (true, false) => out.push(cut(a, b)),
                (false, true) => {
                    out.push(cut(a, b));
                    out.push(*b);
                }
                (false, false) => {}
            }
        }
    }
    out
}

pub struct Renderer {
    /// Swapchain image acquired ahead of drawing ([`Renderer::acquire`]).
    /// Declared before `surface`: fields drop in order, and a frame still in
    /// hand when the renderer goes (a screenshot run quitting) must go before
    /// its surface, or the GL backend panics on "Surface does not exist".
    acquired: Option<wgpu::SurfaceTexture>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    cam_buf: wgpu::Buffer,
    cam_bind: wgpu::BindGroup,
    indexed_layout: wgpu::BindGroupLayout,
    rgba_layout: wgpu::BindGroupLayout,
    /// Per blend mode (see [`Blend::pipe_index`]).
    indexed_pipes: [wgpu::RenderPipeline; 3],
    rgba_pipes: [wgpu::RenderPipeline; 3],
    sampler: wgpu::Sampler,
    textures: Vec<Tex>,
    /// Palette views; freed slots ([`Renderer::free_palette`]) are reused.
    palettes: Slots<wgpu::TextureView>,
    /// Per indexed texture, its (palette, bind group) pairs, built lazily
    /// (a fighter's sprite is drawn with one or two palettes), so freeing a
    /// texture drops its bind groups in one lookup.
    indexed_binds: std::collections::HashMap<usize, Vec<(usize, wgpu::BindGroup)>>,
    white: TextureId,
    verts: Vec<Vertex>,
    runs: Vec<Run>,
    /// The frame's vertices, kept from frame to frame and rewritten in place
    /// (grown when a frame needs more), instead of a new buffer per frame.
    vbuf: wgpu::Buffer,
    /// Quads `vbuf` has room for.
    vbuf_quads: u32,
    /// Two triangles per quad over `vbuf`, written once per growth.
    ibuf: wgpu::Buffer,
    ibuf_quads: u32,
    timing: FrameTiming,
    /// Texture uploads (count, bytes) since the last [`Renderer::take_stats`].
    uploads: std::cell::Cell<(u64, u64)>,
    /// Draw calls and quads of the last frame sent to the GPU.
    last_frame: (usize, usize),
    /// Freed texture slots ([`Renderer::free_texture`]).
    free_slots: Vec<usize>,
    /// The GPU this renderer draws with (replays record it).
    adapter: AdapterSummary,
    /// Largest texture side the device accepts (sprites beyond it are skipped).
    max_texture: u32,
}

/// The GPU adapter in words: its name, the wgpu backend and the driver.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterSummary {
    /// `AMD Radeon RX 7800 XT (RADV NAVI32)`
    pub name: String,
    /// `Vulkan`, `Metal`, `Dx12`, `Gl`
    pub backend: String,
    /// `radv Mesa 25.1.4` (driver and its info, when the backend says)
    pub driver: String,
}

/// Counters for frame profiling (`RUGEN_FRAMESTATS`).
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    /// Texture uploads since the last call.
    pub uploads: u64,
    pub upload_bytes: u64,
    /// Draw calls / quads of the last frame.
    pub draws: usize,
    pub quads: usize,
    /// Textures and palettes alive (freed ones not counted).
    pub textures: usize,
    pub palettes: usize,
}

/// A list whose ids stay valid until removed; removed slots are handed out
/// again (GPU resources that screens upload and release).
struct Slots<T> {
    items: Vec<Option<T>>,
    free: Vec<usize>,
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Slots<T> {
    fn insert(&mut self, v: T) -> usize {
        match self.free.pop() {
            Some(i) => {
                self.items[i] = Some(v);
                i
            }
            None => {
                self.items.push(Some(v));
                self.items.len() - 1
            }
        }
    }
    /// False when `i` was not alive.
    fn remove(&mut self, i: usize) -> bool {
        match self.items.get_mut(i).and_then(Option::take) {
            Some(_) => {
                self.free.push(i);
                true
            }
            None => false,
        }
    }
    fn get(&self, i: usize) -> Option<&T> {
        self.items.get(i).and_then(Option::as_ref)
    }
    fn live(&self) -> usize {
        self.items.len() - self.free.len()
    }
}

/// How frames reach the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Presentation {
    /// Lowest latency without tearing: mailbox when the driver offers it
    /// (the newest frame replaces a waiting one), else FIFO with at most one
    /// frame queued.
    #[default]
    LowLatency,
    /// The driver's default vsync (FIFO, up to two frames queued).
    Vsync,
}

/// Timing of the last presented frame (for latency measurements).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    /// Time spent waiting for a swapchain image (vsync back-pressure).
    pub acquire: std::time::Duration,
    /// When `present` returned.
    pub presented: Option<std::time::Instant>,
    /// When the swapchain image was acquired.
    pub acquired: Option<std::time::Instant>,
}

impl Renderer {
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, Error> {
        Self::with_presentation(target, width, height, Presentation::default()).await
    }

    pub async fn with_presentation(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        presentation: Presentation,
    ) -> Result<Self, Error> {
        // `WGPU_BACKEND=gl|vulkan|dx12|metal` picks the backend (old
        // integrated GPUs without a working Vulkan driver, and testing them)
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let surface = instance.create_surface(target)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|_| Error::NoAdapter)?;
        let info = adapter.get_info();
        let adapter_summary = AdapterSummary {
            name: info.name.trim().to_string(),
            backend: format!("{:?}", info.backend),
            driver: [info.driver.trim(), info.driver_info.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        };
        let caps = adapter.get_downlevel_capabilities();
        let adapter_limits = adapter.limits();
        log::info!(
            "render: adapter {:?} ({} {:?}, {}), max texture {}, downlevel {}",
            adapter_summary.name,
            adapter_summary.backend,
            info.device_type,
            adapter_summary.driver,
            adapter_limits.max_texture_dimension_2d,
            if caps.is_webgpu_compliant() {
                "no".to_string()
            } else {
                format!("yes (missing {:?})", !caps.flags)
            }
        );
        // The sprite pipeline needs nothing beyond the WebGL2-level
        // baseline: ask for that (raised to the adapter's texture sizes) so
        // a GL-only adapter (Intel HD 4000, old laptops) gets a device
        // instead of failing `Limits::default()`'s Vulkan-level numbers; a
        // stranger adapter falls back to exactly what it offers.
        let wanted =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits.clone());
        let descriptor = |limits| wgpu::DeviceDescriptor {
            label: Some("rugen"),
            required_limits: limits,
            ..Default::default()
        };
        let (device, queue) = match adapter.request_device(&descriptor(wanted)).await {
            Ok(d) => d,
            Err(e) => {
                log::warn!(
                    "render: device with WebGL2-level limits refused ({e}); asking for the adapter's own"
                );
                adapter
                    .request_device(&descriptor(adapter_limits.clone()))
                    .await?
            }
        };
        let max_texture = device.limits().max_texture_dimension_2d.max(1);
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or(Error::SurfaceConfig)?;
        let modes = surface.get_capabilities(&adapter).present_modes;
        match presentation {
            Presentation::LowLatency => {
                config.present_mode = if modes.contains(&wgpu::PresentMode::Mailbox) {
                    wgpu::PresentMode::Mailbox
                } else {
                    wgpu::PresentMode::Fifo
                };
                config.desired_maximum_frame_latency = 1;
            }
            Presentation::Vsync => config.present_mode = wgpu::PresentMode::AutoVsync,
        }
        // diagnostics: `RUGEN_PRESENT=fifo|mailbox|immediate`
        let forced = match std::env::var("RUGEN_PRESENT").as_deref() {
            Ok("fifo") => Some(wgpu::PresentMode::Fifo),
            Ok("mailbox") => Some(wgpu::PresentMode::Mailbox),
            Ok("immediate") => Some(wgpu::PresentMode::Immediate),
            _ => None,
        };
        if let Some(m) = forced.filter(|m| modes.contains(m)) {
            config.present_mode = m;
        }
        log::info!(
            "render: present mode {:?}, at most {} frame(s) queued (offered: {:?})",
            config.present_mode,
            config.desired_maximum_frame_latency,
            modes
        );
        // MUGEN blends in raw 8-bit space; keep the whole pipeline linear-in-bytes.
        config.format = config.format.remove_srgb_suffix();
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&CameraUniform {
                screen: [320.0, 240.0],
                center: [0.0, 0.0],
                zoom: 1.0,
                _pad: 0.0,
                _pad2: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cam_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &cam_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cam_buf.as_entire_binding(),
            }],
        });

        let tex_entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let indexed_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("indexed"),
            entries: &[
                tex_entry(0, wgpu::TextureSampleType::Uint),
                tex_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let rgba_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rgba"),
            entries: &[
                tex_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint16x4],
        };
        // The shader emits premultiplied colour and an alpha that already
        // encodes the destination scale, so Alpha and Add share one state.
        let blends = [
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::ReverseSubtract,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        ];
        let make_pipe = |label, bgl: &wgpu::BindGroupLayout, fs, blend| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[&cam_layout, bgl],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: std::slice::from_ref(&vertex_layout),
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            })
        };
        let indexed_pipes = blends.map(|b| make_pipe("indexed", &indexed_layout, "fs_indexed", b));
        let rgba_pipes = blends.map(|b| make_pipe("rgba", &rgba_layout, "fs_rgba", b));

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let (vbuf, ibuf) = Self::make_buffers(&device, &queue, INITIAL_QUADS);
        let mut r = Self {
            acquired: None,
            surface,
            device,
            queue,
            config,
            cam_buf,
            cam_bind,
            indexed_layout,
            rgba_layout,
            indexed_pipes,
            rgba_pipes,
            sampler,
            textures: Vec::new(),
            palettes: Slots::default(),
            indexed_binds: Default::default(),
            white: TextureId(0),
            verts: Vec::new(),
            runs: Vec::new(),
            vbuf,
            vbuf_quads: INITIAL_QUADS,
            ibuf,
            ibuf_quads: INITIAL_QUADS,
            timing: FrameTiming::default(),
            uploads: std::cell::Cell::new((0, 0)),
            last_frame: (0, 0),
            free_slots: Vec::new(),
            adapter: adapter_summary,
            max_texture,
        };
        r.white = r.add_rgba(1, 1, &[255, 255, 255, 255]);
        Ok(r)
    }

    /// Vertex and index buffers for `quads` quads, the indices filled in.
    fn make_buffers(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quads: u32,
    ) -> (wgpu::Buffer, wgpu::Buffer) {
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("verts"),
            size: quads as u64 * (QUAD_VERTS * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices: Vec<u32> = (0..quads).flat_map(quad_indices).collect();
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let _ = queue;
        (vbuf, ibuf)
    }

    /// The GPU adapter, backend and driver this renderer draws with.
    pub fn adapter(&self) -> &AdapterSummary {
        &self.adapter
    }

    /// Largest texture side (pixels) the device accepts.
    pub fn max_texture_size(&self) -> u32 {
        self.max_texture
    }

    /// True when a `w x h` texture fits the device; else say so once per
    /// size (the sprite is drawn as the white texture instead of failing
    /// device validation, which would end the process).
    fn fits(&self, w: u32, h: u32) -> bool {
        if w <= self.max_texture && h <= self.max_texture {
            return true;
        }
        log::error!(
            "render: a {w}x{h} sprite exceeds this GPU's {} px texture limit and is skipped",
            self.max_texture
        );
        false
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.acquired = None;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The present mode in use.
    pub fn present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }

    /// Frames the swapchain may queue.
    pub fn frame_latency(&self) -> u32 {
        self.config.desired_maximum_frame_latency
    }

    /// Timing of the last [`end`](Self::end).
    pub fn timing(&self) -> FrameTiming {
        self.timing
    }

    /// The swapchain applies vsync back-pressure on acquire (FIFO): the
    /// caller can acquire first, then simulate, then draw, so the newest
    /// state goes out right after the vblank that freed the image.
    pub fn acquire_blocks(&self) -> bool {
        matches!(
            self.config.present_mode,
            wgpu::PresentMode::Fifo | wgpu::PresentMode::FifoRelaxed | wgpu::PresentMode::AutoVsync
        )
    }

    /// Wait for the next swapchain image now (vsync); [`end`](Self::end)
    /// then draws into it without waiting.
    pub fn acquire(&mut self) -> Result<(), wgpu::SurfaceError> {
        if self.acquired.is_none() {
            let t = std::time::Instant::now();
            self.acquired = Some(self.surface.get_current_texture()?);
            self.timing.acquire = t.elapsed();
            self.timing.acquired = Some(std::time::Instant::now());
        }
        Ok(())
    }

    fn make_texture(
        &self,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        bpp: u32,
        data: &[u8],
    ) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if w > 0 && h > 0 && data.len() >= (w * h * bpp) as usize {
            let (n, b) = self.uploads.get();
            self.uploads.set((n + 1, b + (w * h * bpp) as u64));
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * bpp),
                    rows_per_image: Some(h),
                },
                size,
            );
        }
        tex
    }

    /// Upload 8-bit indices (`w*h` bytes).
    pub fn add_indexed(&mut self, w: u32, h: u32, indices: &[u8]) -> TextureId {
        if !self.fits(w, h) {
            return self.white;
        }
        let tex = self.make_texture(w, h, wgpu::TextureFormat::R8Uint, 1, indices);
        let view = tex.create_view(&Default::default());
        self.store(Tex {
            kind: Kind::Indexed { view },
            width: w,
            height: h,
        })
    }

    /// Keep a texture in a freed slot or a new one.
    fn store(&mut self, t: Tex) -> TextureId {
        match self.free_slots.pop() {
            Some(i) => {
                self.textures[i] = t;
                TextureId(i)
            }
            None => {
                self.textures.push(t);
                TextureId(self.textures.len() - 1)
            }
        }
    }

    /// Release a texture's GPU memory; its id may be handed out again by the
    /// next upload, so the caller must forget it.
    pub fn free_texture(&mut self, id: TextureId) {
        if id == self.white
            || matches!(
                self.textures.get(id.0).map(|t| &t.kind),
                None | Some(Kind::Free)
            )
        {
            return;
        }
        self.textures[id.0] = Tex {
            kind: Kind::Free,
            width: 1,
            height: 1,
        };
        self.indexed_binds.remove(&id.0);
        self.free_slots.push(id.0);
    }

    /// [`free_texture`](Self::free_texture) for many at once (a screen or a
    /// match going away).
    pub fn free_textures(&mut self, ids: impl IntoIterator<Item = TextureId>) {
        for id in ids {
            self.free_texture(id);
        }
    }

    /// Upload straight RGBA8 (`w*h*4` bytes).
    pub fn add_rgba(&mut self, w: u32, h: u32, rgba: &[u8]) -> TextureId {
        if !self.fits(w, h) {
            return self.white;
        }
        let tex = self.make_texture(w, h, wgpu::TextureFormat::Rgba8Unorm, 4, rgba);
        let view = tex.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.rgba_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.store(Tex {
            kind: Kind::Rgba { bind },
            width: w,
            height: h,
        })
    }

    /// Upload a palette (up to 256 RGBA entries; shorter ones are zero-padded).
    pub fn add_palette(&mut self, colors: &[[u8; 4]]) -> PaletteId {
        let mut data = vec![0u8; 256 * 4];
        for (i, c) in colors.iter().take(256).enumerate() {
            data[i * 4..i * 4 + 4].copy_from_slice(c);
        }
        let tex = self.make_texture(256, 1, wgpu::TextureFormat::Rgba8Unorm, 4, &data);
        PaletteId(self.palettes.insert(tex.create_view(&Default::default())))
    }

    /// Release a palette's GPU memory (and the bind groups made with it);
    /// its id may be handed out again by the next upload, so the caller
    /// must forget it. Draws still naming it are skipped.
    pub fn free_palette(&mut self, id: PaletteId) {
        if self.palettes.remove(id.0) {
            self.drop_palette_binds(|p| p == id.0);
        }
    }

    /// [`free_palette`](Self::free_palette) for many at once: one pass over
    /// the bind groups instead of one per palette.
    pub fn free_palettes(&mut self, ids: impl IntoIterator<Item = PaletteId>) {
        let gone: std::collections::HashSet<usize> = ids
            .into_iter()
            .filter(|id| self.palettes.remove(id.0))
            .map(|id| id.0)
            .collect();
        if !gone.is_empty() {
            self.drop_palette_binds(|p| gone.contains(&p));
        }
    }

    /// Forget the bind groups made with palettes `gone` says are freed.
    fn drop_palette_binds(&mut self, gone: impl Fn(usize) -> bool) {
        self.indexed_binds.retain(|_, binds| {
            binds.retain(|(p, _)| !gone(*p));
            !binds.is_empty()
        });
    }

    /// The bind group of an indexed texture with a palette, if built.
    fn indexed_bind(&self, tex: usize, pal: usize) -> Option<&wgpu::BindGroup> {
        self.indexed_binds
            .get(&tex)?
            .iter()
            .find(|(p, _)| *p == pal)
            .map(|(_, b)| b)
    }

    /// Profiling counters; resets the upload count.
    pub fn take_stats(&self) -> RenderStats {
        let (uploads, upload_bytes) = self.uploads.replace((0, 0));
        RenderStats {
            uploads,
            upload_bytes,
            draws: self.last_frame.0,
            quads: self.last_frame.1,
            textures: self.textures.len() - self.free_slots.len(),
            palettes: self.palettes.live(),
        }
    }

    pub fn texture_size(&self, id: TextureId) -> (u32, u32) {
        let t = &self.textures[id.0];
        (t.width, t.height)
    }

    pub fn white(&self) -> TextureId {
        self.white
    }

    pub fn begin(&mut self) {
        self.verts.clear();
        self.runs.clear();
    }

    pub fn draw(&mut self, d: Draw) {
        let (tw, th) = match d.rect {
            Some(rc) => (rc[2], rc[3]),
            None => self.texture_size(d.tex),
        };
        let src = d.src.unwrap_or([0, 0, tw, th]);
        let (w, h) = (src[2] as f32 * d.scale[0], src[3] as f32 * d.scale[1]);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (su0, su1) = (
            src[0] as f32 / tw.max(1) as f32,
            (src[0] + src[2]) as f32 / tw.max(1) as f32,
        );
        let (sv0, sv1) = (
            src[1] as f32 / th.max(1) as f32,
            (src[1] + src[3]) as f32 / th.max(1) as f32,
        );
        let ax = d.axis[0] * d.scale[0];
        let ay = d.axis[1] * d.scale[1];
        let (x0, x1) = if d.hflip {
            (d.pos[0] + ax - w, d.pos[0] + ax)
        } else {
            (d.pos[0] - ax, d.pos[0] - ax + w)
        };
        let (y0, y1) = if d.vflip {
            (d.pos[1] + ay - h, d.pos[1] + ay)
        } else {
            (d.pos[1] - ay, d.pos[1] - ay + h)
        };
        let (mut u0, mut u1) = if d.hflip { (su1, su0) } else { (su0, su1) };
        let (mut v0, mut v1) = if d.vflip { (sv1, sv0) } else { (sv0, sv1) };
        let (mut x0, mut x1, mut y0, mut y1) = (x0, x1, y0, y1);
        if let Some(c) = d.clip
            && (d.angle == 0.0 || d.clip_local)
        {
            // shrink the quad to the window and trim the UVs by the same fractions
            let (cx0, cx1) = (x0.max(c[0]), x1.min(c[2]));
            let (cy0, cy1) = (y0.max(c[1]), y1.min(c[3]));
            if cx0 >= cx1 || cy0 >= cy1 {
                return;
            }
            let fx0 = (cx0 - x0) / (x1 - x0);
            let fx1 = (cx1 - x0) / (x1 - x0);
            let fy0 = (cy0 - y0) / (y1 - y0);
            let fy1 = (cy1 - y0) / (y1 - y0);
            let (nu0, nu1) = (u0 + (u1 - u0) * fx0, u0 + (u1 - u0) * fx1);
            let (nv0, nv1) = (v0 + (v1 - v0) * fy0, v0 + (v1 - v0) * fy1);
            (u0, u1, v0, v1) = (nu0, nu1, nv0, nv1);
            (x0, x1, y0, y1) = (cx0, cx1, cy0, cy1);
        }
        let mut corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        if d.skew != 0.0 {
            corners[0][0] += d.skew;
            corners[1][0] += d.skew;
        }
        if d.angle != 0.0 {
            let rad = d.angle * (std::f32::consts::PI / 180.0);
            let (sn, cs) = (libm::sinf(rad), libm::cosf(rad));
            for c in &mut corners {
                let (dx, dy) = (c[0] - d.pos[0], c[1] - d.pos[1]);
                *c = [d.pos[0] + dx * cs + dy * sn, d.pos[1] - dx * sn + dy * cs];
            }
        }
        let mut color = d.color;
        color[3] *= d.blend.src_scale();
        let rect = d
            .rect
            .map_or(WHOLE, |rc| rc.map(|v| v.min(u16::MAX as u32) as u16));
        self.push_quad(
            corners,
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
            color,
            d.tint,
            d.tex,
            d.pal,
            d.blend,
            rect,
        );
    }

    /// Queue a [`Quad`].
    pub fn draw_quad(&mut self, q: &Quad) {
        let [tl, tr, br, bl] = q.corners;
        let n = q.strips.clamp(1, 256);
        let lerp =
            |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let mut color = q.color;
        color[3] *= q.blend.src_scale();
        for k in 0..n {
            let (t0, t1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
            let mut poly = vec![
                (lerp(tl, bl, t0), [0.0, t0]),
                (lerp(tr, br, t0), [1.0, t0]),
                (lerp(tr, br, t1), [1.0, t1]),
                (lerp(tl, bl, t1), [0.0, t1]),
            ];
            if let Some(c) = q.clip {
                poly = clip_polygon(poly, c);
            }
            if poly.len() < 3 {
                continue;
            }
            // a fan of triangles, two per pushed quad (a lone last one is
            // doubled: a quad of six vertices stays the buffer's unit)
            let mut k2 = 1;
            while k2 + 1 < poly.len() {
                let a = poly[0];
                let b = poly[k2];
                let c = poly[k2 + 1];
                let d = if k2 + 2 < poly.len() { poly[k2 + 2] } else { c };
                self.push_quad(
                    [a.0, b.0, c.0, d.0],
                    [a.1, b.1, c.1, d.1],
                    color,
                    q.tint,
                    q.tex,
                    q.pal,
                    q.blend,
                    WHOLE,
                );
                k2 += 2;
            }
        }
    }

    /// Where the next queued draw starts (see [`Renderer::remap_since`]).
    pub fn vertex_mark(&self) -> usize {
        self.verts.len()
    }

    /// Move everything queued since `mark` (drawn for camera `from`) into
    /// the rectangle `dst` (`[min x, min y, max x, max y]`) of the space the
    /// frame ends in: `from`'s view is scaled uniformly to cover `dst`
    /// (centred, the overflow cropped) and clipped to it, so one frame can
    /// show a fight in a window of a menu (the fighter debugger).
    pub fn remap_since(&mut self, mark: usize, from: &Camera, dst: [f32; 4]) {
        let (w, h) = (dst[2] - dst[0], dst[3] - dst[1]);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let k = from.zoom * (w / from.screen[0]).max(h / from.screen[1]);
        let mid = [(dst[0] + dst[2]) / 2.0, (dst[1] + dst[3]) / 2.0];
        let map = |p: [f32; 2]| {
            [
                mid[0] + (p[0] - from.center[0]) * k,
                mid[1] + (p[1] - from.center[1]) * k,
            ]
        };
        let start = mark.min(self.verts.len()) / QUAD_VERTS * QUAD_VERTS;
        for q in self.verts[start..].chunks_exact_mut(QUAD_VERTS) {
            for v in q.iter_mut() {
                v.pos = map(v.pos);
            }
            clip_quad(q, dst);
        }
    }

    /// Axis-aligned filled rectangle in logical coords.
    pub fn rect(&mut self, min: [f32; 2], max: [f32; 2], color: [f32; 4]) {
        let w = self.white;
        self.quad(min, max, [0.0, 0.0], [1.0, 1.0], color, w, None);
    }

    /// Rectangle outline, `t` logical pixels thick.
    pub fn rect_outline(&mut self, min: [f32; 2], max: [f32; 2], t: f32, color: [f32; 4]) {
        self.rect(min, [max[0], min[1] + t], color);
        self.rect([min[0], max[1] - t], max, color);
        self.rect(min, [min[0] + t, max[1]], color);
        self.rect([max[0] - t, min[1]], max, color);
    }

    #[allow(clippy::too_many_arguments)]
    fn quad(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        uv0: [f32; 2],
        uv1: [f32; 2],
        color: [f32; 4],
        tex: TextureId,
        pal: Option<PaletteId>,
    ) {
        self.push_quad(
            [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]],
            [
                [uv0[0], uv0[1]],
                [uv1[0], uv0[1]],
                [uv1[0], uv1[1]],
                [uv0[0], uv1[1]],
            ],
            color,
            Tint::default(),
            tex,
            pal,
            Blend::Alpha,
            WHOLE,
        );
    }

    /// Push a quad with corners in order top-left, top-right, bottom-right, bottom-left.
    #[allow(clippy::too_many_arguments)]
    fn push_quad(
        &mut self,
        c: [[f32; 2]; 4],
        uv: [[f32; 2]; 4],
        color: [f32; 4],
        tint: Tint,
        tex: TextureId,
        pal: Option<PaletteId>,
        blend: Blend,
        rect: [u16; 4],
    ) {
        let pal = pal.map(|p| p.0);
        let start = (self.verts.len() / QUAD_VERTS) as u32;
        match self.runs.last_mut() {
            Some(r) if r.tex == tex.0 && r.pal == pal && r.blend == blend => {}
            _ => self.runs.push(Run {
                tex: tex.0,
                pal,
                blend,
                start,
            }),
        }
        let fx_mul = [
            tint.mul[0],
            tint.mul[1],
            tint.mul[2],
            tint.color.clamp(0.0, 1.0) + if tint.invert { 2.0 } else { 0.0 },
        ];
        let fx_add = [tint.add[0], tint.add[1], tint.add[2], blend.dst_scale()];
        let v = |i: usize| Vertex {
            pos: c[i],
            uv: uv[i],
            color,
            fx_mul,
            fx_add,
            rect,
        };
        self.verts.extend_from_slice(&[v(0), v(1), v(2), v(3)]);
    }

    /// Build bind groups for this frame's runs, upload the camera and the
    /// vertices (into the persistent buffer, grown when a frame outgrows it).
    fn prepare(&mut self, cam: &Camera) {
        let quads = (self.verts.len() / QUAD_VERTS) as u32;
        self.last_frame = (self.runs.len(), quads as usize);
        if quads > self.vbuf_quads {
            let want = quads.next_power_of_two().max(INITIAL_QUADS);
            let (v, i) = Self::make_buffers(&self.device, &self.queue, want);
            self.vbuf = v;
            self.ibuf = i;
            self.vbuf_quads = want;
            self.ibuf_quads = want;
            log::debug!("render: vertex buffer grown to {want} quads");
        }
        for r in &self.runs {
            let (t, p) = (r.tex, r.pal);
            if let (Kind::Indexed { view }, Some(p)) = (&self.textures[t].kind, p)
                && let Some(pal) = self.palettes.get(p)
                && self.indexed_bind(t, p).is_none()
            {
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.indexed_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(pal),
                        },
                    ],
                });
                self.indexed_binds.entry(t).or_default().push((p, bind));
            }
        }
        self.queue.write_buffer(
            &self.cam_buf,
            0,
            bytemuck::bytes_of(&CameraUniform {
                screen: cam.screen,
                center: cam.center,
                zoom: cam.zoom,
                _pad: 0.0,
                _pad2: [0.0; 2],
            }),
        );
        if !self.verts.is_empty() {
            self.queue
                .write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&self.verts));
        }
    }

    fn encode_pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: [f64; 4],
    ) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0],
                        g: clear[1],
                        b: clear[2],
                        a: clear[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, &self.cam_bind, &[]);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        let total = (self.verts.len() / QUAD_VERTS) as u32;
        // the pipeline rarely changes between runs (indexed sprites, alpha
        // blend): set it only when it does
        let mut pipe: Option<(bool, usize)> = None;
        for (i, r) in self.runs.iter().enumerate() {
            let (t, p, start) = (r.tex, r.pal, r.start);
            let end = self.runs.get(i + 1).map_or(total, |r| r.start);
            let bi = r.blend.pipe_index();
            let (indexed, bind) = match (&self.textures[t].kind, p) {
                // a freed palette has no bind group: skip the draw
                (Kind::Indexed { .. }, Some(p)) => match self.indexed_bind(t, p) {
                    Some(b) => (true, b),
                    None => continue,
                },
                (Kind::Rgba { bind }, _) => (false, bind),
                (Kind::Indexed { .. }, None) | (Kind::Free, _) => continue,
            };
            if pipe != Some((indexed, bi)) {
                pipe = Some((indexed, bi));
                pass.set_pipeline(if indexed {
                    &self.indexed_pipes[bi]
                } else {
                    &self.rgba_pipes[bi]
                });
            }
            pass.set_bind_group(1, bind, &[]);
            let n = QUAD_INDICES as u32;
            pass.draw_indexed(start * n..end * n, 0, 0..1);
        }
    }

    /// Present the frame queued since [`begin`](Self::begin).
    pub fn end(&mut self, cam: &Camera, clear: [f64; 4]) -> Result<(), wgpu::SurfaceError> {
        self.prepare(cam);
        let frame = match self.acquired.take() {
            Some(f) => f,
            None => {
                let t = std::time::Instant::now();
                let f = self.surface.get_current_texture()?;
                self.timing.acquire = t.elapsed();
                self.timing.acquired = Some(std::time::Instant::now());
                f
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_pass(&mut enc, &view, clear);
        self.queue.submit([enc.finish()]);
        frame.present();
        self.timing.presented = Some(std::time::Instant::now());
        Ok(())
    }

    /// Render the queued frame into an offscreen `w x h` image and read it
    /// back as straight RGBA8. For screenshots and tests.
    pub fn end_to_image(&mut self, cam: &Camera, clear: [f64; 4], w: u32, h: u32) -> Vec<u8> {
        self.prepare(cam);
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_pass(&mut enc, &view, clear);
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(h),
                },
            },
            size,
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait());
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let r = &data[(row * bpr) as usize..(row * bpr + w * 4) as usize];
            if bgra {
                for px in r.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(r);
            }
        }
        drop(data);
        buf.unmap();
        out
    }
}

pub use wgpu::{SurfaceError, SurfaceTarget};

#[cfg(test)]
mod tests {
    use super::*;

    /// Released palettes give their slot back: replaced stage previews no
    /// longer grow the palette list (and a stale id draws nothing).
    #[test]
    fn freed_slots_are_reused() {
        let mut s: Slots<u32> = Slots::default();
        let a = s.insert(1);
        let b = s.insert(2);
        assert_eq!(s.live(), 2);
        assert!(s.remove(a));
        assert!(!s.remove(a), "a second free is ignored");
        assert_eq!(s.get(a), None, "a freed id resolves to nothing");
        assert_eq!(s.live(), 1);
        for round in 0..100 {
            let c = s.insert(10 + round);
            assert_eq!(c, a, "the freed slot is reused");
            assert!(s.remove(c));
        }
        assert_eq!(s.items.len(), 2, "no growth over 100 replacements");
        assert_eq!(s.get(b), Some(&2));
    }
}

#[cfg(test)]
mod remap_tests {
    use super::*;

    fn quad(x0: f32, y0: f32, x1: f32, y1: f32) -> [Vertex; QUAD_VERTS] {
        let v = |x: f32, y: f32, u: f32, w: f32| Vertex {
            pos: [x, y],
            uv: [u, w],
            color: [1.0; 4],
            fx_mul: [1.0; 4],
            fx_add: [0.0; 4],
            rect: WHOLE,
        };
        [
            v(x0, y0, 0.0, 0.0),
            v(x1, y0, 1.0, 0.0),
            v(x1, y1, 1.0, 1.0),
            v(x0, y1, 0.0, 1.0),
        ]
    }

    /// Two triangles per quad, over its four corners, in vertex order.
    #[test]
    fn quads_are_indexed() {
        assert_eq!(quad_indices(0), [0, 1, 2, 0, 2, 3]);
        assert_eq!(quad_indices(2), [8, 9, 10, 8, 10, 11]);
    }

    /// A quad half outside the window is cut at its edge with the texture
    /// trimmed to match; one wholly outside draws nothing.
    #[test]
    fn quads_are_clipped_to_the_window() {
        let mut q = quad(-10.0, 0.0, 10.0, 10.0);
        clip_quad(&mut q, [0.0, 0.0, 100.0, 100.0]);
        assert_eq!(q[0].pos, [0.0, 0.0]);
        assert_eq!(q[0].uv, [0.5, 0.0]);
        assert_eq!(q[1].pos, [10.0, 0.0]);
        assert_eq!(q[1].uv, [1.0, 0.0]);
        let mut out = quad(200.0, 0.0, 210.0, 10.0);
        clip_quad(&mut out, [0.0, 0.0, 100.0, 100.0]);
        assert!(out.iter().all(|v| v.pos == [0.0, 0.0]));
        let mut inside = quad(1.0, 1.0, 2.0, 2.0);
        let before = inside;
        clip_quad(&mut inside, [0.0, 0.0, 100.0, 100.0]);
        assert_eq!(inside[2].pos, before[2].pos);
    }
}
