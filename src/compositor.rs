use anyhow::{anyhow, Context, Result};
use compositor_pipeline::pipeline::output::*;
use compositor_pipeline::pipeline::RegisterOutputOptions;
use compositor_pipeline::queue::PipelineEvent;
use compositor_render::scene::*;
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
        *,
    },
    queue::QueueInputOptions,
    Pipeline,
};
use compositor_render::{
    image::{ImageSource, ImageSpec},
    scene::Component,
    *,
};

use crate::wgpu::to_image;

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

pub const WIDTH: usize = 1920;
pub const HEIGHT: usize = 1080;
pub const IMAGE: &str = "RGBBW.png";
pub const MP4: &str = "RGBBW.mp4";

pub struct Compositor {
    graphics_context: GraphicsContext,
    pipeline: Arc<Mutex<Pipeline>>,

    image_component: Component,
    mp4_component: Component,

    mp4_output: OutputId,
    raw_output: OutputId,
}

impl Compositor {
    pub fn new() -> Result<Self> {
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
        let image_component = Component::Image(ImageComponent {
            id: None,
            image_id: RendererId(image_input_id.0.clone()),
        });
        let mp4_component = Component::Rescaler(RescalerComponent {
            id: None,
            child: Box::new(Component::InputStream(InputStreamComponent {
                id: None,
                input_id: mp4_input_id.clone(),
            })),
            position: Position::Absolute(AbsolutePosition {
                width: Some(WIDTH as f32),
                height: Some(HEIGHT as f32),
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
        });

        Ok(Self {
            graphics_context,
            pipeline,

            image_component,
            mp4_component,

            mp4_output: OutputId(Arc::from("mp4_output")),
            raw_output: OutputId(Arc::from("raw_output")),
        })
    }

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
        let image_path = assets_path.join(IMAGE);
        let mp4_path = assets_path.join(MP4);

        // Register image
        let image_input_id = RendererId(Arc::from("image_input"));
        Pipeline::register_renderer(
            pipeline,
            image_input_id.clone(),
            RendererSpec::Image(ImageSpec {
                src: ImageSource::LocalPath {
                    path: image_path.to_string_lossy().to_string(),
                },
                image_type: compositor_render::image::ImageType::Png,
            }),
        )?;
        info!("Registered {}", image_path.display());

        // Register MP4
        let mp4_input_id = InputId(Arc::from("mp4_input"));
        let video_decoder = VideoDecoder::FFmpegH264;
        let input_options = InputOptions::Mp4(Mp4Options {
            source: Source::File(mp4_path.clone()),
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
        info!("Registered {}", mp4_path.display());

        Ok((image_input_id, mp4_input_id))
    }

    fn start_record(&mut self, path: PathBuf) -> Result<()> {
        use compositor_pipeline::pipeline::encoder::*;
        use compositor_pipeline::pipeline::output::*;

        if path.exists() {
            std::fs::remove_file(path.clone())?;
        }

        compositor_pipeline::Pipeline::register_output(
            &self.pipeline,
            self.mp4_output.clone(),
            RegisterOutputOptions {
                output_options: OutputOptions::Mp4(mp4::Mp4OutputOptions {
                    output_path: path.clone(),
                    video: Some(VideoEncoderOptions::H264(ffmpeg_h264::Options {
                        preset: ffmpeg_h264::EncoderPreset::Medium,
                        resolution: Resolution {
                            width: WIDTH,
                            height: HEIGHT,
                        },
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
        info!("Started recording to {}", path.display());

        Ok(())
    }

    fn stop_record(&mut self) -> Result<()> {
        let mut pipeline = self.pipeline.lock().unwrap();
        Pipeline::unregister_output(&mut *pipeline, &self.mp4_output)?;
        info!("Stopped recording");

        Ok(())
    }

    /// Alternates between the image and the MP4, changing every second.
    fn alternate_scenes(&mut self, duration: Duration) -> Result<()> {
        for i in 0..duration.as_secs() {
            let component = match i % 2 {
                0 => self.image_component.clone(),
                _ => self.mp4_component.clone(),
            };

            let mut pipeline_lock = self.pipeline.lock().unwrap();
            Pipeline::update_output(
                &mut *pipeline_lock,
                self.mp4_output.clone(),
                Some(component.clone()),
                None,
            )?;
            drop(pipeline_lock);

            std::thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    }

    fn register_raw_output(&mut self) -> Result<RawDataReceiver> {
        let raw_receiver = Pipeline::register_raw_data_output(
            &self.pipeline,
            self.raw_output.clone(),
            RegisterOutputOptions {
                output_options: RawDataOutputOptions {
                    video: Some(RawVideoOptions {
                        resolution: Resolution {
                            width: WIDTH,
                            height: HEIGHT,
                        },
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

        Ok(raw_receiver)
    }

    fn deregister_raw_output(&mut self) -> Result<()> {
        let mut pipeline = self.pipeline.lock().unwrap();
        Pipeline::unregister_output(&mut *pipeline, &self.raw_output)?;

        Ok(())
    }

    pub fn get_last_frame(raw_receiver: &RawDataReceiver) -> Result<Arc<wgpu::Texture>> {
        let receiver = raw_receiver.video.as_ref().context("No video channel")?;

        // Wait to have at least one frame
        let mut latest_frame = loop {
            match receiver.recv()? {
                PipelineEvent::Data(frame) => break frame,
                _ => continue,
            }
        };

        // Drain any additional available frames
        while let Ok(event) = receiver.try_recv() {
            if let PipelineEvent::Data(frame) = event {
                latest_frame = frame;
            }
        }

        // Extract the texture
        match latest_frame.data {
            FrameData::Rgba8UnormWgpuTexture(texture) => Ok(texture.clone()),
            _ => Err(anyhow!("Expected Rgba8UnormWgpuTexture")),
        }
    }

    /// Switch to a given component and extract pipeline output.
    pub fn render_component(
        &mut self,
        receiver: &RawDataReceiver,
        component: Component,
    ) -> Result<Arc<wgpu::Texture>> {
        let mut pipeline_lock = self.pipeline.lock().unwrap();
        Pipeline::update_output(
            &mut *pipeline_lock,
            self.raw_output.clone(),
            Some(component),
            None,
        )?;
        drop(pipeline_lock);

        std::thread::sleep(Duration::from_millis(100)); // Make sure this is the new component

        let frame = Self::get_last_frame(&receiver)?;

        Ok(frame)
    }

    pub fn save_images(&mut self) -> Result<()> {
        let receiver = self.register_raw_output()?;
        info!("Saving output to output_*.png");

        let frame = self.render_component(&receiver, self.image_component.clone())?;
        let image = to_image(&self.graphics_context, &frame)?;
        image.save("output_png.png")?;

        let frame = self.render_component(&receiver, self.mp4_component.clone())?;
        let image = to_image(&self.graphics_context, &frame)?;
        image.save("output_mp4.png")?;

        self.deregister_raw_output()?;
        info!("Images saved");

        Ok(())
    }

    pub fn record_for(&mut self, duration: Duration) -> Result<()> {
        self.start_record(PathBuf::from("output.mp4"))?;
        self.alternate_scenes(duration)?;
        self.stop_record()?;
        std::thread::sleep(Duration::from_secs(1));

        Ok(())
    }
}
