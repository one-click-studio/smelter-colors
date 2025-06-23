use anyhow::{Context, Result};
use compositor_pipeline::pipeline::RawDataReceiver;
use compositor_render::scene::{
    AbsolutePosition, BorderRadius, HorizontalAlign, HorizontalPosition, ImageComponent,
    InputStreamComponent, Overflow, Padding, Position, RGBAColor, RescaleMode, RescalerComponent,
    VerticalAlign, VerticalPosition, ViewChildrenDirection, ViewComponent,
};
use compositor_render::Resolution;
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
    scene::Component,
    Frame, Framerate, InputId, OutputId, RendererId, RendererSpec, RenderingMode,
};

pub static PLACEHOLDER: Component = Component::View(ViewComponent {
    id: None,
    children: vec![],
    direction: ViewChildrenDirection::Row,
    position: Position::Static {
        width: None,
        height: None,
    },
    transition: None,
    overflow: Overflow::Visible,
    background_color: RGBAColor(0, 0, 0, 0),
    border_radius: BorderRadius {
        top_left: 0.,
        top_right: 0.,
        bottom_right: 0.,
        bottom_left: 0.,
    },
    border_width: 0.,
    border_color: RGBAColor(0, 0, 0, 0),
    box_shadow: vec![],
    padding: Padding {
        top: 0.,
        right: 0.,
        bottom: 0.,
        left: 0.,
    },
});

#[allow(dead_code)]
pub struct CompositorPipeline {
    pipeline: Arc<Mutex<Pipeline>>,
    output_receiver: Option<crossbeam_channel::Receiver<PipelineEvent<Frame>>>,
    graphics_context: GraphicsContext,

    components: Vec<Component>,
    raw_output: OutputId,
    mp4_output: OutputId,
    is_recording: bool,
}

impl CompositorPipeline {
    pub fn new(width: usize, height: usize) -> Result<(Self, GraphicsContext)> {
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

        // Components to alternate between
        let components = [
            Component::Image(ImageComponent {
                id: None,
                image_id: RendererId(image_input_id.0.clone()),
            }),
            Component::Rescaler(RescalerComponent {
                id: None,
                child: Box::new(Component::InputStream(InputStreamComponent {
                    id: None,
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
            }),
        ]
        .to_vec();

        // Register raw output
        let raw_output = OutputId(Arc::from("raw_output"));
        let mp4_output = OutputId(Arc::from("mp4_output"));
        let raw_receiver = Self::register_raw_output(&raw_output, &pipeline, width, height)?;

        let compositor = Self {
            pipeline,
            output_receiver: raw_receiver.video,
            graphics_context: graphics_context.clone(),

            components,
            raw_output,
            mp4_output,
            is_recording: false,
        };

        Ok((compositor, graphics_context))
    }

    fn create_pipeline(graphics_context: &GraphicsContext) -> Result<Arc<Mutex<Pipeline>>> {
        let (pipeline, _event_loop) = Pipeline::new(compositor_pipeline::pipeline::Options {
            queue_options: compositor_pipeline::queue::QueueOptions {
                default_buffer_duration: Duration::ZERO,
                ahead_of_time_processing: false,
                output_framerate: Framerate { num: 10, den: 1 },
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

    fn register_raw_output(
        output_id: &OutputId,
        pipeline: &Arc<Mutex<Pipeline>>,
        width: usize,
        height: usize,
    ) -> Result<RawDataReceiver> {
        let raw_receiver = Pipeline::register_raw_data_output(
            &pipeline,
            output_id.clone(),
            RegisterOutputOptions {
                output_options: RawDataOutputOptions {
                    video: Some(RawVideoOptions {
                        resolution: Resolution { width, height },
                    }),
                    audio: None,
                },
                video: Some(OutputVideoOptions {
                    initial: PLACEHOLDER.clone(),
                    end_condition: PipelineOutputEndCondition::Never,
                }),
                audio: None,
            },
        )?;
        info!("Registered raw output");
        Ok(raw_receiver)
    }

    fn start_record(
        pipeline: &Arc<Mutex<Pipeline>>,
        output_id: &OutputId,
        width: usize,
        height: usize,
        path: PathBuf,
    ) -> Result<()> {
        use compositor_pipeline::pipeline::encoder::*;
        use compositor_pipeline::pipeline::output::*;

        info!("Starting recording to {}", path.display());

        if path.exists() {
            std::fs::remove_file(path.clone())?;
        }

        let _ = compositor_pipeline::Pipeline::register_output(
            pipeline,
            output_id.clone(),
            RegisterOutputOptions {
                output_options: OutputOptions::Mp4(mp4::Mp4OutputOptions {
                    output_path: path.clone(),
                    video: Some(VideoEncoderOptions::H264(ffmpeg_h264::Options {
                        preset: ffmpeg_h264::EncoderPreset::Medium,
                        resolution: Resolution { width, height },
                        raw_options: [].to_vec(),
                    })),
                    audio: None,
                }),
                video: Some(OutputVideoOptions {
                    initial: PLACEHOLDER.clone(),
                    end_condition: PipelineOutputEndCondition::Never,
                }),
                audio: None,
            },
        )?;

        Ok(())
    }

    fn stop_record(pipeline: &Arc<Mutex<Pipeline>>, output_id: &OutputId) -> Result<()> {
        let mut pipeline = pipeline.lock().unwrap();
        Pipeline::unregister_output(&mut *pipeline, output_id)?;

        info!("Stopped recording");

        Ok(())
    }

    pub fn start(&mut self) {
        Self::alternate_scenes(
            &self.pipeline.clone(),
            self.components.clone(),
            &self.raw_output,
            None,
        );
        self.record(
            1920,
            1080,
            Duration::from_secs(5),
            PathBuf::from("output.mp4"),
        );
    }

    fn alternate_scenes(
        pipeline: &Arc<Mutex<Pipeline>>,
        components: Vec<Component>,
        output_id: &OutputId,
        duration: Option<Duration>,
    ) {
        let components = components.clone();
        let pipeline = pipeline.clone();
        let output_id = output_id.clone();

        std::thread::spawn(move || {
            let mut index = 0;
            let start = std::time::Instant::now();
            loop {
                index = (index + 1) % components.len();
                let component = components[index].clone();

                let mut pipeline_lock = pipeline.lock().unwrap();
                let _ = Pipeline::update_output(
                    &mut *pipeline_lock,
                    output_id.clone(),
                    Some(component.clone()),
                    None,
                );
                drop(pipeline_lock);

                if let Some(duration) = duration {
                    if start.elapsed() >= duration {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    pub fn record(&mut self, width: usize, height: usize, duration: Duration, path: PathBuf) {
        let components = self.components.clone();
        let pipeline = self.pipeline.clone();
        let mp4_output = self.mp4_output.clone();

        std::thread::spawn(move || {
            Self::start_record(&pipeline, &mp4_output, width, height, path).unwrap();
            Self::alternate_scenes(&pipeline, components, &mp4_output, Some(duration));
            std::thread::sleep(duration);
            Self::stop_record(&pipeline, &mp4_output).unwrap();
        });
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
