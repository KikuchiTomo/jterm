#[cfg(test)]
mod tests {
    use crate::cell::Attrs;
    use crate::color::{Color, NamedColor};
    use crate::term::modes::{ClipboardEvent, MouseFormat, MouseMode};
    use crate::term::print::char_width;
    use crate::term::{DrcsFontStore, DrcsGlyph, Terminal};

    fn feed_str(term: &mut Terminal, parser: &mut vte::Parser, s: &str) {
        term.feed(parser, s.as_bytes());
    }

    #[test]
    fn test_print_basic() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "Hello");
        assert_eq!(term.grid().cell(0, 0).c, 'H');
        assert_eq!(term.grid().cell(4, 0).c, 'o');
        assert_eq!(term.cursor_col, 5);
    }

    #[test]
    fn test_newline() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        // LF only moves cursor down; CR returns to column 0.
        feed_str(&mut term, &mut parser, "Line1\r\nLine2");
        assert_eq!(term.grid().cell(0, 0).c, 'L');
        assert_eq!(term.grid().cell(0, 1).c, 'L');
        assert_eq!(term.cursor_row, 1);
    }

    #[test]
    fn test_cursor_movement() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);
    }

    #[test]
    fn test_erase_in_line() {
        let mut term = Terminal::new(10, 1);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "ABCDEFGHIJ");
        feed_str(&mut term, &mut parser, "\x1b[6G\x1b[K");
        assert_eq!(term.grid().cell(4, 0).c, 'E');
        assert_eq!(term.grid().cell(5, 0).c, ' ');
    }

    #[test]
    fn test_alternate_screen() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "Main");
        assert_eq!(term.grid().cell(0, 0).c, 'M');

        feed_str(&mut term, &mut parser, "\x1b[?1049h");
        assert!(term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, ' ');

        feed_str(&mut term, &mut parser, "Alt");
        assert_eq!(term.grid().cell(0, 0).c, 'A');

        feed_str(&mut term, &mut parser, "\x1b[?1049l");
        assert!(!term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, 'M');
    }

    #[test]
    fn test_sgr_colors() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[1;31;44mX");
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, 'X');
        assert_eq!(cell.fg, Color::Named(NamedColor::Red));
        assert_eq!(cell.bg, Color::Named(NamedColor::Blue));
        assert!(cell.attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn test_sgr_truecolor() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[38;2;255;128;0mX");
        assert_eq!(term.grid().cell(0, 0).fg, Color::Rgb(255, 128, 0));
    }

    #[test]
    fn test_sgr_256color() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[38;5;196mX");
        assert_eq!(term.grid().cell(0, 0).fg, Color::Indexed(196));
    }

    #[test]
    fn test_scroll_region() {
        let mut term = Terminal::new(80, 10);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[3;7r");
        assert_eq!(term.scroll_top, 2);
        assert_eq!(term.scroll_bottom, 6);
    }

    #[test]
    fn test_bracketed_paste_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.bracketed_paste);
        feed_str(&mut term, &mut parser, "\x1b[?2004h");
        assert!(term.modes.bracketed_paste);
        feed_str(&mut term, &mut parser, "\x1b[?2004l");
        assert!(!term.modes.bracketed_paste);
    }

    #[test]
    fn test_cursor_save_restore() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        feed_str(&mut term, &mut parser, "\x1b7");
        feed_str(&mut term, &mut parser, "\x1b[1;1H");
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 0);
        feed_str(&mut term, &mut parser, "\x1b8");
        assert_eq!(term.cursor_col, 9);
        assert_eq!(term.cursor_row, 4);
    }

    #[test]
    fn test_wide_char() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "A\u{6f22}B");
        assert_eq!(term.grid().cell(0, 0).c, 'A');
        assert_eq!(term.grid().cell(0, 0).width, 1);
        assert_eq!(term.grid().cell(1, 0).c, '\u{6f22}');
        assert_eq!(term.grid().cell(1, 0).width, 2);
        assert_eq!(term.grid().cell(2, 0).width, 0);
        assert_eq!(term.grid().cell(3, 0).c, 'B');
    }

    #[test]
    fn test_emoji() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "A\u{1F600}B");
        assert_eq!(term.grid().cell(0, 0).c, 'A');
        let emoji_cell = term.grid().cell(1, 0);
        eprintln!(
            "emoji cell: c={:?} U+{:04X} width={}",
            emoji_cell.c, emoji_cell.c as u32, emoji_cell.width
        );
        assert_eq!(emoji_cell.c, '\u{1F600}');
        assert_eq!(emoji_cell.width, 2);
        assert_eq!(term.grid().cell(2, 0).width, 0); // continuation
        assert_eq!(term.grid().cell(3, 0).c, 'B');
    }

    // --- Mouse tracking mode tests ---

    #[test]
    fn test_mouse_mode_click() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_mode, MouseMode::None);

        // Enable mode 1000 (click tracking).
        feed_str(&mut term, &mut parser, "\x1b[?1000h");
        assert_eq!(term.modes.mouse_mode, MouseMode::Click);

        // Disable mode 1000.
        feed_str(&mut term, &mut parser, "\x1b[?1000l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_mode_button_motion() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1002h");
        assert_eq!(term.modes.mouse_mode, MouseMode::ButtonMotion);

        feed_str(&mut term, &mut parser, "\x1b[?1002l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_mode_any_motion() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1003h");
        assert_eq!(term.modes.mouse_mode, MouseMode::AnyMotion);

        feed_str(&mut term, &mut parser, "\x1b[?1003l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_format_sgr() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);

        feed_str(&mut term, &mut parser, "\x1b[?1006h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Sgr);

        feed_str(&mut term, &mut parser, "\x1b[?1006l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    #[test]
    fn test_mouse_format_utf8() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1005h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Utf8);

        feed_str(&mut term, &mut parser, "\x1b[?1005l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    #[test]
    fn test_mouse_format_urxvt() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1015h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Urxvt);

        feed_str(&mut term, &mut parser, "\x1b[?1015l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    // --- Focus events mode test ---

    #[test]
    fn test_focus_events_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.focus_events);

        feed_str(&mut term, &mut parser, "\x1b[?1004h");
        assert!(term.modes.focus_events);

        feed_str(&mut term, &mut parser, "\x1b[?1004l");
        assert!(!term.modes.focus_events);
    }

    // --- Kitty keyboard protocol tests ---

    #[test]
    fn test_kitty_keyboard_push_pop() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.kitty_keyboard_mode(), 0);

        // Push flags=1.
        feed_str(&mut term, &mut parser, "\x1b[>1u");
        assert_eq!(term.kitty_keyboard_mode(), 1);

        // Push flags=3.
        feed_str(&mut term, &mut parser, "\x1b[>3u");
        assert_eq!(term.kitty_keyboard_mode(), 3);

        // Pop one.
        feed_str(&mut term, &mut parser, "\x1b[<1u");
        assert_eq!(term.kitty_keyboard_mode(), 1);

        // Pop one more.
        feed_str(&mut term, &mut parser, "\x1b[<1u");
        assert_eq!(term.kitty_keyboard_mode(), 0);
    }

    #[test]
    fn test_kitty_keyboard_pop_multiple() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[>1u");
        feed_str(&mut term, &mut parser, "\x1b[>2u");
        feed_str(&mut term, &mut parser, "\x1b[>3u");
        assert_eq!(term.kitty_keyboard_mode(), 3);

        // Pop 2 at once.
        feed_str(&mut term, &mut parser, "\x1b[<2u");
        assert_eq!(term.kitty_keyboard_mode(), 1);
    }

    #[test]
    fn test_kitty_keyboard_pop_empty_stack() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Popping from empty stack should be safe.
        feed_str(&mut term, &mut parser, "\x1b[<5u");
        assert_eq!(term.kitty_keyboard_mode(), 0);
    }

    #[test]
    fn test_kitty_keyboard_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query should not crash (just logs).
        feed_str(&mut term, &mut parser, "\x1b[?u");
        assert_eq!(term.kitty_keyboard_mode(), 0);

        feed_str(&mut term, &mut parser, "\x1b[>5u");
        feed_str(&mut term, &mut parser, "\x1b[?u");
        assert_eq!(term.kitty_keyboard_mode(), 5);
    }

    // --- OSC 8 hyperlinks tests ---

    #[test]
    fn test_osc8_hyperlink() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Start hyperlink: OSC 8 ; ; https://example.com ST
        feed_str(
            &mut term,
            &mut parser,
            "\x1b]8;;https://example.com\x1b\\",
        );
        // Print some text while hyperlink is active.
        feed_str(&mut term, &mut parser, "link");

        assert!(term.grid().cell(0, 0).hyperlink);
        assert!(term.grid().cell(1, 0).hyperlink);
        assert!(term.grid().cell(2, 0).hyperlink);
        assert!(term.grid().cell(3, 0).hyperlink);

        // End hyperlink: OSC 8 ; ; ST
        feed_str(&mut term, &mut parser, "\x1b]8;;\x1b\\");
        // Print text after hyperlink ends.
        feed_str(&mut term, &mut parser, "text");

        assert!(!term.grid().cell(4, 0).hyperlink);
        assert!(!term.grid().cell(5, 0).hyperlink);
    }

    #[test]
    fn test_osc8_hyperlink_not_set_by_default() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "hello");
        assert!(!term.grid().cell(0, 0).hyperlink);
        assert!(!term.grid().cell(4, 0).hyperlink);
    }

    // --- OSC 52 clipboard tests ---

    #[test]
    fn test_osc52_set_clipboard() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(term.clipboard_event.is_none());

        // "hello" in base64 is "aGVsbG8="
        // OSC 52 ; c ; aGVsbG8= ST
        feed_str(&mut term, &mut parser, "\x1b]52;c;aGVsbG8=\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Set { selection, data }) => {
                assert_eq!(selection, "c");
                assert_eq!(data, "hello");
            }
            other => panic!("expected ClipboardEvent::Set, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_query_clipboard() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // OSC 52 ; c ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]52;c;?\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Query { selection }) => {
                assert_eq!(selection, "c");
            }
            other => panic!("expected ClipboardEvent::Query, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_primary_selection() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // "test" base64 = "dGVzdA=="
        feed_str(&mut term, &mut parser, "\x1b]52;p;dGVzdA==\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Set { selection, data }) => {
                assert_eq!(selection, "p");
                assert_eq!(data, "test");
            }
            other => panic!("expected ClipboardEvent::Set, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_invalid_base64() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Invalid base64 — should not set clipboard_event.
        feed_str(&mut term, &mut parser, "\x1b]52;c;!!!invalid!!!\x1b\\");

        assert!(term.clipboard_event.is_none());
    }

    #[test]
    fn test_char_width_basic() {
        // ASCII characters are always 1-cell wide.
        assert_eq!(char_width('A', false), 1);
        assert_eq!(char_width('A', true), 1);

        // CJK ideographs are always 2-cell wide.
        assert_eq!(char_width('\u{3042}', false), 2);
        assert_eq!(char_width('\u{3042}', true), 2);
        assert_eq!(char_width('\u{6f22}', false), 2);
        assert_eq!(char_width('\u{6f22}', true), 2);

        // Fullwidth forms are always 2-cell wide.
        assert_eq!(char_width('\u{ff01}', false), 2);
        assert_eq!(char_width('\u{ff01}', true), 2);
    }

    #[test]
    fn test_char_width_ambiguous() {
        // U+25EF LARGE CIRCLE — East Asian Width: Ambiguous.
        // Narrow in Western locales, wide in CJK locales.
        assert_eq!(char_width('\u{25EF}', false), 1);
        assert_eq!(char_width('\u{25EF}', true), 2);

        // Other common ambiguous-width characters:
        // U+25CB WHITE CIRCLE
        assert_eq!(char_width('\u{25CB}', false), 1);
        assert_eq!(char_width('\u{25CB}', true), 2);

        // U+25CF BLACK CIRCLE
        assert_eq!(char_width('\u{25CF}', false), 1);
        assert_eq!(char_width('\u{25CF}', true), 2);

        // U+25A0 BLACK SQUARE
        assert_eq!(char_width('\u{25A0}', false), 1);
        assert_eq!(char_width('\u{25A0}', true), 2);

        // U+25B3 WHITE UP-POINTING TRIANGLE
        assert_eq!(char_width('\u{25B3}', false), 1);
        assert_eq!(char_width('\u{25B3}', true), 2);

        // U+2605 BLACK STAR
        assert_eq!(char_width('\u{2605}', false), 1);
        assert_eq!(char_width('\u{2605}', true), 2);

        // U+2460 CIRCLED DIGIT ONE
        assert_eq!(char_width('\u{2460}', false), 1);
        assert_eq!(char_width('\u{2460}', true), 2);
    }

    #[test]
    fn test_cjk_width_terminal_print() {
        // Test that U+25EF is stored as width-2 when cjk_width is enabled.
        let mut term = Terminal::new(80, 24);
        term.set_cjk_width(true);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\u{25EF}");

        // The character should be stored in cell (0, 0) with width 2.
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, '\u{25EF}');
        assert_eq!(cell.width, 2);

        // Cell (1, 0) should be a continuation cell (width 0, NUL char).
        let cont = term.grid().cell(1, 0);
        assert_eq!(cont.c, '\0');
        assert_eq!(cont.width, 0);

        // Cursor should be at column 2 (after the 2-cell wide character).
        assert_eq!(term.cursor_col, 2);
    }

    #[test]
    fn test_narrow_width_terminal_print() {
        // Test that U+25EF is stored as width-1 when cjk_width is disabled.
        let mut term = Terminal::new(80, 24);
        term.set_cjk_width(false);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\u{25EF}");

        // The character should be stored in cell (0, 0) with width 1.
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, '\u{25EF}');
        assert_eq!(cell.width, 1);

        // Cursor should be at column 1.
        assert_eq!(term.cursor_col, 1);
    }

    // =========================================================================
    // Tests for Issue 9, 16, 17 — new terminal protocol features
    // =========================================================================

    // --- DECCKM (mode 1) ---

    #[test]
    fn test_decckm_application_cursor_keys() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.application_cursor_keys);

        // Enable DECCKM.
        feed_str(&mut term, &mut parser, "\x1b[?1h");
        assert!(term.modes.application_cursor_keys);

        // Disable DECCKM.
        feed_str(&mut term, &mut parser, "\x1b[?1l");
        assert!(!term.modes.application_cursor_keys);
    }

    // --- DECOM (mode 6) ---

    #[test]
    fn test_decom_origin_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.origin_mode);

        // Set scroll region.
        feed_str(&mut term, &mut parser, "\x1b[5;20r");
        // Move cursor somewhere.
        feed_str(&mut term, &mut parser, "\x1b[10;10H");
        assert_eq!(term.cursor_row, 9);
        assert_eq!(term.cursor_col, 9);

        // Enable origin mode — cursor should move to scroll region origin.
        feed_str(&mut term, &mut parser, "\x1b[?6h");
        assert!(term.modes.origin_mode);
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 4); // scroll_top = 4 (row 5, 0-indexed)

        // Disable origin mode — cursor should move to absolute origin.
        feed_str(&mut term, &mut parser, "\x1b[?6l");
        assert!(!term.modes.origin_mode);
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 0);
    }

    // --- X10 mouse (mode 9) ---

    #[test]
    fn test_x10_mouse_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_mode, MouseMode::None);

        feed_str(&mut term, &mut parser, "\x1b[?9h");
        assert_eq!(term.modes.mouse_mode, MouseMode::Click);

        feed_str(&mut term, &mut parser, "\x1b[?9l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    // --- DECSDM (mode 80) ---

    #[test]
    fn test_decsdm_sixel_display_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.sixel_display_mode);

        feed_str(&mut term, &mut parser, "\x1b[?80h");
        assert!(term.modes.sixel_display_mode);

        feed_str(&mut term, &mut parser, "\x1b[?80l");
        assert!(!term.modes.sixel_display_mode);
    }

    // --- Mode 1047 (alt screen without cursor save) ---

    #[test]
    fn test_mode_1047_alt_screen() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "Main");
        assert_eq!(term.grid().cell(0, 0).c, 'M');

        // Enter alt screen via mode 1047.
        feed_str(&mut term, &mut parser, "\x1b[?1047h");
        assert!(term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, ' '); // Alt screen is clear.

        // Leave alt screen via mode 1047.
        feed_str(&mut term, &mut parser, "\x1b[?1047l");
        assert!(!term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, 'M'); // Main screen restored.
    }

    // --- Mode 1048 (save/restore cursor) ---

    #[test]
    fn test_mode_1048_save_restore_cursor() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Move cursor to (9, 4).
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);

        // Save cursor via mode 1048.
        feed_str(&mut term, &mut parser, "\x1b[?1048h");

        // Move cursor elsewhere.
        feed_str(&mut term, &mut parser, "\x1b[1;1H");
        assert_eq!(term.cursor_row, 0);
        assert_eq!(term.cursor_col, 0);

        // Restore cursor via mode 1048.
        feed_str(&mut term, &mut parser, "\x1b[?1048l");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);
    }

    // --- LNM (ANSI mode 20) ---

    #[test]
    fn test_lnm_linefeed_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.linefeed_mode);

        // Set LNM.
        feed_str(&mut term, &mut parser, "\x1b[20h");
        assert!(term.modes.linefeed_mode);

        // Move cursor to column 5.
        feed_str(&mut term, &mut parser, "\x1b[1;6H");
        assert_eq!(term.cursor_col, 5);

        // LF should also do CR when LNM is set.
        feed_str(&mut term, &mut parser, "\n");
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 1);

        // Reset LNM.
        feed_str(&mut term, &mut parser, "\x1b[20l");
        assert!(!term.modes.linefeed_mode);

        // Move cursor to column 5 again.
        feed_str(&mut term, &mut parser, "\x1b[3;6H");
        assert_eq!(term.cursor_col, 5);

        // LF should NOT do CR when LNM is off.
        feed_str(&mut term, &mut parser, "\n");
        assert_eq!(term.cursor_col, 5);
    }

    // --- OSC 10/11/12 Color Queries ---

    #[test]
    fn test_osc10_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query foreground color: OSC 10 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]10;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).expect("valid utf8");
        assert!(resp.contains("]10;rgb:"));
    }

    #[test]
    fn test_osc11_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query background color: OSC 11 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]11;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).expect("valid utf8");
        assert!(resp.contains("]11;rgb:"));
    }

    #[test]
    fn test_osc12_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query cursor color: OSC 12 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]12;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).expect("valid utf8");
        assert!(resp.contains("]12;rgb:"));
    }

    #[test]
    fn test_osc10_set_color_no_response() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Setting a color should not produce a response.
        feed_str(
            &mut term,
            &mut parser,
            "\x1b]10;rgb:ffff/0000/0000\x1b\\",
        );

        assert!(!term.has_pending_responses());
    }

    // --- DSR (Device Status Report) ---

    #[test]
    fn test_dsr_status_report() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // DSR status request (5).
        feed_str(&mut term, &mut parser, "\x1b[5n");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[0n");
    }

    #[test]
    fn test_dsr_cursor_position_report() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Move cursor to (9, 4).
        feed_str(&mut term, &mut parser, "\x1b[5;10H");

        // DSR cursor position request (6).
        feed_str(&mut term, &mut parser, "\x1b[6n");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[5;10R");
    }

    // --- DA (Device Attributes) ---

    #[test]
    fn test_da_primary() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Primary DA: CSI 0 c
        feed_str(&mut term, &mut parser, "\x1b[0c");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).expect("valid utf8");
        assert!(resp.starts_with("\x1b[?"));
        // Should indicate VT220, Sixel support.
        assert!(resp.contains("62"));
        assert!(resp.contains("4"));
    }

    #[test]
    fn test_da_secondary() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Secondary DA: CSI > 0 c
        feed_str(&mut term, &mut parser, "\x1b[>0c");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).expect("valid utf8");
        assert!(resp.starts_with("\x1b[>"));
    }

    // --- DRCS / DECDLD ---

    #[test]
    fn test_drcs_font_store() {
        let mut store = DrcsFontStore::new();
        assert!(store.is_empty());

        store.set_glyph(
            0,
            0,
            DrcsGlyph {
                bitmap: vec![0xFF, 0x00],
                width: 10,
                height: 20,
            },
        );

        assert!(!store.is_empty());
        assert!(store.get_glyph(0, 0).is_some());
        assert!(store.get_glyph(0, 1).is_none());
        assert!(store.get_glyph(1, 0).is_none());

        let glyph = store.get_glyph(0, 0).expect("glyph exists");
        assert_eq!(glyph.width, 10);
        assert_eq!(glyph.height, 20);

        // Erase font 0.
        store.erase_font(0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_drcs_font_store_erase_all() {
        let mut store = DrcsFontStore::new();
        store.set_glyph(
            0,
            0,
            DrcsGlyph {
                bitmap: vec![0xFF],
                width: 8,
                height: 16,
            },
        );
        store.set_glyph(
            1,
            0,
            DrcsGlyph {
                bitmap: vec![0xFF],
                width: 8,
                height: 16,
            },
        );
        assert_eq!(store.glyphs().len(), 2);

        store.erase_all();
        assert!(store.is_empty());
    }

    #[test]
    fn test_decdld_basic_dcs_hook() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Send DECDLD: DCS 0;0;0;10;0;0;20;0 { B <data> ST
        // This defines font 0, starting at char 0, 10x20 cell.
        // Minimal sixel data: one glyph with a single column of all-on pixels.
        // DCS format: ESC P <params> { <Dscs> <data> ESC backslash
        feed_str(&mut term, &mut parser, "\x1bP0;0;0;10;0;0;20;0{B~\x1b\\");

        // The DRCS font store should now have at least one glyph.
        assert!(!term.drcs_fonts.is_empty());
    }

    // --- Response queue ---

    #[test]
    fn test_response_queue_drain() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.has_pending_responses());

        // Trigger two responses.
        feed_str(&mut term, &mut parser, "\x1b[5n"); // DSR status
        assert!(term.has_pending_responses());

        // Drain.
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert!(!term.has_pending_responses());
    }

    // --- Multiple modes in single sequence ---

    #[test]
    fn test_multiple_private_modes() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Set multiple modes at once: DECCKM + DECAWM + bracketed paste.
        feed_str(&mut term, &mut parser, "\x1b[?1;7;2004h");
        assert!(term.modes.application_cursor_keys);
        assert!(term.modes.auto_wrap);
        assert!(term.modes.bracketed_paste);

        // Reset them.
        feed_str(&mut term, &mut parser, "\x1b[?1;7;2004l");
        assert!(!term.modes.application_cursor_keys);
        assert!(!term.modes.auto_wrap);
        assert!(!term.modes.bracketed_paste);
    }
}
