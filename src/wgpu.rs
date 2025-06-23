use anyhow::{anyhow, ensure, Result};
use compositor_pipeline::pipeline::GraphicsContext;
use image::{ImageBuffer, RgbaImage};
use wgpu::*;

const U8_MEM_SIZE: usize = std::mem::size_of::<u8>();
const COMPATIBLE_FORMATS: [TextureFormat; 2] =
    [TextureFormat::Rgba8Unorm, TextureFormat::Rgba8UnormSrgb];

fn padded_bytes_per_row(texture: &Texture) -> Result<u32> {
    ensure!(
        COMPATIBLE_FORMATS.contains(&texture.format()),
        "Can only compute padding for {:?} textures, got {:?}",
        COMPATIBLE_FORMATS,
        texture.format()
    );

    let texture_size = texture.size();
    let unaligned_bytes_per_row = texture_size.width * U8_MEM_SIZE as u32 * 4;

    let padded_bytes_per_row = ((unaligned_bytes_per_row + COPY_BYTES_PER_ROW_ALIGNMENT - 1)
        / COPY_BYTES_PER_ROW_ALIGNMENT)
        * COPY_BYTES_PER_ROW_ALIGNMENT;

    Ok(padded_bytes_per_row)
}

pub fn to_image(graphics_context: &GraphicsContext, texture: &wgpu::Texture) -> Result<RgbaImage> {
    ensure!(
        COMPATIBLE_FORMATS.contains(&texture.format()),
        "Can only save {:?} formatted textures, got {:?}",
        COMPATIBLE_FORMATS,
        texture.format()
    );

    let texture_size = texture.size();
    let padded_bytes_per_row = padded_bytes_per_row(texture)?;
    let buffer_size = padded_bytes_per_row * texture_size.height;

    let buffer = graphics_context
        .device
        .create_buffer(&wgpu::BufferDescriptor {
            label: Some("Save texture buffer"),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

    let mut encoder =
        graphics_context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Save texture encoder"),
            });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(texture_size.height),
            },
        },
        texture_size,
    );
    graphics_context.queue.submit(Some(encoder.finish()));

    let buffer_slice = buffer.slice(..);
    buffer_slice.map_async(wgpu::MapMode::Read, |_| ());
    graphics_context.device.poll(wgpu::Maintain::Wait);

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
