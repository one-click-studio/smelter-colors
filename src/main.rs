mod compositor;
mod renderer;
mod winit;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter(
            "smelter_colors=debug,compositor_pipeline=warn,compositor_render=warn",
        )
        .init();

    winit::App::run()
}
