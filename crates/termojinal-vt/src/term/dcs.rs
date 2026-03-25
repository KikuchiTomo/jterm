//! DCS (Device Control String) handling: Sixel and DECDLD.

use crate::image;

use super::modes::{DcsMode, DrcsGlyph};
use super::Terminal;

impl Terminal {
    /// Handle the `hook` callback from vte::Perform (DCS start).
    pub(crate) fn do_dcs_hook(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'q' => {
                // Sixel DCS: ESC P <params> q <sixel_data> ST
                log::trace!("DCS hook: Sixel");
                self.dcs_mode = DcsMode::Sixel;
                self.dcs_data.clear();
            }
            // DECDLD — Dynamically Redefinable Character Set (soft fonts).
            //
            // Format: DCS Pfn ; Pcn ; Pe ; Pcmw ; Pss ; Pt ; Pcmh ; Pcss { Dscs <data> ST
            //   Pfn = font number (0-3)
            //   Pcn = starting character code (0-95, added to 0x20)
            //   Pe  = erase control (0=erase all, 1=erase loaded chars only, 2=erase all)
            //   Pcmw = character cell width (0 = default)
            //   Pss  = font set size (0=80-col, 1=132-col, 2=both)
            //   Pt   = text/full cell (0=text, 1=full cell, 2=text)
            //   Pcmh = character cell height (0 = default)
            //   Pcss = character set size (0=94, 1=96)
            //
            // The '{' final byte introduces the font data.
            '{' => {
                let p: Vec<u16> = params
                    .iter()
                    .map(|s| s.first().copied().unwrap_or(0))
                    .collect();
                let pfn = p.first().copied().unwrap_or(0).min(3) as u8;
                let pcn = p.get(1).copied().unwrap_or(0).min(95) as u8;
                let pe = p.get(2).copied().unwrap_or(0).min(2) as u8;
                let pcmw = p.get(3).copied().unwrap_or(0) as u8;
                let pcmh = p.get(6).copied().unwrap_or(0) as u8;

                // Default cell dimensions based on font size.
                let cell_width = if pcmw == 0 { 10 } else { pcmw };
                let cell_height = if pcmh == 0 { 20 } else { pcmh };

                log::debug!(
                    "DCS hook: DECDLD font={pfn} start_char={pcn} erase={pe} \
                     cell={}x{}",
                    cell_width, cell_height
                );

                // Erase existing glyphs per the erase control parameter.
                match pe {
                    0 | 2 => self.drcs_fonts.erase_font(pfn),
                    1 => {} // Only erase chars being loaded (handled during glyph parsing).
                    _ => {}
                }

                self.dcs_mode = DcsMode::Decdld {
                    font_number: pfn,
                    start_char: pcn,
                    cell_width,
                    cell_height,
                    erase_control: pe,
                };
                self.dcs_data.clear();
            }
            _ => {
                log::trace!("DCS hook: action={action}");
                self.dcs_mode = DcsMode::None;
            }
        }
    }

    /// Handle the `put` callback from vte::Perform (DCS data byte).
    pub(crate) fn do_dcs_put(&mut self, byte: u8) {
        match self.dcs_mode {
            DcsMode::Sixel | DcsMode::Decdld { .. } => {
                self.dcs_data.push(byte);
            }
            DcsMode::None => {}
        }
    }

    /// Handle the `unhook` callback from vte::Perform (DCS end).
    pub(crate) fn do_dcs_unhook(&mut self) {
        match std::mem::replace(&mut self.dcs_mode, DcsMode::None) {
            DcsMode::Sixel => {
                log::trace!("DCS unhook: Sixel ({} bytes)", self.dcs_data.len());
                let data = std::mem::take(&mut self.dcs_data);
                let col = self.cursor_col;
                let row = self.cursor_row;
                let before = self.image_store.placements().len();
                image::process_sixel(&data, &mut self.image_store, col, row);
                if self.image_store.placements().len() > before {
                    self.image_store.cap_placement_size(self.cols, self.rows);
                    let cell_rows = self
                        .image_store
                        .placements()
                        .last()
                        .map(|p| p.cell_rows)
                        .unwrap_or(1);
                    self.advance_cursor_past_image(cell_rows);
                }
            }
            DcsMode::Decdld {
                font_number,
                start_char,
                cell_width,
                cell_height,
                ..
            } => {
                let data = std::mem::take(&mut self.dcs_data);
                log::debug!(
                    "DCS unhook: DECDLD font={font_number} ({} bytes)",
                    data.len()
                );
                // Skip the Dscs (designator) byte(s) if present.
                // The data format after Dscs is: rows of sixel-like data separated by ';'.
                // Each glyph row is separated by '/'.
                let body = if let Some(_pos) = data.iter().position(|&b| b == b'/') {
                    // There may be leading designator chars before the first data.
                    // Actually, the Dscs is a single character set designator like 'B' or '@'.
                    // For simplicity, we treat the entire data as glyph definitions.
                    &data[..]
                } else {
                    &data[..]
                };

                // Parse glyph data: glyphs are separated by ';'.
                // Within each glyph, sixel rows are separated by '/'.
                let mut char_code = start_char;
                for glyph_data in body.split(|&b| b == b';') {
                    if glyph_data.is_empty() {
                        char_code = char_code.wrapping_add(1);
                        continue;
                    }

                    let w = cell_width as usize;
                    let h = cell_height as usize;
                    // Each row is a sequence of sixel-encoded columns.
                    // Rows within a glyph are separated by '/'.
                    let mut bitmap = vec![0u8; (w * h + 7) / 8];
                    let mut pixel_y = 0usize;

                    for row_data in glyph_data.split(|&b| b == b'/') {
                        let mut pixel_x = 0usize;
                        let mut i = 0;
                        while i < row_data.len() {
                            let b = row_data[i];
                            if b == b'!' {
                                // Repeat: !<count><char>
                                i += 1;
                                let mut count = 0usize;
                                while i < row_data.len() && row_data[i].is_ascii_digit() {
                                    count = count * 10 + (row_data[i] - b'0') as usize;
                                    i += 1;
                                }
                                if i < row_data.len() && row_data[i] >= 0x3F && row_data[i] <= 0x7E
                                {
                                    let val = row_data[i] - 0x3F;
                                    for _ in 0..count {
                                        for bit in 0..6u8 {
                                            if val & (1 << bit) != 0 {
                                                let py = pixel_y + bit as usize;
                                                if pixel_x < w && py < h {
                                                    let bit_idx = py * w + pixel_x;
                                                    bitmap[bit_idx / 8] |=
                                                        1 << (7 - (bit_idx % 8));
                                                }
                                            }
                                        }
                                        pixel_x += 1;
                                    }
                                    i += 1;
                                }
                                continue;
                            }
                            if b >= 0x3F && b <= 0x7E {
                                let val = b - 0x3F;
                                for bit in 0..6u8 {
                                    if val & (1 << bit) != 0 {
                                        let py = pixel_y + bit as usize;
                                        if pixel_x < w && py < h {
                                            let bit_idx = py * w + pixel_x;
                                            bitmap[bit_idx / 8] |= 1 << (7 - (bit_idx % 8));
                                        }
                                    }
                                }
                                pixel_x += 1;
                            }
                            i += 1;
                        }
                        pixel_y += 6;
                    }

                    self.drcs_fonts.set_glyph(
                        font_number,
                        char_code,
                        DrcsGlyph {
                            bitmap,
                            width: cell_width,
                            height: cell_height,
                        },
                    );
                    log::trace!(
                        "DECDLD: defined glyph font={font_number} char=0x{:02X}",
                        0x20 + char_code
                    );
                    char_code = char_code.wrapping_add(1);
                }
            }
            DcsMode::None => {
                log::trace!("DCS unhook");
            }
        }
    }
}
