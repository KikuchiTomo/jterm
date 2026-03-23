//! Font atlas — CPU glyph rasterization + texture packing.
//!
//! Each glyph is rasterized into a **cell-sized** bitmap with the glyph
//! placed at the correct position using font metrics (bearing). The shader
//! can then map the cell quad directly to the atlas region without worrying
//! about glyph positioning.

use std::collections::HashMap;

/// Configuration for loading the font.
#[derive(Debug, Clone)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::from("monospace"),
            size: 16.0,
            line_height: 1.2,
        }
    }
}

/// Cell dimensions derived from font metrics.
#[derive(Debug, Clone, Copy)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
    /// Font ascent (distance from baseline to top of tallest glyph).
    pub ascent: f32,
    /// Font descent (negative, distance from baseline to bottom of lowest glyph).
    pub descent: f32,
}

/// UV region within the atlas for a single glyph (in texel coordinates).
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    pub atlas_x: f32,
    pub atlas_y: f32,
    pub atlas_w: f32,
    pub atlas_h: f32,
    pub bearing_x: f32,
    pub bearing_y: f32,
}

/// A font atlas that maps characters to UV regions in a texture.
pub struct Atlas {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    glyphs: HashMap<char, GlyphInfo>,
    pub cell_size: CellSize,
    font: fontdue::Font,
    fallback_font: Option<fontdue::Font>,
    cjk_font: Option<fontdue::Font>,
    symbols_font: Option<fontdue::Font>,
    font_size: f32,
    ascent: f32,
    cell_w: u32,
    cell_h: u32,
    pack_x: u32,
    pack_y: u32,
    pack_row_height: u32,
    /// Whether to use CJK-aware character width calculation.
    pub cjk_width: bool,
}

impl Atlas {
    pub fn new(config: &FontConfig) -> Result<Self, AtlasError> {
        let font_data = Self::load_font_data(&config.family)?;
        let font = fontdue::Font::from_bytes(
            font_data.as_slice(),
            fontdue::FontSettings::default(),
        )
        .map_err(|e| AtlasError::FontParsing(e.to_string()))?;

        let line_metrics = font
            .horizontal_line_metrics(config.size)
            .ok_or(AtlasError::FontParsing(
                "no horizontal line metrics".to_string(),
            ))?;

        let ascent = line_metrics.ascent;
        let descent = line_metrics.descent;
        let natural_height = ascent - descent;
        let cell_height = (natural_height * config.line_height).ceil();

        let (m_metrics, _) = font.rasterize('M', config.size);
        let cell_width = m_metrics.advance_width.ceil();

        let cell_size = CellSize {
            width: cell_width,
            height: cell_height,
            ascent,
            descent,
        };

        let cell_w = cell_width as u32;
        let cell_h = cell_height as u32;

        log::info!(
            "font metrics: ascent={ascent:.1}, descent={descent:.1}, \
             cell={}x{}, size={}",
            cell_w, cell_h, config.size
        );

        // Try to load a Nerd Font as fallback for PUA / box-drawing glyphs.
        let fallback_font = Self::load_fallback_nerd_font();
        // Try to load a CJK font as fallback for Japanese/Chinese/Korean characters.
        let cjk_font = Self::load_cjk_fallback_font();
        // Try to load a symbols font as fallback for Braille, geometric shapes,
        // misc symbols, arrows, etc. that the primary monospace font may lack.
        let symbols_font = Self::load_symbols_fallback_font();

        let atlas_width = 1024u32;
        let atlas_height = 1024u32;
        let data = vec![0u8; (atlas_width * atlas_height) as usize];

        let mut atlas = Self {
            data,
            width: atlas_width,
            height: atlas_height,
            glyphs: HashMap::new(),
            cell_size,
            font,
            fallback_font,
            cjk_font,
            symbols_font,
            font_size: config.size,
            // Shift baseline down by half the line_height extra space so text is
            // vertically centered in the cell, not top-aligned.
            ascent: ascent + (cell_height - natural_height) / 2.0,
            cell_w,
            cell_h,
            pack_x: 1,
            pack_y: 1,
            pack_row_height: 0,
            cjk_width: false,
        };

        // Pre-rasterize ASCII printable characters.
        for c in (32u8..=126).map(|b| b as char) {
            atlas.rasterize_glyph(c);
        }

        Ok(atlas)
    }

    pub fn get_glyph(&mut self, c: char) -> GlyphInfo {
        if let Some(&info) = self.glyphs.get(&c) {
            return info;
        }
        self.rasterize_glyph(c)
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.glyphs.contains_key(&c)
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Try to draw block elements, shade characters, and box-drawing characters
    /// procedurally. Returns None if the character is not handled.
    fn try_procedural_block(&mut self, c: char) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let hw = w / 2; // half width
        let hh = h / 2; // half height

        // --- Shade characters ---
        let shade = match c {
            '░' => Some(64u8),    // LIGHT SHADE ~25%
            '▒' => Some(128u8),   // MEDIUM SHADE ~50%
            '▓' => Some(192u8),   // DARK SHADE ~75%
            _ => None,
        };
        if let Some(alpha) = shade {
            let mut bitmap = vec![0u8; (w * h) as usize];
            for pixel in &mut bitmap {
                *pixel = alpha;
            }
            let info = self.pack_cell_bitmap(&bitmap, w, h);
            return Some(info);
        }

        // --- Box-drawing characters (U+2500–U+257F) ---
        // Draw lines that extend to the exact cell edges to ensure seamless joining.
        if c >= '\u{2500}' && c <= '\u{257F}' {
            return self.try_procedural_box_drawing(c);
        }

        // --- Block elements (U+2580–U+259F) ---
        let regions: Vec<(u32, u32, u32, u32)> = match c {
            '█' => vec![(0, 0, w, h)],           // FULL BLOCK
            '▀' => vec![(0, 0, w, hh)],          // UPPER HALF
            '▄' => vec![(0, hh, w, h)],          // LOWER HALF
            '▌' => vec![(0, 0, hw, h)],          // LEFT HALF
            '▐' => vec![(hw, 0, w, h)],          // RIGHT HALF
            '▖' => vec![(0, hh, hw, h)],         // QUADRANT LOWER LEFT
            '▗' => vec![(hw, hh, w, h)],         // QUADRANT LOWER RIGHT
            '▘' => vec![(0, 0, hw, hh)],         // QUADRANT UPPER LEFT
            '▝' => vec![(hw, 0, w, hh)],         // QUADRANT UPPER RIGHT
            '▙' => vec![(0, 0, hw, h), (hw, hh, w, h)],
            '▛' => vec![(0, 0, w, hh), (0, hh, hw, h)],
            '▜' => vec![(0, 0, w, hh), (hw, hh, w, h)],
            '▟' => vec![(hw, 0, w, hh), (0, hh, w, h)],
            _ => return None,
        };

        if regions.is_empty() {
            return None;
        }

        let mut bitmap = vec![0u8; (w * h) as usize];
        for (x0, y0, x1, y1) in &regions {
            for y in *y0..*y1 {
                for x in *x0..*x1 {
                    if x < w && y < h {
                        bitmap[(y * w + x) as usize] = 255;
                    }
                }
            }
        }
        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw box-drawing characters procedurally.
    /// Lines extend to the exact cell edges for seamless joining between cells.
    fn try_procedural_box_drawing(&mut self, c: char) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let cx = w / 2; // center x
        let cy = h / 2; // center y

        // Line thickness: thin = 1px, heavy = 2-3px depending on cell size.
        let thin = 1u32.max(w / 10);
        let heavy = (thin * 2).max(2).min(w / 4);

        // Segments: (left, right, up, down) with thickness.
        // 0 = none, 1 = thin, 2 = heavy, 3 = double
        let (left, right, up, down) = match c {
            '─' => (1, 1, 0, 0), // horizontal thin
            '━' => (2, 2, 0, 0), // horizontal heavy
            '│' => (0, 0, 1, 1), // vertical thin
            '┃' => (0, 0, 2, 2), // vertical heavy
            '┌' => (0, 1, 0, 1), // top-left thin
            '┐' => (1, 0, 0, 1), // top-right thin
            '└' => (0, 1, 1, 0), // bottom-left thin
            '┘' => (1, 0, 1, 0), // bottom-right thin
            '├' => (0, 1, 1, 1), // left-T thin
            '┤' => (1, 0, 1, 1), // right-T thin
            '┬' => (1, 1, 0, 1), // top-T thin
            '┴' => (1, 1, 1, 0), // bottom-T thin
            '┼' => (1, 1, 1, 1), // cross thin
            '╌' => (1, 1, 0, 0), // dashed horizontal (draw as solid)
            '╎' => (0, 0, 1, 1), // dashed vertical (draw as solid)
            '┄' => (1, 1, 0, 0), // triple-dash horizontal
            '┆' => (0, 0, 1, 1), // triple-dash vertical
            '┈' => (1, 1, 0, 0), // quad-dash horizontal
            '┊' => (0, 0, 1, 1), // quad-dash vertical
            // Heavy corners
            '┍' => (0, 2, 0, 1), '┎' => (0, 1, 0, 2),
            '┏' => (0, 2, 0, 2),
            '┑' => (2, 0, 0, 1), '┒' => (1, 0, 0, 2),
            '┓' => (2, 0, 0, 2),
            '┕' => (0, 2, 1, 0), '┖' => (0, 1, 2, 0),
            '┗' => (0, 2, 2, 0),
            '┙' => (2, 0, 1, 0), '┚' => (1, 0, 2, 0),
            '┛' => (2, 0, 2, 0),
            // Heavy T-junctions
            '┝' => (0, 2, 1, 1), '┞' => (0, 1, 2, 1), '┟' => (0, 1, 1, 2),
            '┠' => (0, 1, 2, 2), '┡' => (0, 2, 2, 1), '┢' => (0, 2, 1, 2),
            '┣' => (0, 2, 2, 2),
            '┥' => (2, 0, 1, 1), '┦' => (1, 0, 2, 1), '┧' => (1, 0, 1, 2),
            '┨' => (1, 0, 2, 2), '┩' => (2, 0, 2, 1), '┪' => (2, 0, 1, 2),
            '┫' => (2, 0, 2, 2),
            '┭' => (2, 1, 0, 1), '┮' => (1, 2, 0, 1), '┯' => (2, 2, 0, 1),
            '┰' => (1, 1, 0, 2), '┱' => (2, 1, 0, 2), '┲' => (1, 2, 0, 2),
            '┳' => (2, 2, 0, 2),
            '┵' => (2, 1, 1, 0), '┶' => (1, 2, 1, 0), '┷' => (2, 2, 1, 0),
            '┸' => (1, 1, 2, 0), '┹' => (2, 1, 2, 0), '┺' => (1, 2, 2, 0),
            '┻' => (2, 2, 2, 0),
            // Heavy crosses
            '┽' => (2, 1, 1, 1), '┾' => (1, 2, 1, 1), '┿' => (2, 2, 1, 1),
            '╀' => (1, 1, 2, 1), '╁' => (1, 1, 1, 2), '╂' => (1, 1, 2, 2),
            '╃' => (2, 1, 2, 1), '╄' => (1, 2, 2, 1), '╅' => (2, 1, 1, 2),
            '╆' => (1, 2, 1, 2), '╇' => (2, 2, 2, 1), '╈' => (2, 2, 1, 2),
            '╉' => (2, 1, 2, 2), '╊' => (1, 2, 2, 2),
            '╋' => (2, 2, 2, 2),
            // Double lines
            '═' => (3, 3, 0, 0), // double horizontal
            '║' => (0, 0, 3, 3), // double vertical
            '╔' => (0, 3, 0, 3), '╗' => (3, 0, 0, 3),
            '╚' => (0, 3, 3, 0), '╝' => (3, 0, 3, 0),
            '╠' => (0, 3, 3, 3), '╣' => (3, 0, 3, 3),
            '╦' => (3, 3, 0, 3), '╩' => (3, 3, 3, 0),
            '╬' => (3, 3, 3, 3),
            // Mixed single/double
            '╒' => (0, 3, 0, 1), '╓' => (0, 1, 0, 3),
            '╕' => (3, 0, 0, 1), '╖' => (1, 0, 0, 3),
            '╘' => (0, 3, 1, 0), '╙' => (0, 1, 3, 0),
            '╛' => (3, 0, 1, 0), '╜' => (1, 0, 3, 0),
            '╞' => (0, 3, 1, 1), '╟' => (0, 1, 3, 3),
            '╡' => (3, 0, 1, 1), '╢' => (1, 0, 3, 3),
            '╤' => (3, 3, 0, 1), '╥' => (1, 1, 0, 3),
            '╧' => (3, 3, 1, 0), '╨' => (1, 1, 3, 0),
            '╪' => (3, 3, 1, 1), '╫' => (1, 1, 3, 3),
            // Rounded corners
            '╭' => (0, 1, 0, 1), '╮' => (1, 0, 0, 1),
            '╯' => (1, 0, 1, 0), '╰' => (0, 1, 1, 0),
            _ => return None,
        };

        let mut bitmap = vec![0u8; (w * h) as usize];

        // Helper: draw a filled rect into bitmap
        let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32| {
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    bitmap[(y * w + x) as usize] = 255;
                }
            }
        };

        let draw_segment = |fill: &mut dyn FnMut(u32, u32, u32, u32), dir: u32, thickness: u32, is_double: bool| {
            if is_double {
                let gap = (thickness + 1).max(2);
                let t = thickness.max(1);
                match dir {
                    0 => { // left
                        fill(0, cy - gap, cx, cy - gap + t);
                        fill(0, cy + gap - t, cx, cy + gap);
                    }
                    1 => { // right
                        fill(cx, cy - gap, w, cy - gap + t);
                        fill(cx, cy + gap - t, w, cy + gap);
                    }
                    2 => { // up
                        fill(cx - gap, 0, cx - gap + t, cy);
                        fill(cx + gap - t, 0, cx + gap, cy);
                    }
                    3 => { // down
                        fill(cx - gap, cy, cx - gap + t, h);
                        fill(cx + gap - t, cy, cx + gap, h);
                    }
                    _ => {}
                }
            } else {
                let half_t = thickness / 2;
                match dir {
                    0 => fill(0, cy.saturating_sub(half_t), cx + half_t, cy + thickness - half_t), // left
                    1 => fill(cx.saturating_sub(half_t), cy.saturating_sub(half_t), w, cy + thickness - half_t), // right
                    2 => fill(cx.saturating_sub(half_t), 0, cx + thickness - half_t, cy + half_t), // up
                    3 => fill(cx.saturating_sub(half_t), cy.saturating_sub(half_t), cx + thickness - half_t, h), // down
                    _ => {}
                }
            }
        };

        // Draw each segment.
        let segments = [(0u32, left), (1, right), (2, up), (3, down)];
        for (dir, style) in segments {
            if style == 0 { continue; }
            let is_double = style == 3;
            let thickness = if style == 2 { heavy } else { thin };
            draw_segment(&mut fill, dir, thickness, is_double);
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Returns true if the character is in a range that may need a fallback font:
    /// Private Use Area (Nerd Font icons), box-drawing, block elements, CJK,
    /// Braille patterns, geometric shapes, miscellaneous symbols, dingbats, arrows,
    /// and other symbol blocks commonly used in terminal output.
    fn needs_fallback_check(c: char) -> bool {
        matches!(c,
            '\u{E000}'..='\u{F8FF}'   // BMP Private Use Area (Nerd Font icons)
            | '\u{F0000}'..='\u{FFFFF}' // Supplementary PUA-A
            | '\u{2500}'..='\u{257F}'  // Box-drawing characters
            | '\u{2580}'..='\u{259F}'  // Block elements
            | '\u{2190}'..='\u{21FF}'  // Arrows (←↑→↓⇐⇑⇒⇓ etc.)
            | '\u{2200}'..='\u{22FF}'  // Mathematical Operators (∞≠≤≥ etc.)
            | '\u{2300}'..='\u{23FF}'  // Miscellaneous Technical (⌘⌥⌫⏎ etc.)
            | '\u{2460}'..='\u{24FF}'  // Enclosed Alphanumerics (①② etc.)
            | '\u{25A0}'..='\u{25FF}'  // Geometric Shapes (■□▲△○●◆◇◯ etc.)
            | '\u{2600}'..='\u{26FF}'  // Miscellaneous Symbols (☀☁☂★☆♠♣♥♦ etc.)
            | '\u{2700}'..='\u{27BF}'  // Dingbats (✓✗✘✚✜ etc.)
            | '\u{27C0}'..='\u{27EF}'  // Misc Mathematical Symbols-A
            | '\u{27F0}'..='\u{27FF}'  // Supplemental Arrows-A
            | '\u{2800}'..='\u{28FF}'  // Braille Patterns (spinners: ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏)
            | '\u{2900}'..='\u{297F}'  // Supplemental Arrows-B
            | '\u{2B00}'..='\u{2BFF}'  // Misc Symbols and Arrows
            | '\u{3000}'..='\u{9FFF}'  // CJK Unified Ideographs + Hiragana/Katakana
            | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
            | '\u{AC00}'..='\u{D7AF}'  // Hangul
            | '\u{FF00}'..='\u{FFEF}'  // Halfwidth and Fullwidth Forms (！？ etc.)
            | '\u{FE30}'..='\u{FE4F}'  // CJK Compatibility Forms
            | '\u{FE50}'..='\u{FE6F}'  // Small Form Variants
            | '\u{20000}'..='\u{2A6DF}' // CJK Unified Ideographs Extension B
            | '\u{2A700}'..='\u{2B73F}' // CJK Unified Ideographs Extension C
            | '\u{1F000}'..='\u{1F02F}' // Mahjong Tiles
            | '\u{1F030}'..='\u{1F09F}' // Domino Tiles
        )
    }

    /// Returns true if the character is in CJK ranges (needs CJK-specific fallback).
    /// This includes not only CJK ideographs and kana, but also symbol ranges
    /// that are commonly rendered with CJK fonts (geometric shapes, enclosed
    /// alphanumerics, etc.) which have East Asian Ambiguous width.
    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{3000}'..='\u{9FFF}'  // CJK Unified Ideographs + Hiragana/Katakana
            | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
            | '\u{AC00}'..='\u{D7AF}'  // Hangul
            | '\u{FF00}'..='\u{FFEF}'  // Halfwidth and Fullwidth Forms (！？ etc.)
            | '\u{FE30}'..='\u{FE4F}'  // CJK Compatibility Forms
            | '\u{FE50}'..='\u{FE6F}'  // Small Form Variants
            | '\u{25A0}'..='\u{25FF}'  // Geometric Shapes (■□▲△○●◆◇◯ etc.)
            | '\u{2600}'..='\u{26FF}'  // Miscellaneous Symbols (commonly in CJK fonts)
            | '\u{2460}'..='\u{24FF}'  // Enclosed Alphanumerics (①②③ etc.)
            | '\u{20000}'..='\u{2A6DF}' // CJK Unified Ideographs Extension B
            | '\u{2A700}'..='\u{2B73F}' // CJK Unified Ideographs Extension C
        )
    }

    /// Rasterize a glyph, place it at the correct bearing offset within a
    /// cell-sized bitmap, and pack that bitmap into the atlas.
    fn rasterize_glyph(&mut self, c: char) -> GlyphInfo {
        // Block elements and shade characters: draw procedurally to fill cells
        // perfectly (font glyphs leave gaps that break ASCII art).
        if let Some(info) = self.try_procedural_block(c) {
            self.glyphs.insert(c, info);
            return info;
        }

        let (metrics, bitmap) = self.font.rasterize(c, self.font_size);

        let glyph_w = metrics.width as u32;
        let glyph_h = metrics.height as u32;

        // Determine the display width: CJK/wide characters span 2 cells.
        let char_width = termojinal_vt::char_width(c, self.cjk_width) as u32;
        let entry_w = self.cell_w * char_width;
        let entry_h = self.cell_h;

        // Check if we should use a fallback font instead.
        // Use fallback when: the primary returns a zero-size bitmap and the
        // char is in a special range, OR the primary font has no glyph for it.
        let primary_missing = (glyph_w == 0 || glyph_h == 0)
            || self.font.lookup_glyph_index(c) == 0;

        let (metrics, bitmap) = if Self::is_cjk(c) {
            // For CJK characters, ALWAYS prefer CJK font (Hiragino) for correct
            // wide-glyph rendering, even when the primary font has a glyph.
            if let Some(ref cjk) = self.cjk_font {
                if cjk.lookup_glyph_index(c) != 0 {
                    cjk.rasterize(c, self.font_size)
                } else if !primary_missing {
                    (metrics, bitmap) // primary has it, CJK doesn't
                } else if let Some(ref fb) = self.fallback_font {
                    if fb.lookup_glyph_index(c) != 0 {
                        fb.rasterize(c, self.font_size)
                    } else {
                        (metrics, bitmap)
                    }
                } else {
                    (metrics, bitmap)
                }
            } else if !primary_missing {
                (metrics, bitmap)
            } else if let Some(ref fb) = self.fallback_font {
                if fb.lookup_glyph_index(c) != 0 {
                    fb.rasterize(c, self.font_size)
                } else {
                    (metrics, bitmap)
                }
            } else {
                (metrics, bitmap)
            }
        } else if primary_missing && Self::needs_fallback_check(c) {
            // Non-CJK fallback: try Nerd Font first, then symbols font.
            let mut result = None;
            if let Some(ref fb) = self.fallback_font {
                if fb.lookup_glyph_index(c) != 0 {
                    result = Some(fb.rasterize(c, self.font_size));
                }
            }
            if result.is_none() {
                if let Some(ref sym) = self.symbols_font {
                    if sym.lookup_glyph_index(c) != 0 {
                        result = Some(sym.rasterize(c, self.font_size));
                    }
                }
            }
            result.unwrap_or((metrics, bitmap))
        } else if primary_missing {
            // Last-resort fallback for any character the primary font lacks:
            // try symbols font, then Nerd Font.
            let mut result = None;
            if let Some(ref sym) = self.symbols_font {
                if sym.lookup_glyph_index(c) != 0 {
                    result = Some(sym.rasterize(c, self.font_size));
                }
            }
            if result.is_none() {
                if let Some(ref fb) = self.fallback_font {
                    if fb.lookup_glyph_index(c) != 0 {
                        result = Some(fb.rasterize(c, self.font_size));
                    }
                }
            }
            result.unwrap_or((metrics, bitmap))
        } else {
            (metrics, bitmap)
        };

        let glyph_w = metrics.width as u32;
        let glyph_h = metrics.height as u32;

        // Handle zero-size glyphs (space, control chars) — still reserve a
        // cell-sized slot so background rendering works correctly.
        if glyph_w == 0 || glyph_h == 0 {
            let info = self.pack_cell_bitmap(&vec![0u8; (entry_w * entry_h) as usize], entry_w, entry_h);
            self.glyphs.insert(c, info);
            return info;
        }

        let mut cell_bitmap = vec![0u8; (entry_w * entry_h) as usize];

        // If glyph is wider than cell, squish horizontally only (keep full height).
        let (src_bitmap, src_w) = if glyph_w > entry_w {
            let mut squished = vec![0u8; (entry_w * glyph_h) as usize];
            for row in 0..glyph_h {
                for col in 0..entry_w {
                    let src_col = (col as f32 * glyph_w as f32 / entry_w as f32) as u32;
                    let si = (row * glyph_w + src_col.min(glyph_w - 1)) as usize;
                    let di = (row * entry_w + col) as usize;
                    if si < bitmap.len() && di < squished.len() {
                        squished[di] = bitmap[si];
                    }
                }
            }
            (squished, entry_w)
        } else {
            (bitmap, glyph_w)
        };

        let offset_x = if src_w < entry_w {
            metrics.xmin.max(0) as u32
        } else {
            0
        };
        let glyph_top_from_baseline = glyph_h as f32 + metrics.ymin as f32;
        let offset_y = (self.ascent - glyph_top_from_baseline).max(0.0) as u32;

        for row in 0..glyph_h.min(entry_h) {
            for col in 0..src_w.min(entry_w) {
                let dst_x = offset_x + col;
                let dst_y = offset_y + row;
                if dst_x < entry_w && dst_y < entry_h {
                    let src_idx = (row * src_w + col) as usize;
                    let dst_idx = (dst_y * entry_w + dst_x) as usize;
                    if src_idx < src_bitmap.len() && dst_idx < cell_bitmap.len() {
                        cell_bitmap[dst_idx] = src_bitmap[src_idx];
                    }
                }
            }
        }

        let info = self.pack_cell_bitmap(&cell_bitmap, entry_w, entry_h);
        self.glyphs.insert(c, info);
        info
    }

    /// Pack a bitmap into the atlas, returning the GlyphInfo.
    fn pack_cell_bitmap(&mut self, bitmap: &[u8], entry_w: u32, entry_h: u32) -> GlyphInfo {
        let padded_w = entry_w + 1;
        let padded_h = entry_h + 1;

        // Advance to next row if needed.
        if self.pack_x + padded_w > self.width {
            self.pack_x = 1;
            self.pack_y += self.pack_row_height + 1;
            self.pack_row_height = 0;
        }

        // Grow atlas if needed.
        if self.pack_y + padded_h > self.height {
            self.grow_atlas();
        }

        let atlas_x = self.pack_x;
        let atlas_y = self.pack_y;

        // Copy cell bitmap into atlas.
        for row in 0..entry_h {
            let src_offset = (row * entry_w) as usize;
            let dst_offset = ((atlas_y + row) * self.width + atlas_x) as usize;
            let src_end = src_offset + entry_w as usize;
            let dst_end = dst_offset + entry_w as usize;
            if src_end <= bitmap.len() && dst_end <= self.data.len() {
                self.data[dst_offset..dst_end].copy_from_slice(&bitmap[src_offset..src_end]);
            }
        }

        // Advance packing cursor.
        self.pack_x += padded_w;
        self.pack_row_height = self.pack_row_height.max(padded_h);

        GlyphInfo {
            atlas_x: atlas_x as f32,
            atlas_y: atlas_y as f32,
            atlas_w: entry_w as f32,
            atlas_h: entry_h as f32,
            bearing_x: 0.0, // Baked into the bitmap.
            bearing_y: 0.0,
        }
    }

    fn grow_atlas(&mut self) {
        let new_height = self.height * 2;
        let mut new_data = vec![0u8; (self.width * new_height) as usize];
        new_data[..self.data.len()].copy_from_slice(&self.data);
        self.data = new_data;
        self.height = new_height;
        log::info!("atlas grew to {}x{}", self.width, self.height);
    }

    // TODO: load font path from ~/.config/termojinal/config.toml instead of hardcoding
    fn load_font_data(family: &str) -> Result<Vec<u8>, AtlasError> {
        let candidates = if family == "monospace" || family.is_empty() {
            vec![
                // Prefer single-file TTF/OTF over TTC (fontdue handles them better).
                "/System/Library/Fonts/SFNSMono.ttf",
                "/Library/Fonts/JetBrainsMono-Regular.ttf",
                "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                "/System/Library/Fonts/Supplemental/Courier New.ttf",
                // TTC files: fontdue uses collection_index=0 by default.
                "/System/Library/Fonts/Menlo.ttc",
                "/System/Library/Fonts/Courier.ttc",
            ]
        } else {
            vec![]
        };

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                log::info!("loaded font from {path}");
                return Ok(data);
            }
        }

        let fallbacks = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ];
        for path in &fallbacks {
            if let Ok(data) = std::fs::read(path) {
                log::info!("loaded fallback font from {path}");
                return Ok(data);
            }
        }

        Err(AtlasError::FontNotFound(family.to_string()))
    }

    /// Try to find and load a Nerd Font from ~/Library/Fonts/ for fallback
    /// glyph rendering (PUA icons, box-drawing, etc.).
    fn load_fallback_nerd_font() -> Option<fontdue::Font> {
        let home = std::env::var("HOME").ok()?;
        let fonts_dir = std::path::PathBuf::from(&home).join("Library/Fonts");
        let entries = std::fs::read_dir(&fonts_dir).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // Look for Nerd Font files (contain "Nerd" or "NF" in the name).
            let is_nerd = name.contains("Nerd") || name.contains(" NF ");
            let is_ttf = name.ends_with(".ttf") || name.ends_with(".otf");

            if is_nerd && is_ttf {
                if let Ok(data) = std::fs::read(&path) {
                    match fontdue::Font::from_bytes(
                        data.as_slice(),
                        fontdue::FontSettings::default(),
                    ) {
                        Ok(font) => {
                            log::info!("loaded fallback Nerd Font from {}", path.display());
                            return Some(font);
                        }
                        Err(e) => {
                            log::warn!(
                                "failed to parse fallback font {}: {e}",
                                path.display()
                            );
                        }
                    }
                }
            }
        }

        log::info!("no Nerd Font found in {}, fallback disabled", fonts_dir.display());
        None
    }

    /// Try to find and load a CJK font from system font directories for fallback
    /// glyph rendering of Japanese/Chinese/Korean characters.
    fn load_cjk_fallback_font() -> Option<fontdue::Font> {
        // macOS system CJK font candidates.
        // Prefer single-file TTF/OTF over TTC since fontdue handles them better.
        // For TTC files, fontdue uses collection_index=0 by default.
        let candidates = [
            "/System/Library/Fonts/Supplemental/Hiragino Sans W3.ttc",
            "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ];

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                match fontdue::Font::from_bytes(
                    data.as_slice(),
                    fontdue::FontSettings::default(),
                ) {
                    Ok(font) => {
                        // Verify the font can actually render a common CJK character.
                        if font.lookup_glyph_index('あ') != 0 {
                            log::info!("loaded CJK fallback font from {path}");
                            return Some(font);
                        }
                        log::debug!("font {path} loaded but lacks CJK glyphs, skipping");
                    }
                    Err(e) => {
                        log::debug!("failed to parse CJK font {path}: {e}");
                    }
                }
            }
        }

        log::info!("no CJK fallback font found, CJK characters may not render");
        None
    }

    /// Try to load a symbols font for fallback rendering of Braille Patterns,
    /// geometric shapes, arrows, dingbats, and other Unicode symbols that
    /// monospace fonts typically lack. These are used by CLI tools for spinners,
    /// progress bars, and status indicators.
    fn load_symbols_fallback_font() -> Option<fontdue::Font> {
        // On macOS, Apple Symbols covers a wide range of Unicode symbols including
        // Braille Patterns (U+2800-28FF), Geometric Shapes, Arrows, Dingbats, etc.
        // Apple Braille is specifically for Braille Patterns.
        // LastResort covers virtually all Unicode as a final fallback.
        let candidates = [
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Apple Braille.ttf",
            "/System/Library/Fonts/LastResort.otf",
            // Linux fallbacks
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        ];

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                match fontdue::Font::from_bytes(
                    data.as_slice(),
                    fontdue::FontSettings::default(),
                ) {
                    Ok(font) => {
                        // Verify the font can render a Braille Pattern character
                        // (U+280B = ⠋, commonly used in CLI spinners).
                        if font.lookup_glyph_index('\u{280B}') != 0 {
                            log::info!("loaded symbols fallback font from {path}");
                            return Some(font);
                        }
                        log::debug!("font {path} loaded but lacks Braille glyphs, skipping");
                    }
                    Err(e) => {
                        log::debug!("failed to parse symbols font {path}: {e}");
                    }
                }
            }
        }

        log::info!("no symbols fallback font found, Braille/symbol characters may not render");
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("font not found: {0}")]
    FontNotFound(String),

    #[error("font parsing error: {0}")]
    FontParsing(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_create_atlas() -> Option<Atlas> {
        let config = FontConfig::default();
        match Atlas::new(&config) {
            Ok(atlas) => Some(atlas),
            Err(AtlasError::FontNotFound(_)) => {
                eprintln!("Skipping atlas test: no system monospace font found");
                None
            }
            Err(e) => panic!("Unexpected atlas error: {e}"),
        }
    }

    #[test]
    fn test_atlas_creation() {
        if let Some(atlas) = try_create_atlas() {
            assert!(atlas.width > 0);
            assert!(atlas.height > 0);
            assert!(atlas.cell_size.width > 0.0);
            assert!(atlas.cell_size.height > 0.0);
        }
    }

    #[test]
    fn test_ascii_glyphs_present() {
        if let Some(atlas) = try_create_atlas() {
            for c in (32u8..=126).map(|b| b as char) {
                assert!(
                    atlas.has_glyph(c),
                    "Missing glyph for '{c}' (0x{:02X})",
                    c as u32
                );
            }
        }
    }

    #[test]
    fn test_all_glyphs_are_cell_sized() {
        if let Some(atlas) = try_create_atlas() {
            let cell_w = atlas.cell_size.width;
            let cell_h = atlas.cell_size.height;
            for (&c, &info) in &atlas.glyphs {
                assert!(
                    (info.atlas_w - cell_w).abs() < 0.01
                        && (info.atlas_h - cell_h).abs() < 0.01,
                    "Glyph '{c}' has atlas size {}x{}, expected {}x{}",
                    info.atlas_w,
                    info.atlas_h,
                    cell_w,
                    cell_h
                );
            }
        }
    }

    #[test]
    fn test_on_demand_rasterize() {
        if let Some(mut atlas) = try_create_atlas() {
            let initial_count = atlas.glyph_count();
            let _info = atlas.get_glyph('\u{2500}');
            assert!(atlas.glyph_count() > initial_count);
        }
    }

    #[test]
    fn test_glyph_info_a() {
        if let Some(atlas) = try_create_atlas() {
            let glyph = atlas.glyphs.get(&'A').expect("'A' glyph missing");
            assert!(glyph.atlas_w > 0.0);
            assert!(glyph.atlas_h > 0.0);
        }
    }

    #[test]
    fn test_braille_spinner_chars_rasterize() {
        // Braille Pattern characters used by CLI spinners (e.g. Claude Code thinking animation).
        if let Some(mut atlas) = try_create_atlas() {
            let braille_spinner = [
                '\u{280B}', // ⠋
                '\u{2819}', // ⠙
                '\u{2839}', // ⠹
                '\u{2838}', // ⠸
                '\u{283C}', // ⠼
                '\u{2834}', // ⠴
                '\u{2826}', // ⠦
                '\u{2827}', // ⠧
                '\u{2807}', // ⠇
                '\u{280F}', // ⠏
            ];

            for &c in &braille_spinner {
                let initial = atlas.glyph_count();
                let glyph = atlas.get_glyph(c);
                assert!(
                    glyph.atlas_w > 0.0 && glyph.atlas_h > 0.0,
                    "Braille char '{}' (U+{:04X}) must have non-zero atlas size",
                    c, c as u32
                );
                // Verify the glyph was actually rasterized (not just a blank slot).
                assert!(
                    atlas.has_glyph(c),
                    "Braille char '{}' (U+{:04X}) must be cached in atlas",
                    c, c as u32
                );
                // Verify the atlas bitmap has non-zero pixels for this glyph
                // (i.e., actual glyph data, not just a transparent empty cell).
                let ax = glyph.atlas_x as u32;
                let ay = glyph.atlas_y as u32;
                let aw = glyph.atlas_w as u32;
                let ah = glyph.atlas_h as u32;
                let mut nonzero = 0usize;
                for row in ay..(ay + ah).min(atlas.height) {
                    for col in ax..(ax + aw).min(atlas.width) {
                        let idx = (row * atlas.width + col) as usize;
                        if idx < atlas.data.len() && atlas.data[idx] > 0 {
                            nonzero += 1;
                        }
                    }
                }
                assert!(
                    nonzero > 0,
                    "Braille char '{}' (U+{:04X}) must have visible pixels (got 0 non-zero)",
                    c, c as u32
                );
                eprintln!(
                    "Braille '{}' (U+{:04X}): atlas_w={}, atlas_h={}, nonzero_pixels={}, new={}",
                    c, c as u32, glyph.atlas_w, glyph.atlas_h, nonzero, atlas.glyph_count() > initial
                );
            }
        }
    }

    #[test]
    fn test_needs_fallback_check_includes_braille() {
        // Verify Braille Patterns are included in fallback check.
        assert!(Atlas::needs_fallback_check('\u{2800}')); // Empty braille
        assert!(Atlas::needs_fallback_check('\u{280B}')); // ⠋ (spinner)
        assert!(Atlas::needs_fallback_check('\u{28FF}')); // End of braille range

        // Also verify other symbol ranges used by CLI tools.
        assert!(Atlas::needs_fallback_check('\u{2714}')); // ✔ check mark
        assert!(Atlas::needs_fallback_check('\u{2718}')); // ✘ ballot x
        assert!(Atlas::needs_fallback_check('\u{25CF}')); // ● black circle
        assert!(Atlas::needs_fallback_check('\u{2190}')); // ← left arrow
        assert!(Atlas::needs_fallback_check('\u{2588}')); // █ full block (already handled)
    }

    #[test]
    fn test_packing_no_overlap() {
        if let Some(atlas) = try_create_atlas() {
            let rects: Vec<_> = atlas
                .glyphs
                .values()
                .filter(|g| g.atlas_w > 0.0 && g.atlas_h > 0.0)
                .collect();

            for (i, a) in rects.iter().enumerate() {
                for b in rects.iter().skip(i + 1) {
                    let no_overlap = a.atlas_x + a.atlas_w <= b.atlas_x
                        || b.atlas_x + b.atlas_w <= a.atlas_x
                        || a.atlas_y + a.atlas_h <= b.atlas_y
                        || b.atlas_y + b.atlas_h <= a.atlas_y;
                    assert!(
                        no_overlap,
                        "Overlap: ({},{},{},{}) vs ({},{},{},{})",
                        a.atlas_x, a.atlas_y, a.atlas_w, a.atlas_h, b.atlas_x, b.atlas_y,
                        b.atlas_w, b.atlas_h,
                    );
                }
            }
        }
    }
}
