//! Overlay rendering: preedit, separators, rounded rectangles, blur.

use super::types::*;
use super::Renderer;
use crate::emoji_atlas;
use crate::rounded_rect_renderer::RoundedRect;

impl Renderer {
    /// Render IME preedit (composition) text as underlined overlay cells at the
    /// terminal cursor position. Issues its own draw call with a separate
    /// encoder+submit so it works regardless of the main render path.
    pub(crate) fn render_preedit_overlay(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        text: &str,
        viewport: Option<(u32, u32, u32, u32)>,
        view: &wgpu::TextureView,
    ) {
        if text.is_empty() {
            return;
        }

        let cursor_col = terminal.cursor_col;
        let cursor_row = terminal.cursor_row;

        let fg = self.theme_palette.fg;
        let bg = self.preedit_bg;

        let mut col_offset: usize = 0;
        let mut preedit_instances = Vec::new();

        for ch in text.chars() {
            // Skip zero-width characters (variation selectors, ZWJ, etc.)
            // that have no visual representation and would cause empty glyph
            // lookups or layout issues.
            if emoji_atlas::is_zero_width_for_render(ch) {
                continue;
            }

            let cw = termojinal_vt::char_width(ch, self.cjk_width);
            // Try emoji atlas for emoji / text-emoji characters, mono atlas otherwise.
            let (glyph, is_emoji_cell) =
                if emoji_atlas::is_emoji(ch) || emoji_atlas::is_text_emoji(ch) {
                    if let Some(eg) = self.emoji_atlas.get_glyph(ch) {
                        (eg, true)
                    } else {
                        (self.atlas.get_glyph(ch), false)
                    }
                } else {
                    (self.atlas.get_glyph(ch), false)
                };

            let width_scale = if cw > 1 { cw as f32 } else { 1.0 };
            let flags = FLAG_UNDERLINE | if is_emoji_cell { FLAG_EMOJI } else { 0 };
            preedit_instances.push(CellInstance {
                grid_pos: [(cursor_col + col_offset) as f32, cursor_row as f32],
                atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                fg_color: fg,
                bg_color: bg,
                flags,
                cell_width_scale: width_scale,
                _pad: [0; 2],
            });

            col_offset += cw;
        }

        if preedit_instances.is_empty() {
            return;
        }

        // Re-upload atlas if new glyphs were rasterized for preedit characters.
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

        let count = preedit_instances.len();

        // Use the dedicated preedit instance buffer to avoid invalidating
        // the main pane cache (which would force expensive full rebuilds).
        if count > self.preedit_instance_capacity {
            self.preedit_instance_capacity = count.next_power_of_two();
            self.preedit_instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("preedit instances"),
                size: (self.preedit_instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Upload preedit instances to the dedicated buffer.
        self.queue.write_buffer(
            &self.preedit_instance_buffer,
            0,
            bytemuck::cast_slice(&preedit_instances),
        );

        // Compute uniforms matching the current render mode.
        let uniforms = match viewport {
            Some((vp_x, vp_y, vp_w, vp_h)) => {
                self.compute_uniforms_viewport(terminal, vp_x, vp_y, vp_w, vp_h)
            }
            None => self.compute_uniforms_full(terminal),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Issue a draw call for the preedit instances.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preedit encoder"),
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preedit render pass"),
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
            if let Some((vp_x, vp_y, vp_w, vp_h)) = viewport {
                render_pass.set_scissor_rect(vp_x, vp_y, vp_w, vp_h);
            }
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.preedit_instance_buffer.slice(..));
            render_pass.draw(0..6, 0..count as u32);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Submit a separator draw. Call after all panes are rendered.
    pub fn submit_separator(
        &mut self,
        view: &wgpu::TextureView,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [f32; 4],
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("separator encoder"),
            });
        self.draw_separator(&mut encoder, view, x, y, width, height, color);
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw a 1px separator line at the given position.
    ///
    /// The separator is rendered as one or more background-only quads
    /// positioned at the given pixel coordinates.
    pub fn draw_separator(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [f32; 4],
    ) {
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // How many cell-sized quads do we need to cover the separator?
        let cells_needed_x = ((width as f32) / cell_w).ceil() as usize;
        let cells_needed_y = ((height as f32) / cell_h).ceil() as usize;
        let num_quads = cells_needed_x.max(1) * cells_needed_y.max(1);

        let space_glyph = self.atlas.get_glyph(' ');

        // Build separator instances. We position them using NDC coordinates
        // directly by computing grid_pos such that grid_pos * cell_size + offset
        // places them at the right pixel location.
        //
        // We'll set up a custom uniform for this pass where grid_offset is
        // at the separator origin.
        let sep_ndc_x = (x as f32 / surface_w) * 2.0 - 1.0;
        let sep_ndc_y = 1.0 - (y as f32 / surface_h) * 2.0;

        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let mut instances = Vec::with_capacity(num_quads);
        for iy in 0..cells_needed_y.max(1) {
            for ix in 0..cells_needed_x.max(1) {
                instances.push(CellInstance {
                    grid_pos: [ix as f32, iy as f32],
                    atlas_uv: [
                        space_glyph.atlas_x,
                        space_glyph.atlas_y,
                        space_glyph.atlas_w,
                        space_glyph.atlas_h,
                    ],
                    fg_color: color,
                    bg_color: color,
                    flags: 0,
                    cell_width_scale: 1.0,
                    _pad: [0; 2],
                });
            }
        }

        // We need to temporarily set uniforms for the separator.
        let sep_uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [sep_ndc_x, sep_ndc_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [
                self.emoji_atlas.width as f32,
                self.emoji_atlas.height as f32,
            ],
            cursor_pos: [0.0; 4],
            cursor_extra: [0.0; 4],
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&sep_uniforms));

        // Ensure instance buffer is large enough for the separator quads.
        // (We temporarily use the instance buffer; in production, a separate
        // buffer would be cleaner, but this works for the separator use case.)
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

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("separator render pass"),
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

            // Clip to the separator region.
            render_pass.set_scissor_rect(x, y, width, height);
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }

        // Invalidate all pane caches since we overwrote the instance buffer.
        self.pane_caches.clear();
    }

    // -----------------------------------------------------------------------
    // Overlay rendering API (rounded rects + blur)
    // -----------------------------------------------------------------------

    /// Render rounded rectangle overlays (e.g., command palette background).
    pub fn render_rounded_rects(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rects: &[RoundedRect],
    ) {
        let screen_width = self.surface_config.width as f32;
        let screen_height = self.surface_config.height as f32;
        self.rounded_rect_renderer.render(
            encoder,
            view,
            &self.device,
            &self.queue,
            screen_width,
            screen_height,
            rects,
        );
    }

    /// Submit rounded rectangle overlays immediately (creates its own encoder).
    ///
    /// Convenience wrapper around [`Self::render_rounded_rects`] that mirrors
    /// the pattern of [`Self::submit_separator`].
    pub fn submit_rounded_rects(&mut self, view: &wgpu::TextureView, rects: &[RoundedRect]) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rounded_rect encoder"),
            });
        self.render_rounded_rects(&mut encoder, view, rects);
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Apply a two-pass Gaussian blur to the framebuffer.
    pub fn blur_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        radius: f32,
    ) {
        let width = self.surface_config.width;
        let height = self.surface_config.height;
        self.blur_renderer.blur(
            encoder,
            &self.device,
            &self.queue,
            source,
            target,
            radius,
            width,
            height,
        );
    }
}
