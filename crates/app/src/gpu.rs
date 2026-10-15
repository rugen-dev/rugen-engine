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
    ) -> Result<Self, String> {
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
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("could not open a window: {e}"))?,
        );
        let s = window.inner_size();
        let renderer = pollster::block_on(Renderer::with_presentation(
            window.clone(),
            s.width,
            s.height,
            presentation,
        ))
        .map_err(|e| gpu_error(&e))?;
        crate::machine::set_gpu(renderer.adapter());
        crate::machine::set_display(s.width, s.height);
        Ok(Self { renderer, window })
    }
}

/// What to tell the player when no GPU device could be made (logged and
/// printed; the shell quits instead of panicking in the event loop).
pub fn gpu_error(e: &rugen_render::Error) -> String {
    format!(
        "rugen could not start the graphics device: {e}.\n\
         rugen needs OpenGL ES 3.0 / OpenGL 3.3, Vulkan, DirectX 12 or Metal. \
         Update the graphics driver; on an old or integrated GPU try WGPU_BACKEND=gl \
         (or WGPU_BACKEND=vulkan) in the environment. Send the log around this line \
         (RUST_LOG=info) when reporting the problem."
    )
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
