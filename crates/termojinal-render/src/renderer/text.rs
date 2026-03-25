//! Text rendering at arbitrary pixel positions.

use super::types::*;
use super::Renderer;
use crate::emoji_atlas;

impl Renderer {
    /// Render a string of text at a specific pixel position on the surface.
    ///
    /// Each character is rendered as one cell instance. The text is positioned
    /// at `(px_x, px_y)` in physical pixel coordinates (top-left origin).
    /// `clip_rect` optionally overrides the scissor rect as `(x, y, w, h)`.
    pub fn render_text(
        &mut self,
        view: &wgpu::TextureView,
        text: &str,
        px_x: f32,
        px_y: f32,
        fg: [f32; 4],
        bg: [f32; 4],
    ) {
        self.render_text_clipped(view, text, px_x, px_y, fg, bg, None);
    }

    /// Like `render_text` but with an optional explicit scissor clip rect.
    pub fn render_text_clipped(
        &mut self,
        view: &wgpu::TextureView,
        text: &str,
        px_x: f32,
        px_y: f32,
        fg: [f32; 4],
        bg: [f32; 4],
        clip_rect: Option<(u32, u32, u32, u32)>,
    ) {
        if text.is_empty() {
            return;
        }

        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // Build one instance per character.
        let mut instances = Vec::with_capacity(text.len());
        let mut col = 0usize;
        for c in text.chars() {
            let (glyph, is_emoji_cell) = if emoji_atlas::is_emoji(c) {
                if let Some(eg) = self.emoji_atlas.get_glyph(c) {
                    (eg, true)
                } else {
                    (self.atlas.get_glyph(c), false)
                }
            } else {
                let mono_glyph = self.atlas.get_glyph(c);
                let try_emoji_fallback = (c > ' '
                    && !c.is_control()
                    && mono_glyph.atlas_w > 0.0
                    && self.atlas.is_glyph_empty(c))
                    || emoji_atlas::is_text_emoji(c);
                if try_emoji_fallback {
                    if let Some(eg) = self.emoji_atlas.get_glyph(c) {
                        (eg, true)
                    } else {
                        (mono_glyph, false)
                    }
                } else {
                    (mono_glyph, false)
                }
            };
            let cw = termojinal_vt::char_width(c, self.cjk_width);
            let width_scale = if cw > 1 { cw as f32 } else { 1.0 };
            let flags = if is_emoji_cell { FLAG_EMOJI } else { 0 };
            instances.push(CellInstance {
                grid_pos: [col as f32, 0.0],
                atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                fg_color: fg,
                bg_color: bg,
                flags,
                cell_width_scale: width_scale,
                _pad: [0; 2],
            });
            col += cw;
        }

        if instances.is_empty() {
            return;
        }

        // Re-upload emoji atlas if new glyphs were rasterized.
        let current_emoji_count = self.emoji_atlas.glyph_count();
        if current_emoji_count != self.emoji_texture_version {
            self.reupload_emoji_atlas();
            self.emoji_texture_version = current_emoji_count;
        }

        // Re-upload atlas if new glyphs were rasterized.
        let current_glyph_count = self.atlas.glyph_count();
        if current_glyph_count != self.atlas_texture_version {
            self.reupload_atlas();
            self.atlas_texture_version = current_glyph_count;
        }

        // Re-upload emoji atlas if needed.
        let current_emoji_count = self.emoji_atlas.glyph_count();
        if current_emoji_count != self.emoji_texture_version {
            self.reupload_emoji_atlas();
            self.emoji_texture_version = current_emoji_count;
        }

        // Compute NDC positioning for the text origin.
        let ndc_x = (px_x / surface_w) * 2.0 - 1.0;
        let ndc_y = 1.0 - (px_y / surface_h) * 2.0;
        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let text_uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [ndc_x, ndc_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [
                self.emoji_atlas.width as f32,
                self.emoji_atlas.height as f32,
            ],
            cursor_pos: [0.0; 4],
            cursor_extra: [0.0; 4],
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&text_uniforms));

        // Ensure instance buffer is large enough.
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cell instances"),
                size: (self.instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("text encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Clip to the text region (or custom clip rect).
            let (clip_x, clip_y, clip_w, clip_h) = if let Some((cx, cy, cw, ch)) = clip_rect {
                (cx, cy, cw, ch)
            } else {
                let text_width = (col as f32 * cell_w).ceil() as u32;
                let text_height = cell_h.ceil() as u32;
                (px_x as u32, px_y as u32, text_width, text_height)
            };
            render_pass.set_scissor_rect(
                clip_x.min(surface_w as u32),
                clip_y.min(surface_h as u32),
                clip_w.min(surface_w as u32 - clip_x.min(surface_w as u32)),
                clip_h.min(surface_h as u32 - clip_y.min(surface_h as u32)),
            );

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Invalidate pane caches since we overwrote the instance buffer.
        self.pane_caches.clear();
    }
}
