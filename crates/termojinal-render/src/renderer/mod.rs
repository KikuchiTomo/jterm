//! wgpu-based GPU terminal renderer.
//!
//! Sets up the wgpu render pipeline and renders terminal cells as textured quads
//! using instanced rendering for efficiency.

mod gpu;
mod overlay;
mod pane;
mod text;
pub(crate) mod types;

use std::collections::HashMap;
use std::sync::Arc;

use crate::atlas::{Atlas, CellSize, FontConfig};
use crate::blur_renderer::BlurRenderer;
use crate::color_convert::{self, ThemePalette};
use crate::emoji_atlas::EmojiAtlas;
use crate::image_render::ImageRenderer;
use crate::rounded_rect_renderer::RoundedRectRenderer;

pub use types::{RenderError, ScrollbarGeometry};
use types::{CellInstance, PaneCache, PaneKey, Uniforms};

/// The GPU renderer for the terminal.
pub struct Renderer {
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) surface_config: wgpu::SurfaceConfiguration,
    pub(crate) render_pipeline: wgpu::RenderPipeline,
    pub(crate) bind_group_layout: wgpu::BindGroupLayout,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) uniform_buffer: wgpu::Buffer,
    pub(crate) instance_buffer: wgpu::Buffer,
    pub(crate) instance_capacity: usize,
    pub(crate) atlas: Atlas,
    pub(crate) atlas_texture: wgpu::Texture,
    pub(crate) atlas_texture_version: usize,
    /// Retained font config (logical sizes) for rebuilding atlas on font size / DPI change.
    pub(crate) font_config: FontConfig,
    /// Display scale factor (e.g. 2.0 for Retina, 1.0 for FHD).
    pub scale_factor: f32,
    /// Color emoji atlas (RGBA).
    pub(crate) emoji_atlas: EmojiAtlas,
    pub(crate) emoji_texture: wgpu::Texture,
    pub(crate) emoji_texture_version: usize,
    /// Whether the cursor blink is in the "on" state.
    pub cursor_blink_on: bool,
    /// Background opacity (0.0 = fully transparent, 1.0 = opaque).
    pub bg_opacity: f32,
    /// Terminal default background color (from theme config, replaces DEFAULT_BG).
    pub default_bg: [f32; 4],
    /// IME preedit background color.
    pub preedit_bg: [f32; 4],
    /// Scrollbar thumb opacity.
    pub scrollbar_thumb_opacity: f32,
    /// Scrollbar track opacity.
    pub scrollbar_track_opacity: f32,
    /// Fixed scrollbar width in physical pixels.
    pub scrollbar_width_px: f32,
    /// Theme palette for ANSI 16-color overrides and default fg/bg.
    pub theme_palette: ThemePalette,

    // --- Dirty rendering: per-pane cache ---
    pub(crate) pane_caches: HashMap<PaneKey, PaneCache>,
    /// The pane key currently active (for single-pane render() calls).
    pub(crate) current_pane_key: PaneKey,
    /// Separate instance buffer for preedit overlay rendering.
    /// Using a dedicated buffer prevents preedit rendering from
    /// invalidating the main pane cache (which causes full rebuilds
    /// and display corruption during long sessions).
    pub(crate) preedit_instance_buffer: wgpu::Buffer,
    pub(crate) preedit_instance_capacity: usize,
    /// Image renderer for inline terminal images (Kitty/iTerm2/Sixel).
    pub(crate) image_renderer: ImageRenderer,
    /// SDF-based rounded rectangle renderer for overlays (command palette, etc.).
    pub rounded_rect_renderer: RoundedRectRenderer,
    /// Two-pass Gaussian blur renderer for frosted-glass background effects.
    pub blur_renderer: BlurRenderer,
    /// The surface texture format (retained for recreating pipelines on format change).
    pub(crate) surface_format: wgpu::TextureFormat,
    /// Whether to use CJK-aware character width calculation.
    pub cjk_width: bool,
}

impl Renderer {
    /// Create a new renderer for the given window.
    ///
    /// Takes `Arc<Window>` because the wgpu surface requires `'static` lifetime.
    pub async fn new(
        window: Arc<winit::window::Window>,
        font_config: &FontConfig,
    ) -> Result<Self, RenderError> {
        // Create wgpu instance.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..Default::default()
        });

        // Create surface. Arc<Window> is 'static so the surface can own it.
        let surface = instance.create_surface(window.clone())?;

        // Request adapter.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or(RenderError::AdapterNotFound)?;

        // Request device.
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("termojinal device"),
                    ..Default::default()
                },
                None,
            )
            .await?;

        // Configure surface.
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: {
                let caps = surface.get_capabilities(&adapter);
                if caps
                    .alpha_modes
                    .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
                {
                    wgpu::CompositeAlphaMode::PostMultiplied
                } else if caps
                    .alpha_modes
                    .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
                {
                    wgpu::CompositeAlphaMode::PreMultiplied
                } else {
                    wgpu::CompositeAlphaMode::Auto
                }
            },
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        // Build font atlas. Config font size is in logical points.
        // fontdue rasterizes in physical pixels, so scale by DPI factor.
        let scale = window.scale_factor() as f32;
        let scaled_config = FontConfig {
            size: font_config.size * scale,
            ..font_config.clone()
        };
        let atlas = Atlas::new(&scaled_config)?;

        // Create atlas texture.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload atlas data.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Build emoji atlas (use scaled font size to match atlas cell dimensions).
        let emoji_atlas = EmojiAtlas::new(
            atlas.cell_size.width as u32,
            atlas.cell_size.height as u32,
            scaled_config.size,
        );

        // Create emoji atlas texture (RGBA8).
        let emoji_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("emoji atlas"),
            size: wgpu::Extent3d {
                width: emoji_atlas.width,
                height: emoji_atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload initial (empty) emoji atlas data.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &emoji_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &emoji_atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(emoji_atlas.width * 4),
                rows_per_image: Some(emoji_atlas.height),
            },
            wgpu::Extent3d {
                width: emoji_atlas.width,
                height: emoji_atlas.height,
                depth_or_array_layers: 1,
            },
        );

        let emoji_view = emoji_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let emoji_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("emoji sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Uniform buffer.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Instance buffer — start with space for 80x24 cells.
        let initial_capacity = 80 * 24;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell instances"),
            size: (initial_capacity * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout (5 entries: uniform, atlas texture, atlas sampler,
        // emoji texture, emoji sampler).
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
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

        // Shader module.
        let shader_source = include_str!("../shader.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Instance buffer layout.
        let instance_layout = Self::instance_buffer_layout();

        // Render pipeline.
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[instance_layout],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let atlas_glyph_count = atlas.glyph_count();

        // Create image renderer for inline terminal images.
        let image_renderer = ImageRenderer::new(&device, surface_format);

        // Create rounded rectangle renderer for overlay UI.
        let rounded_rect_renderer = RoundedRectRenderer::new(&device, surface_format);

        // Create blur renderer for frosted-glass effects.
        let blur_renderer = BlurRenderer::new(&device, surface_format);

        // Create a dedicated instance buffer for preedit overlay rendering.
        let preedit_cap = 256;
        let preedit_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preedit instances"),
            size: (preedit_cap * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            adapter,
            device,
            queue,
            surface,
            surface_config,
            render_pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            instance_buffer,
            instance_capacity: initial_capacity,
            atlas,
            atlas_texture,
            atlas_texture_version: atlas_glyph_count,
            font_config: font_config.clone(),
            scale_factor: scale,
            emoji_atlas,
            emoji_texture,
            emoji_texture_version: 0,
            cursor_blink_on: true,
            bg_opacity: 1.0,
            default_bg: color_convert::DEFAULT_BG,
            preedit_bg: [0.15, 0.15, 0.20, 1.0],
            scrollbar_thumb_opacity: 0.5,
            scrollbar_track_opacity: 0.1,
            scrollbar_width_px: 8.0,
            theme_palette: ThemePalette::default(),
            pane_caches: HashMap::new(),
            current_pane_key: 0,
            preedit_instance_buffer,
            preedit_instance_capacity: preedit_cap,
            image_renderer,
            rounded_rect_renderer,
            blur_renderer,
            surface_format,
            cjk_width: false,
        })
    }

    /// Change the font size by recreating the atlas and emoji atlas with the new size.
    ///
    /// After calling this, all panes must be resized (since cell dimensions change).
    /// Change the logical font size (in points). Rebuilds the atlas at `size * scale_factor`.
    pub fn set_font_size(&mut self, size: f32) -> Result<(), RenderError> {
        self.font_config = FontConfig {
            size,
            ..self.font_config.clone()
        };
        let scaled_config = FontConfig {
            size: size * self.scale_factor,
            ..self.font_config.clone()
        };
        let mut new_atlas = Atlas::new(&scaled_config)?;
        new_atlas.cjk_width = self.cjk_width;
        self.atlas = new_atlas;

        // Recreate atlas texture with the new atlas dimensions.
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

        // Upload new atlas data.
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
        self.atlas_texture_version = self.atlas.glyph_count();

        // Recreate emoji atlas with new cell dimensions (use scaled font size).
        self.emoji_atlas = EmojiAtlas::new(
            self.atlas.cell_size.width as u32,
            self.atlas.cell_size.height as u32,
            size * self.scale_factor,
        );

        // Recreate emoji texture.
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

        // Upload emoji atlas data.
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
        self.emoji_texture_version = 0;

        // Recreate bind group with new texture views.
        self.recreate_bind_group();

        // Invalidate all caches.
        self.pane_caches.clear();

        log::info!("font size changed to {size}");
        Ok(())
    }

    /// Set the present mode (e.g., for ProMotion 120Hz displays).
    /// Try to set a present mode. Returns true if the mode is supported.
    pub fn try_set_present_mode(&mut self, mode: wgpu::PresentMode) -> bool {
        let caps = self.surface.get_capabilities(&self.adapter);
        if caps.present_modes.contains(&mode) {
            self.surface_config.present_mode = mode;
            self.surface.configure(&self.device, &self.surface_config);
            true
        } else {
            false
        }
    }

    /// Update the theme palette for live theme switching.
    ///
    /// Replaces the current palette and clears all per-pane render caches
    /// so that the next frame is fully re-rendered with the new colors.
    pub fn set_theme(&mut self, palette: ThemePalette) {
        self.default_bg = palette.bg;
        self.theme_palette = palette;
        self.pane_caches.clear();
    }

    /// Handle a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.pane_caches.clear();
    }

    /// Get the cell size in pixels.
    pub fn cell_size(&self) -> CellSize {
        self.atlas.cell_size
    }

    /// Set CJK ambiguous width mode on the atlas.
    pub fn atlas_set_cjk_width(&mut self, cjk: bool) {
        self.atlas.cjk_width = cjk;
    }

    /// Calculate grid dimensions with padding (for single-pane / full-surface).
    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let cw = self.atlas.cell_size.width;
        let ch = self.atlas.cell_size.height;
        let usable_w = (width as f32) - 2.0 * cw;
        let usable_h = (height as f32) - ch;
        let cols = (usable_w / cw).floor().max(1.0) as u16;
        let rows = (usable_h / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Calculate grid dimensions without padding (for multi-pane viewports).
    pub fn grid_size_raw(&self, width: u32, height: u32) -> (u16, u16) {
        let cw = self.atlas.cell_size.width;
        let ch = self.atlas.cell_size.height;
        let cols = (width as f32 / cw).floor().max(1.0) as u16;
        let rows = (height as f32 / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Synchronize GPU image textures with the terminal's image store.
    ///
    /// Call this before rendering when the image store has been modified
    /// (i.e., when `image_store.take_dirty()` returns true).
    pub fn sync_images(&mut self, store: &termojinal_vt::ImageStore) {
        self.image_renderer
            .sync_images(&self.device, &self.queue, store);
    }

    /// Get a mutable reference to the image renderer.
    pub fn image_renderer_mut(&mut self) -> &mut ImageRenderer {
        &mut self.image_renderer
    }

    /// Get the surface format used by this renderer.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    /// Get the current surface dimensions in physical pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }
}
