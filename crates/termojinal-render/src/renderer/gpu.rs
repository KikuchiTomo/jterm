//! GPU resource management: atlas re-upload, bind group recreation, pipeline generation.

use super::types::CellInstance;
use super::Renderer;

impl Renderer {
    /// Re-upload the atlas texture to the GPU (e.g., after new glyphs are rasterized).
    pub(crate) fn reupload_atlas(&mut self) {
        // Check if atlas size changed (it may have grown).
        let needs_recreate = self.atlas.width != self.atlas_texture.width()
            || self.atlas.height != self.atlas_texture.height();

        if needs_recreate {
            self.atlas_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("atlas"),
                size: wgpu::Extent3d {
                    width: self.atlas.width,
                    height: self.atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            self.recreate_bind_group();
        }

        // Upload atlas data.
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.atlas.width),
                rows_per_image: Some(self.atlas.height),
            },
            wgpu::Extent3d {
                width: self.atlas.width,
                height: self.atlas.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Re-upload the emoji atlas texture to the GPU.
    pub(crate) fn reupload_emoji_atlas(&mut self) {
        let needs_recreate = self.emoji_atlas.width != self.emoji_texture.width()
            || self.emoji_atlas.height != self.emoji_texture.height();

        if needs_recreate {
            self.emoji_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("emoji atlas"),
                size: wgpu::Extent3d {
                    width: self.emoji_atlas.width,
                    height: self.emoji_atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            self.recreate_bind_group();
        }

        // Upload emoji atlas data (RGBA, 4 bytes per pixel).
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.emoji_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.emoji_atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.emoji_atlas.width * 4),
                rows_per_image: Some(self.emoji_atlas.height),
            },
            wgpu::Extent3d {
                width: self.emoji_atlas.width,
                height: self.emoji_atlas.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Recreate the bind group with current atlas and emoji texture views.
    pub(crate) fn recreate_bind_group(&mut self) {
        let atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let emoji_view = self
            .emoji_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let emoji_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("emoji sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&emoji_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&emoji_sampler),
                },
            ],
        });
    }

    /// Create the instance buffer vertex layout descriptor.
    pub(crate) fn instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CellInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                // grid_pos: vec2<f32> at location(0)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // atlas_uv: vec4<f32> at location(1)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 8,
                    shader_location: 1,
                },
                // fg_color: vec4<f32> at location(2)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 24,
                    shader_location: 2,
                },
                // bg_color: vec4<f32> at location(3)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 40,
                    shader_location: 3,
                },
                // flags: u32 at location(4)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 56,
                    shader_location: 4,
                },
                // cell_width_scale: f32 at location(5)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 60,
                    shader_location: 5,
                },
            ],
        }
    }
}
