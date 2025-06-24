use anyhow::{anyhow, ensure, Result};
use compositor_pipeline::pipeline::GraphicsContext;
use image::{ImageBuffer, RgbaImage};
use wgpu::*;

const U8_MEM_SIZE: usize = std::mem::size_of::<u8>();

/// Converts any texture to a specified format.
///
/// Works by creating a destination texture with the desired format,
/// and using a shader to copy the source one into it.
pub fn convert_to(
    context: &GraphicsContext,
    source: &Texture,
    format: TextureFormat,
) -> Result<Texture> {
    let src_view = source.create_view(&TextureViewDescriptor::default());
    let src_size = source.size();

    // Create destination texture
    let dst_texture = context.device.create_texture(&TextureDescriptor {
        label: Some("Converted RGBA8Unorm Texture"),
        size: src_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let dst_view = dst_texture.create_view(&TextureViewDescriptor::default());

    // Shaders
    let shader = context.device.create_shader_module(ShaderModuleDescriptor {
        label: Some("Conversion Shader"),
        source: ShaderSource::Wgsl(include_str!("convert_shader.wgsl").into()),
    });

    // Bind group layout and pipeline
    let bind_group_layout = context
        .device
        .create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Conversion BGL"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled: false,
                        view_dimension: TextureViewDimension::D2,
                        sample_type: TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

    let pipeline_layout = context
        .device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Conversion Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

    let render_pipeline = context
        .device
        .create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Conversion Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: TextureFormat::Rgba8Unorm,
                    blend: Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

    // Sampler
    let sampler = context.device.create_sampler(&SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let bind_group = context.device.create_bind_group(&BindGroupDescriptor {
        label: Some("Conversion Bind Group"),
        layout: &bind_group_layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&src_view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(&sampler),
            },
        ],
    });

    // Render pass
    let mut encoder = context
        .device
        .create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Conversion Encoder"),
        });

    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Conversion Pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &dst_view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&render_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    context.queue.submit(Some(encoder.finish()));

    Ok(dst_texture)
}

/// Compute the number of byter per row for a texture, considering padding for alignment.
fn padded_bytes_per_row(texture: &Texture) -> Result<u32> {
    let format = texture.format();
    ensure!(
        format == TextureFormat::Rgba8Unorm || format == TextureFormat::Rgba8UnormSrgb,
        "Can only compute padding for Rgba8Unorm or Rgba8UnormSrgb textures, got {:?}",
        format
    );

    let texture_size = texture.size();
    let unaligned_bytes_per_row = texture_size.width * U8_MEM_SIZE as u32 * 4;

    let padded_bytes_per_row = ((unaligned_bytes_per_row + COPY_BYTES_PER_ROW_ALIGNMENT - 1)
        / COPY_BYTES_PER_ROW_ALIGNMENT)
        * COPY_BYTES_PER_ROW_ALIGNMENT;

    Ok(padded_bytes_per_row)
}

/// Converts a Wgpu texture to an image buffer (RgbaImage).
pub fn to_image(context: &GraphicsContext, texture: &Texture) -> Result<RgbaImage> {
    // The image crate "assumes an sRGB color space of its data".
    // Before copying pixel data, we need to ensure the texture is in sRGB color space.
    let target_format = TextureFormat::Rgba8UnormSrgb;
    let texture = match texture.format() {
        format if format == target_format => texture.clone(),
        _ => convert_to(context, texture, target_format)?,
    };

    let texture_size = texture.size();
    let padded_bytes_per_row = padded_bytes_per_row(&texture)?;
    let buffer_size = padded_bytes_per_row * texture_size.height;

    let buffer = context.device.create_buffer(&BufferDescriptor {
        label: Some("Save texture buffer"),
        size: buffer_size as u64,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = context
        .device
        .create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Save texture encoder"),
        });

    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: Origin3d { x: 0, y: 0, z: 0 },
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(texture_size.height),
            },
        },
        texture_size,
    );
    context.queue.submit(Some(encoder.finish()));

    let buffer_slice = buffer.slice(..);
    buffer_slice.map_async(MapMode::Read, |_| ());
    context.device.poll(Maintain::Wait);

    let data = buffer_slice.get_mapped_range();

    // Allocate the final image data, copying each row without the extra padding
    let mut image_data =
        Vec::with_capacity((texture_size.width * texture_size.height * 4) as usize);
    for chunk in data.chunks(padded_bytes_per_row as usize) {
        image_data.extend_from_slice(&chunk[..(texture_size.width * 4) as usize]);
    }

    ImageBuffer::from_raw(texture_size.width, texture_size.height, image_data)
        .ok_or(anyhow!("Failed to create image buffer"))
}
