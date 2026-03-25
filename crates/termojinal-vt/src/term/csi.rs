//! CSI (Control Sequence Introducer) dispatch handling.

use crate::cell::Attrs;
use crate::color::{Color, NamedColor};

use super::modes::{CursorShape, MouseFormat, MouseMode, SavedCursor};
use super::Terminal;

impl Terminal {
    /// Handle the `csi_dispatch` callback from vte::Perform.
    pub(crate) fn do_csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let p: Vec<Vec<u16>> = params.iter().map(|s| s.to_vec()).collect();
        let param = |idx: usize, default: u16| -> u16 {
            p.get(idx)
                .and_then(|s| s.first().copied())
                .filter(|&v| v != 0)
                .unwrap_or(default)
        };

        match (intermediates, action) {
            ([], 'A') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.wrap_pending = false;
            }
            ([], 'B') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(self.cursor_row + n);
                self.wrap_pending = false;
            }
            ([], 'C') => {
                let n = param(0, 1) as usize;
                self.cursor_col = self.clamp_col(self.cursor_col + n);
                self.wrap_pending = false;
            }
            ([], 'D') => {
                let n = param(0, 1) as usize;
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.wrap_pending = false;
            }
            ([], 'E') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(self.cursor_row + n);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            ([], 'F') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            ([], 'G') => {
                let col = param(0, 1) as usize;
                self.cursor_col = self.clamp_col(col.saturating_sub(1));
                self.wrap_pending = false;
            }
            ([], 'H') | ([], 'f') => {
                let row = param(0, 1) as usize;
                let col = param(1, 1) as usize;
                self.cursor_row = self.clamp_row(row.saturating_sub(1));
                self.cursor_col = self.clamp_col(col.saturating_sub(1));
                self.wrap_pending = false;
            }
            // ED — Erase in Display (BCE: use current pen background).
            ([], 'J') => {
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                match param(0, 0) {
                    0 => self.grid_mut().erase_below_with_bg(col, row, bg),
                    1 => self.grid_mut().erase_above_with_bg(col, row, bg),
                    2 | 3 => self.grid_mut().clear_with_bg(bg),
                    _ => {}
                }
            }
            // EL — Erase in Line (BCE: use current pen background).
            ([], 'K') => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                let bg = self.pen.bg;
                match param(0, 0) {
                    0 => self.grid_mut().clear_to_eol_with_bg(col, row, bg),
                    1 => self.grid_mut().clear_from_bol_with_bg(col, row, bg),
                    2 => self.grid_mut().clear_row_with_bg(row, bg),
                    _ => {}
                }
            }
            // IL — Insert Lines (BCE).
            ([], 'L') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().insert_lines_with_bg(row, n, bottom, bg);
            }
            // DL — Delete Lines (BCE).
            ([], 'M') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().delete_lines_with_bg(row, n, bottom, bg);
            }
            // DCH — Delete Characters (BCE).
            ([], 'P') => {
                let n = param(0, 1) as usize;
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                self.grid_mut().delete_cells_with_bg(col, row, n, bg);
            }
            // SU — Scroll Up (C3: track scrolled lines for command history) (BCE).
            ([], 'S') => {
                let n = param(0, 1) as usize;
                let top = self.scroll_top;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                // Save scrolled-off rows to scrollback (same guard as newline).
                if top == 0 && !self.using_alt {
                    for i in 0..n.min(bottom + 1) {
                        let row = self.grid().row_cells(i);
                        self.scrollback.push(row);
                    }
                    self.total_scrolled_lines += n.min(bottom + 1);
                    // Scroll image placements so they track the grid content.
                    self.image_store.scroll_up(n);
                }
                self.grid_mut().scroll_up_with_bg(top, bottom, n, bg);
            }
            // SD — Scroll Down (BCE).
            ([], 'T') => {
                let n = param(0, 1) as usize;
                let top = self.scroll_top;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().scroll_down_with_bg(top, bottom, n, bg);
            }
            // ECH — Erase Characters (BCE: use current pen background).
            ([], 'X') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let col = self.cursor_col;
                let cols = self.cols;
                let bg = self.pen.bg;
                // Handle wide char fragment at start of erased region.
                if col < cols && self.grid().cell(col, row).width == 0 && col > 0 {
                    self.grid_mut().cell_mut(col - 1, row).reset_with_bg(bg);
                }
                // Handle wide char fragment at end of erased region.
                let end = (col + n).min(cols);
                if end > 0 && end < cols && self.grid().cell(end, row).width == 0 {
                    // Continuation of a wide char whose leading cell was erased.
                    self.grid_mut().cell_mut(end, row).reset_with_bg(bg);
                }
                if end > 0 && end <= cols {
                    let last_erased = end - 1;
                    if self.grid().cell(last_erased, row).width == 2 && last_erased + 1 < cols {
                        // Leading cell of wide char partially erased; clear continuation.
                        // (handled below by the reset loop)
                    }
                }
                for i in 0..n {
                    let c = col + i;
                    if c < cols {
                        self.grid_mut().cell_mut(c, row).reset_with_bg(bg);
                    }
                }
            }
            // ICH — Insert Characters (BCE).
            ([], '@') => {
                let n = param(0, 1) as usize;
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                self.grid_mut().insert_cells_with_bg(col, row, n, bg);
            }
            // VPA — Vertical Line Position Absolute.
            ([], 'd') => {
                let row = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(row.saturating_sub(1));
                self.wrap_pending = false;
            }
            // SGR — Select Graphic Rendition.
            ([], 'm') => {
                self.handle_sgr(params);
            }
            // DSR — Device Status Report.
            ([], 'n') => {
                match param(0, 0) {
                    5 => {
                        // Status report — respond "OK".
                        self.queue_response(b"\x1b[0n".to_vec());
                        log::trace!("DSR: status report -> OK");
                    }
                    6 => {
                        // Cursor position report.
                        let row = self.cursor_row + 1;
                        let col = self.cursor_col + 1;
                        let response = format!("\x1b[{row};{col}R");
                        self.queue_response(response.into_bytes());
                        log::trace!("DSR: cursor position -> {row};{col}");
                    }
                    _ => {
                        log::trace!("DSR: unhandled request {}", param(0, 0));
                    }
                }
            }
            // DECSTBM — Set Top and Bottom Margins.
            ([], 'r') => {
                if self.rows == 0 {
                    return;
                }
                let top = param(0, 1) as usize;
                let bottom = param(1, self.rows as u16) as usize;
                self.scroll_top = top.saturating_sub(1);
                self.scroll_bottom = (bottom.saturating_sub(1)).min(self.rows.saturating_sub(1));
                if self.scroll_top >= self.scroll_bottom {
                    // Invalid region (top >= bottom); reset to full screen.
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows.saturating_sub(1);
                }
                self.cursor_col = 0;
                self.cursor_row = 0;
                self.wrap_pending = false;
            }
            // CBT — Cursor Backward Tabulation.
            ([], 'Z') => {
                let n = param(0, 1) as usize;
                for _ in 0..n {
                    if self.cursor_col == 0 {
                        break;
                    }
                    self.cursor_col -= 1;
                    while self.cursor_col > 0 && !self.tab_stops[self.cursor_col] {
                        self.cursor_col -= 1;
                    }
                }
                self.wrap_pending = false;
            }
            // TBC — Tabulation Clear.
            ([], 'g') => match param(0, 0) {
                0 => {
                    if self.cursor_col < self.cols {
                        self.tab_stops[self.cursor_col] = false;
                    }
                }
                3 => {
                    for t in &mut self.tab_stops {
                        *t = false;
                    }
                }
                _ => {}
            },
            // DECSCUSR — Set Cursor Shape.
            ([b' '], 'q') => {
                self.cursor_shape = match param(0, 1) {
                    0 | 1 => CursorShape::BlinkingBlock,
                    2 => CursorShape::Block,
                    3 => CursorShape::BlinkingUnderline,
                    4 => CursorShape::Underline,
                    5 => CursorShape::BlinkingBar,
                    6 => CursorShape::Bar,
                    _ => CursorShape::BlinkingBlock,
                };
            }
            // DECSET — Private mode set.
            ([b'?'], 'h') => {
                for sub in params.iter() {
                    self.handle_private_mode(sub[0], true);
                }
            }
            // DECRST — Private mode reset.
            ([b'?'], 'l') => {
                for sub in params.iter() {
                    self.handle_private_mode(sub[0], false);
                }
            }
            // SM — Set Mode (ANSI modes).
            ([], 'h') => match param(0, 0) {
                4 => self.modes.insert_mode = true,
                20 => {
                    self.modes.linefeed_mode = true;
                    log::trace!("LNM on");
                }
                _ => {}
            },
            // RM — Reset Mode (ANSI modes).
            ([], 'l') => match param(0, 0) {
                4 => self.modes.insert_mode = false,
                20 => {
                    self.modes.linefeed_mode = false;
                    log::trace!("LNM off");
                }
                _ => {}
            },
            // Kitty keyboard protocol: CSI > flags u — push keyboard mode.
            ([b'>'], 'u') => {
                let flags = param(0, 0) as u32;
                self.kitty_keyboard_flags.push(flags);
                log::trace!("kitty keyboard: push flags={flags}");
            }
            // Kitty keyboard protocol: CSI < number u — pop keyboard mode(s).
            ([b'<'], 'u') => {
                let count = param(0, 1).max(1) as usize;
                for _ in 0..count {
                    if self.kitty_keyboard_flags.pop().is_none() {
                        break;
                    }
                }
                log::trace!("kitty keyboard: pop {count}");
            }
            // Kitty keyboard protocol: CSI ? u — query current keyboard mode.
            ([b'?'], 'u') => {
                log::trace!(
                    "kitty keyboard: query (current={})",
                    self.kitty_keyboard_mode()
                );
            }
            // DA — Device Attributes (Primary).
            ([], 'c') => {
                if param(0, 0) == 0 {
                    // Respond as VT220 with Sixel, DRCS support.
                    // Attributes: 62=VT220, 4=Sixel, 22=ANSI color
                    self.queue_response(b"\x1b[?62;4;22c".to_vec());
                    log::trace!("DA: primary device attributes");
                }
            }
            // DA2 — Secondary Device Attributes.
            ([b'>'], 'c') => {
                if param(0, 0) == 0 {
                    // Respond as VT220, firmware version 1, ROM cartridge 0.
                    self.queue_response(b"\x1b[>1;1;0c".to_vec());
                    log::trace!("DA2: secondary device attributes");
                }
            }
            _ => {
                log::trace!(
                    "unhandled CSI: intermediates={intermediates:?}, action={action}, params={p:?}"
                );
            }
        }
    }

    /// Process SGR (Select Graphic Rendition) parameters.
    pub(crate) fn handle_sgr(&mut self, params: &vte::Params) {
        let mut iter = params.iter();

        let first = match iter.next() {
            Some(sub) => sub,
            None => {
                self.pen.reset();
                return;
            }
        };

        let mut pending: Option<&[u16]> = Some(first);

        while let Some(sub) = pending.take().or_else(|| iter.next()) {
            let code = sub[0];
            match code {
                0 => self.pen.reset(),
                1 => self.pen.attrs.insert(Attrs::BOLD),
                2 => self.pen.attrs.insert(Attrs::DIM),
                3 => self.pen.attrs.insert(Attrs::ITALIC),
                4 => {
                    if sub.len() > 1 {
                        match sub[1] {
                            0 => {
                                self.pen.attrs.remove(
                                    Attrs::UNDERLINE
                                        | Attrs::DOUBLE_UNDERLINE
                                        | Attrs::CURLY_UNDERLINE
                                        | Attrs::DOTTED_UNDERLINE
                                        | Attrs::DASHED_UNDERLINE,
                                );
                            }
                            1 => self.pen.attrs.insert(Attrs::UNDERLINE),
                            2 => self.pen.attrs.insert(Attrs::DOUBLE_UNDERLINE),
                            3 => self.pen.attrs.insert(Attrs::CURLY_UNDERLINE),
                            4 => self.pen.attrs.insert(Attrs::DOTTED_UNDERLINE),
                            5 => self.pen.attrs.insert(Attrs::DASHED_UNDERLINE),
                            _ => {}
                        }
                    } else {
                        self.pen.attrs.insert(Attrs::UNDERLINE);
                    }
                }
                5 | 6 => self.pen.attrs.insert(Attrs::BLINK),
                7 => self.pen.attrs.insert(Attrs::REVERSE),
                8 => self.pen.attrs.insert(Attrs::HIDDEN),
                9 => self.pen.attrs.insert(Attrs::STRIKETHROUGH),
                21 => self.pen.attrs.insert(Attrs::DOUBLE_UNDERLINE),
                22 => self.pen.attrs.remove(Attrs::BOLD | Attrs::DIM),
                23 => self.pen.attrs.remove(Attrs::ITALIC),
                24 => {
                    self.pen.attrs.remove(
                        Attrs::UNDERLINE
                            | Attrs::DOUBLE_UNDERLINE
                            | Attrs::CURLY_UNDERLINE
                            | Attrs::DOTTED_UNDERLINE
                            | Attrs::DASHED_UNDERLINE,
                    );
                }
                25 => self.pen.attrs.remove(Attrs::BLINK),
                27 => self.pen.attrs.remove(Attrs::REVERSE),
                28 => self.pen.attrs.remove(Attrs::HIDDEN),
                29 => self.pen.attrs.remove(Attrs::STRIKETHROUGH),
                30..=37 | 90..=97 => {
                    if let Some(c) = NamedColor::from_sgr_fg(code) {
                        self.pen.fg = Color::Named(c);
                    }
                }
                38 => {
                    self.pen.fg = parse_extended_color(&mut iter);
                }
                39 => self.pen.fg = Color::Default,
                40..=47 | 100..=107 => {
                    if let Some(c) = NamedColor::from_sgr_bg(code) {
                        self.pen.bg = Color::Named(c);
                    }
                }
                48 => {
                    self.pen.bg = parse_extended_color(&mut iter);
                }
                49 => self.pen.bg = Color::Default,
                58 => {
                    self.pen.underline_color = parse_extended_color(&mut iter);
                }
                59 => self.pen.underline_color = Color::Default,
                _ => {
                    log::trace!("unhandled SGR code: {code}");
                }
            }
        }
    }

    /// Handle private mode set/reset (DECSET/DECRST).
    pub(crate) fn handle_private_mode(&mut self, code: u16, enable: bool) {
        match code {
            // DECCKM — Application cursor keys.
            1 => {
                self.modes.application_cursor_keys = enable;
                log::trace!("DECCKM {}", if enable { "on" } else { "off" });
            }
            // DECOM — Origin mode.
            6 => {
                self.modes.origin_mode = enable;
                // When origin mode changes, cursor moves to the origin.
                if enable {
                    self.cursor_col = 0;
                    self.cursor_row = self.scroll_top;
                } else {
                    self.cursor_col = 0;
                    self.cursor_row = 0;
                }
                self.wrap_pending = false;
                log::trace!("DECOM {}", if enable { "on" } else { "off" });
            }
            7 => self.modes.auto_wrap = enable,
            // X10 mouse reporting (mode 9).
            9 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::Click
                } else {
                    MouseMode::None
                };
                log::trace!("X10 mouse mode 9 {}", if enable { "on" } else { "off" });
            }
            12 => {}
            25 => self.modes.cursor_visible = enable,
            47 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            // Mouse tracking modes — only one can be active at a time.
            1000 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::Click
                } else {
                    MouseMode::None
                };
                log::trace!(
                    "mouse mode 1000 (click) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1002 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::ButtonMotion
                } else {
                    MouseMode::None
                };
                log::trace!(
                    "mouse mode 1002 (button motion) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1003 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::AnyMotion
                } else {
                    MouseMode::None
                };
                log::trace!(
                    "mouse mode 1003 (any motion) {}",
                    if enable { "on" } else { "off" }
                );
            }
            // Focus events (mode 1004).
            1004 => {
                self.modes.focus_events = enable;
                log::trace!("focus events {}", if enable { "on" } else { "off" });
            }
            // DECSDM — Sixel display mode (mode 80).
            80 => {
                self.modes.sixel_display_mode = enable;
                log::trace!("DECSDM {}", if enable { "on" } else { "off" });
            }
            // Mouse format modes.
            1005 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Utf8
                } else {
                    MouseFormat::X10
                };
                log::trace!(
                    "mouse format 1005 (utf8) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1006 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Sgr
                } else {
                    MouseFormat::X10
                };
                log::trace!(
                    "mouse format 1006 (sgr) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1015 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Urxvt
                } else {
                    MouseFormat::X10
                };
                log::trace!(
                    "mouse format 1015 (urxvt) {}",
                    if enable { "on" } else { "off" }
                );
            }
            // Mode 1047 — Alternate screen buffer (without cursor save/restore).
            1047 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            // Mode 1048 — Save/restore cursor (DECSC/DECRC).
            1048 => {
                if enable {
                    // Save cursor.
                    let saved = SavedCursor {
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
                } else {
                    // Restore cursor.
                    let saved = if self.using_alt {
                        self.saved_cursor_alt
                    } else {
                        self.saved_cursor_main
                    };
                    if let Some(s) = saved {
                        self.cursor_col = s.col;
                        self.cursor_row = s.row;
                        self.pen = s.pen;
                    }
                }
            }
            1049 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            2004 => self.modes.bracketed_paste = enable,
            _ => {
                log::trace!(
                    "unhandled private mode: {code} {}",
                    if enable { "set" } else { "reset" }
                );
            }
        }
    }
}

/// Parse extended color (38/48/58 ; 5;N or 38/48/58 ; 2;R;G;B).
pub(crate) fn parse_extended_color<'a>(iter: &mut impl Iterator<Item = &'a [u16]>) -> Color {
    match iter.next() {
        Some(&[5]) => {
            if let Some(&[n]) = iter.next() {
                Color::Indexed(n as u8)
            } else {
                Color::Default
            }
        }
        Some(&[2]) => {
            let r = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            let g = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            let b = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            Color::Rgb(r, g, b)
        }
        Some(sub) if sub.len() >= 2 && sub[0] == 2 => {
            let r = sub.get(1).copied().unwrap_or(0) as u8;
            let g = sub.get(2).copied().unwrap_or(0) as u8;
            let b = sub.get(3).copied().unwrap_or(0) as u8;
            Color::Rgb(r, g, b)
        }
        Some(sub) if sub.len() >= 2 && sub[0] == 5 => Color::Indexed(sub[1] as u8),
        _ => Color::Default,
    }
}
