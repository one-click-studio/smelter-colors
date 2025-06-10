use anyhow::{Context, Result};
use compositor_render::scene::{
    AbsolutePosition, HorizontalAlign, HorizontalPosition, ImageComponent, InputStreamComponent,
    Position, RGBAColor, RescaleMode, RescalerComponent, VerticalAlign, VerticalPosition,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

use compositor_pipeline::{
    pipeline::{
        input::{
            mp4::{Mp4Options, Source},
            InputOptions,
        },
        output::{RawDataOutputOptions, RawVideoOptions},
        GraphicsContext, GraphicsContextOptions, OutputVideoOptions, PipelineOutputEndCondition,
        RegisterInputOptions, RegisterOutputOptions, VideoDecoder,
    },
    queue::{PipelineEvent, QueueInputOptions},
    Pipeline,
};
use compositor_render::{
    image::{ImageSource, ImageSpec},
    scene::{Component, ComponentId},
    Frame, Framerate, InputId, OutputId, RendererId, RendererSpec, RenderingMode,
};

#[allow(dead_code)]
pub struct CompositorPipeline {
    pipeline: Arc<Mutex<Pipeline>>,
    output_receiver: Option<crossbeam_channel::Receiver<PipelineEvent<Frame>>>,
    graphics_context: GraphicsContext,
    image_input_id: RendererId,
    mp4_input_id: InputId,
    output_id: OutputId,
    current_source: Arc<Mutex<bool>>, // true = image, false = mp4
}

impl CompositorPipeline {
    fn create_pipeline(graphics_context: &GraphicsContext) -> Result<Arc<Mutex<Pipeline>>> {
        let (pipeline, _event_loop) = Pipeline::new(compositor_pipeline::pipeline::Options {
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
            wgpu_features: wgpu::Features::PUSH_CONSTANTS | wgpu::Features::TEXTURE_BINDING_ARRAY,
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
        Ok(pipeline)
    }

    fn register_inputs(pipeline: &Arc<Mutex<Pipeline>>) -> Result<(RendererId, InputId)> {
        let assets_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        let image_path = assets_path.join("RGBBW.jpg");
        let mp4_path = assets_path.join("RGBBW.mp4");

        // Register image
        let image_input_id = RendererId(Arc::from("image_input"));
        Pipeline::register_renderer(
            pipeline,
            image_input_id.clone(),
            RendererSpec::Image(ImageSpec {
                src: ImageSource::LocalPath {
                    path: image_path.to_string_lossy().to_string(),
                },
                image_type: compositor_render::image::ImageType::Jpeg,
            }),
        )?;
        info!("Registered image input");

        // Register MP4
        let mp4_input_id = InputId(Arc::from("mp4_input"));
        let video_decoder = VideoDecoder::FFmpegH264;
        let input_options = InputOptions::Mp4(Mp4Options {
            source: Source::File(mp4_path),
            should_loop: true,
            video_decoder,
        });
        let options = RegisterInputOptions {
            input_options,
            queue_options: QueueInputOptions {
                required: false,
                offset: None,
                buffer_duration: Some(Duration::ZERO),
            },
        };
        Pipeline::register_input(pipeline, mp4_input_id.clone(), options)?;
        info!("Registered MP4 input");

        Ok((image_input_id, mp4_input_id))
    }

    fn start_scene_alternation_thread(
        pipeline: Arc<Mutex<Pipeline>>,
        output_id: OutputId,
        image_input_id: RendererId,
        mp4_input_id: InputId,
        current_source: Arc<Mutex<bool>>,
        width: u32,
        height: u32,
    ) {
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));

            let mut is_image = current_source.lock().unwrap();
            *is_image = !*is_image;

            let new_component = if *is_image {
                Component::Image(ImageComponent {
                    id: Some(ComponentId(image_input_id.0.clone())),
                    image_id: RendererId(image_input_id.0.clone()),
                })
            } else {
                Component::Rescaler(RescalerComponent {
                    id: None,
                    child: Box::new(Component::InputStream(InputStreamComponent {
                        id: Some(ComponentId(mp4_input_id.0.clone())),
                        input_id: mp4_input_id.clone(),
                    })),
                    position: Position::Absolute(AbsolutePosition {
                        width: Some(width as f32),
                        height: Some(height as f32),
                        position_horizontal: HorizontalPosition::LeftOffset(0.0),
                        position_vertical: VerticalPosition::TopOffset(0.0),
                        rotation_degrees: 0.0,
                    }),
                    transition: None,
                    mode: RescaleMode::Fill,
                    horizontal_align: HorizontalAlign::Center,
                    vertical_align: VerticalAlign::Center,
                    border_radius: compositor_render::scene::BorderRadius::ZERO,
                    border_width: 0.0,
                    border_color: RGBAColor(0, 0, 0, 0),
                    box_shadow: vec![],
                })
            };

            let mut pipeline_lock = pipeline.lock().unwrap();
            Pipeline::update_output(
                &mut *pipeline_lock,
                output_id.clone(),
                Some(new_component),
                None,
            )
            .unwrap();
        });
    }

    pub fn new(width: u32, height: u32) -> Result<(Self, GraphicsContext)> {
        // Initialize graphics context
        let graphics_context = GraphicsContext::new(GraphicsContextOptions {
            force_gpu: false,
            features: wgpu::Features::PUSH_CONSTANTS | wgpu::Features::TEXTURE_BINDING_ARRAY,
            limits: wgpu::Limits::default(),
            compatible_surface: None,
            libvulkan_path: None,
        })
        .context("Cannot initialize WGPU")?;

        // Create and start pipeline
        let pipeline = Self::create_pipeline(&graphics_context)?;

        // Register inputs
        let (image_input_id, mp4_input_id) = Self::register_inputs(&pipeline)?;

        // Create initial component for output (start with image)
        let component = Component::Image(ImageComponent {
            id: Some(ComponentId(image_input_id.0.clone())),
            image_id: RendererId(image_input_id.0.clone()),
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

        // Start scene alternation
        let current_source = Arc::new(Mutex::new(true)); // Start with image
        Self::start_scene_alternation_thread(
            pipeline.clone(),
            output_id.clone(),
            image_input_id.clone(),
            mp4_input_id.clone(),
            current_source.clone(),
            width,
            height,
        );

        let compositor = Self {
            pipeline,
            output_receiver: raw_receiver.video,
            graphics_context: graphics_context.clone(),
            image_input_id,
            mp4_input_id,
            output_id,
            current_source,
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
