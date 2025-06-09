use anyhow::{Context, Result};
use compositor_render::scene::ImageComponent;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

use compositor_pipeline::{
    pipeline::{
        output::{RawDataOutputOptions, RawVideoOptions},
        GraphicsContext, GraphicsContextOptions, OutputVideoOptions,
        PipelineOutputEndCondition, RegisterOutputOptions,
    },
    queue::PipelineEvent,
    Pipeline,
};
use compositor_render::{
    image::{ImageSource, ImageSpec},
    scene::{Component, ComponentId},
    Frame, Framerate, OutputId, RendererId, RendererSpec, RenderingMode,
};

#[allow(dead_code)]
pub struct CompositorPipeline {
    pipeline: Arc<Mutex<Pipeline>>,
    output_receiver: Option<crossbeam_channel::Receiver<PipelineEvent<Frame>>>,
    graphics_context: GraphicsContext,
}

impl CompositorPipeline {
    pub fn new(width: u32, height: u32) -> Result<(Self, GraphicsContext)> {
        let graphics_context = GraphicsContext::new(GraphicsContextOptions {
            force_gpu: false,
            features: wgpu::Features::PUSH_CONSTANTS
                | wgpu::Features::TEXTURE_BINDING_ARRAY,
            limits: wgpu::Limits::default(),
            compatible_surface: None,
            libvulkan_path: None,
        })
        .context("Cannot initialize WGPU")?;

        // Smelter pipeline
        let (pipeline, _event_loop) =
            Pipeline::new(compositor_pipeline::pipeline::Options {
                queue_options: compositor_pipeline::queue::QueueOptions {
                    default_buffer_duration: Duration::ZERO,
                    ahead_of_time_processing: false,
                    output_framerate: Framerate { num: 30, den: 1 },
                    run_late_scheduled_events: true,
                    never_drop_output_frames: false,
                },
                stream_fallback_timeout: Duration::from_millis(500),
                web_renderer: compositor_render::web_renderer::WebRendererInitOptions {
                    enable: false,
                    enable_gpu: false,
                },
                force_gpu: false,
                download_root: std::env::temp_dir(),
                mixing_sample_rate: 48000,
                wgpu_features: wgpu::Features::PUSH_CONSTANTS
                    | wgpu::Features::TEXTURE_BINDING_ARRAY,
                load_system_fonts: None,
                wgpu_ctx: Some(graphics_context.clone()),
                stun_servers: Default::default(),
                whip_whep_server_port: 9000,
                start_whip_whep: false,
                tokio_rt: None,
                rendering_mode: RenderingMode::GpuOptimized,
            })
            .context("Failed to create compositor pipeline")?;

        let pipeline = Arc::new(Mutex::new(pipeline));
        Pipeline::start(&pipeline);

        // Register image input
        let image_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("RGBBW.jpg");

        let input_id = RendererId(Arc::from("image_input"));
        Pipeline::register_renderer(
            &pipeline,
            input_id.clone(),
            RendererSpec::Image(ImageSpec {
                src: ImageSource::LocalPath {
                    path: image_path.to_string_lossy().to_string(),
                },
                image_type: compositor_render::image::ImageType::Jpeg,
            }),
        )?;
        info!("Registered image input");

        // Create component for output
        let component = Component::Image(ImageComponent {
            id: Some(ComponentId(input_id.0.clone())),
            image_id: RendererId(input_id.0.clone()),
        });

        // Register raw output
        let output_id = OutputId(Arc::from("raw_output"));
        let raw_receiver = Pipeline::register_raw_data_output(
            &pipeline,
            output_id.clone(),
            RegisterOutputOptions {
                output_options: RawDataOutputOptions {
                    video: Some(RawVideoOptions {
                        resolution: compositor_render::Resolution {
                            width: width as usize,
                            height: height as usize,
                        },
                    }),
                    audio: None,
                },
                video: Some(OutputVideoOptions {
                    initial: component,
                    end_condition: PipelineOutputEndCondition::Never,
                }),
                audio: None,
            },
        )?;
        info!("Registered raw output");

        let compositor = Self {
            pipeline,
            output_receiver: raw_receiver.video,
            graphics_context: graphics_context.clone(),
        };

        Ok((compositor, graphics_context))
    }

    pub fn try_get_frame(&self) -> Option<Frame> {
        let receiver = self.output_receiver.as_ref()?;

        // Get the latest frame, discarding older ones
        let mut latest_frame = None;
        while let Ok(event) = receiver.try_recv() {
            if let PipelineEvent::Data(frame) = event {
                latest_frame = Some(frame);
            }
        }

        latest_frame
    }
}
