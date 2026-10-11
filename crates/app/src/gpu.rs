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
    pub fn create(event_loop: &ActiveEventLoop, size: (u32, u32), fullscreen: bool) -> Self {
        let mut attrs = Window::default_attributes()
            .with_title("rugen")
            .with_inner_size(winit::dpi::LogicalSize::new(size.0 as f64, size.1 as f64));
        if fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let s = window.inner_size();
        let renderer =
            pollster::block_on(Renderer::new(window.clone(), s.width, s.height)).expect("renderer");
        Self { renderer, window }
    }
}
