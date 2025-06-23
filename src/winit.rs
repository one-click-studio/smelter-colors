use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::compositor::CompositorPipeline;
use crate::renderer::Renderer;

pub const WIDTH: usize = 1920;
pub const HEIGHT: usize = 1080;

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    compositor: Option<CompositorPipeline>,
    start_time: Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            compositor: None,
            start_time: Instant::now(),
        }
    }

    pub fn run() -> Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Wait);

        let mut app = App::new();
        event_loop.run_app(&mut app)?;

        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let result: Result<()> = (|| {
                // Create window
                let window_attrs = WindowAttributes::default().with_title("Smelter Colors");
                let window = Arc::new(event_loop.create_window(window_attrs)?);

                // Initialize compositor pipeline first (it creates the graphics context)
                let (mut compositor, graphics_context) = CompositorPipeline::new(WIDTH, HEIGHT)?;
                compositor.start();

                // Initialize renderer with the graphics context from compositor
                let renderer = Renderer::new(window.clone(), &graphics_context, WIDTH, HEIGHT)?;

                // Store components
                self.window = Some(window.clone());
                self.renderer = Some(renderer);
                self.compositor = Some(compositor);
                self.start_time = Instant::now();

                // Request initial redraw
                window.request_redraw();

                info!("Application initialized successfully");
                Ok(())
            })();

            if let Err(e) = result {
                eprintln!("Failed to initialize application: {:?}", e);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(physical_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(physical_size.width, physical_size.height);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Process compositor events and get frames
                if let Some(compositor) = &self.compositor {
                    if let Some(frame) = compositor.try_get_frame() {
                        if let Some(renderer) = &self.renderer {
                            renderer.update_texture_from_compositor(&frame);
                        }
                    }
                }

                // Render the current texture
                if let Some(renderer) = &self.renderer {
                    if let Err(e) = renderer.render() {
                        eprintln!("Render error: {:?}", e);
                    }
                }

                // Request next frame
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}
