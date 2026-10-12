//! Window + renderer pair shared by every screen.

use std::sync::Arc;

use rugen_render::Renderer;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

pub struct Gpu {
    // declared before `window` so the surface is dropped first
    pub renderer: Renderer,
    pub window: Arc<Window>,
}

impl Gpu {
    pub fn create(
        event_loop: &ActiveEventLoop,
        size: (u32, u32),
        fullscreen: bool,
        presentation: rugen_render::Presentation,
    ) -> Self {
        let mut attrs = Window::default_attributes()
            .with_title("rugen")
            .with_window_icon(window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(size.0 as f64, size.1 as f64));
        // app id / WM class matching packaging/rugen.desktop, so desktops show the icon
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs = attrs.with_name("rugen", "rugen");
        }
        if fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let s = window.inner_size();
        let renderer = pollster::block_on(Renderer::with_presentation(
            window.clone(),
            s.width,
            s.height,
            presentation,
        ))
        .expect("renderer");
        Self { renderer, window }
    }
}

/// Window / taskbar icon, embedded from `motif/brand/icon.webp`.
pub fn window_icon() -> Option<winit::window::Icon> {
    const ICON: &[u8] = include_bytes!("../../../motif/brand/icon.webp");
    let mut dec = image_webp::WebPDecoder::new(std::io::Cursor::new(ICON)).ok()?;
    let (w, h) = dec.dimensions();
    let mut buf = vec![0u8; dec.output_buffer_size()?];
    dec.read_image(&mut buf).ok()?;
    if !dec.has_alpha() {
        buf = buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
    }
    winit::window::Icon::from_rgba(buf, w, h).ok()
}
