//! Character printing and execution logic for the terminal.

use unicode_width::UnicodeWidthChar;

use super::Terminal;

/// Calculate the display width of a character, respecting CJK ambiguous width.
///
/// When `cjk` is true, characters with East Asian Width "Ambiguous" are treated
/// as 2-cell wide (standard CJK terminal behavior). Otherwise they are 1-cell wide.
#[inline]
pub fn char_width(c: char, cjk: bool) -> usize {
    if cjk {
        UnicodeWidthChar::width_cjk(c).unwrap_or(1)
    } else {
        UnicodeWidthChar::width(c).unwrap_or(1)
    }
}

/// Returns true for zero-width combining codepoints that should not occupy a cell.
pub(crate) fn is_zero_width_combining(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0xFE00..=0xFE0F        // Variation selectors
        | 0x200D               // ZWJ (Zero Width Joiner)
        | 0x200B..=0x200F      // Zero-width space, ZWNJ, ZWJ, LRM, RLM
        | 0x1F3FB..=0x1F3FF    // Skin tone modifiers
        | 0xE0020..=0xE007F    // Tag characters (flag subdivisions)
        | 0xE0001              // Language tag
        | 0x20E3               // Combining enclosing keycap
        | 0x2060..=0x2064      // Word joiner, invisible separators
        | 0xFEFF               // BOM / zero-width no-break space
    )
}

impl Terminal {
    /// Handle the `print` callback from vte::Perform.
    pub(crate) fn do_print(&mut self, c: char) {
        // Skip zero-width combining characters that should not occupy a cell.
        // These include variation selectors, ZWJ, skin tone modifiers, and tags.
        if is_zero_width_combining(c) {
            return;
        }

        // Guard against zero-size terminal (can happen during resize transitions).
        if self.cols == 0 || self.rows == 0 {
            return;
        }

        let char_width = char_width(c, self.cjk_width);

        if self.wrap_pending {
            self.wrap_pending = false;
            if self.modes.auto_wrap {
                self.cursor_col = 0;
                self.newline();
            }
        }

        if char_width == 2 && self.cursor_col >= self.cols - 1 {
            if self.modes.auto_wrap {
                self.cursor_col = 0;
                self.newline();
            } else {
                return;
            }
        }

        let col = self.cursor_col;
        let row = self.cursor_row;
        let pen = self.pen;

        let has_hyperlink = self.current_hyperlink.is_some();

        // Fix ghost characters: clean up wide character fragments.
        //
        // Case 1: If we are overwriting the continuation cell (width==0)
        // of a wide character, the leading cell must be cleared.
        if col > 0 {
            let prev_width = self.grid().cell(col, row).width;
            if prev_width == 0 {
                // This cell is a continuation — clear the leading cell to the left.
                self.grid_mut().cell_mut(col - 1, row).reset();
            }
        }

        // Case 2: If we are overwriting the leading cell (width==2) of a wide
        // character, the continuation cell to the right must be cleared.
        {
            let old_width = self.grid().cell(col, row).width;
            if old_width == 2 && col + 1 < self.cols {
                self.grid_mut().cell_mut(col + 1, row).reset();
            }
        }

        // Case 3: If we are writing a wide character, the continuation cell
        // at col+1 might be the leading cell of another wide character. If so,
        // clear *that* wide character's continuation at col+2.
        if char_width == 2 && col + 1 < self.cols {
            let next_width = self.grid().cell(col + 1, row).width;
            if next_width == 2 && col + 2 < self.cols {
                self.grid_mut().cell_mut(col + 2, row).reset();
            }
        }

        {
            let cell = self.grid_mut().cell_mut(col, row);
            cell.c = c;
            cell.fg = pen.fg;
            cell.bg = pen.bg;
            cell.attrs = pen.attrs;
            cell.underline_color = pen.underline_color;
            cell.width = char_width as u8;
            cell.hyperlink = has_hyperlink;
        }

        if char_width == 2 && col + 1 < self.cols {
            let cell = self.grid_mut().cell_mut(col + 1, row);
            cell.c = '\0';
            cell.width = 0;
            cell.fg = pen.fg;
            cell.bg = pen.bg;
            cell.attrs = pen.attrs;
            cell.hyperlink = has_hyperlink;
        }

        self.cursor_col += char_width;
        if self.cursor_col >= self.cols {
            self.cursor_col = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    /// Handle the `execute` callback from vte::Perform.
    pub(crate) fn do_execute(&mut self, byte: u8) {
        match byte {
            0x07 => {
                log::trace!("BEL");
            }
            0x08 => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
                self.wrap_pending = false;
            }
            0x09 => {
                if self.cols == 0 {
                    return;
                }
                let cur = self.cursor_col;
                let next_tab = if cur + 1 < self.cols {
                    self.tab_stops[cur + 1..]
                        .iter()
                        .position(|&t| t)
                        .map(|p| cur + 1 + p)
                        .unwrap_or(self.cols.saturating_sub(1))
                } else {
                    self.cols.saturating_sub(1)
                };
                self.cursor_col = next_tab.min(self.cols.saturating_sub(1));
                self.wrap_pending = false;
            }
            0x0A | 0x0B | 0x0C => {
                self.newline();
                // LNM (mode 20): when set, LF also performs CR.
                if self.modes.linefeed_mode {
                    self.cursor_col = 0;
                }
                self.wrap_pending = false;
            }
            0x0D => {
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            0x0E | 0x0F => {}
            _ => {
                log::trace!("unhandled execute: 0x{byte:02x}");
            }
        }
    }

    /// Handle the `esc_dispatch` callback from vte::Perform.
    pub(crate) fn do_esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            ([], b'7') => {
                let saved = super::modes::SavedCursor {
                    col: self.cursor_col,
                    row: self.cursor_row,
                    pen: self.pen,
                    cursor_visible: self.modes.cursor_visible,
                    cursor_shape: self.cursor_shape,
                };
                if self.using_alt {
                    self.saved_cursor_alt = Some(saved);
                } else {
                    self.saved_cursor_main = Some(saved);
                }
            }
            ([], b'8') => {
                let saved = if self.using_alt {
                    self.saved_cursor_alt
                } else {
                    self.saved_cursor_main
                };
                if let Some(s) = saved {
                    self.cursor_col = s.col;
                    self.cursor_row = s.row;
                    self.pen = s.pen;
                    self.modes.cursor_visible = s.cursor_visible;
                    self.cursor_shape = s.cursor_shape;
                }
            }
            ([], b'M') => {
                self.reverse_index();
            }
            ([], b'D') => {
                self.newline();
            }
            ([], b'E') => {
                self.cursor_col = 0;
                self.newline();
            }
            ([], b'H') => {
                if self.cursor_col < self.cols {
                    self.tab_stops[self.cursor_col] = true;
                }
            }
            ([], b'c') => {
                let cols = self.cols;
                let rows = self.rows;
                *self = Terminal::new(cols, rows);
            }
            _ => {
                log::trace!("unhandled ESC: intermediates={intermediates:?}, byte=0x{byte:02x}");
            }
        }
    }
}
