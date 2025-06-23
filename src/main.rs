mod compositor;
mod wgpu;

use anyhow::Result;
use compositor::Compositor;
use std::time::Duration;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter("smelter_colors=debug,compositor_pipeline=error,compositor_render=error")
        .init();

    let mut compositor = Compositor::new()?;
    compositor.save_images()?;
    compositor.record_for(Duration::from_secs(5))?;

    Ok(())
}
