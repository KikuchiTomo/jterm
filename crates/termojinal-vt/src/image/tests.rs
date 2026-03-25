#[cfg(test)]
mod tests {
    use crate::image::kitty::*;
    use crate::image::sixel;
    use crate::image::{ImagePlacement, ImageStore, TerminalImage};
    use base64::Engine as _;

    // -- Kitty header parsing --

    #[test]
    fn test_parse_kitty_header_basic() {
        let cmd = parse_kitty_header("a=T,f=100,i=42,s=100,v=50");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.format, KittyFormat::Png);
        assert_eq!(cmd.image_id, Some(42));
        assert_eq!(cmd.width, Some(100));
        assert_eq!(cmd.height, Some(50));
        assert!(!cmd.more_chunks);
    }

    #[test]
    fn test_parse_kitty_header_defaults() {
        let cmd = parse_kitty_header("");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.format, KittyFormat::Rgba);
        assert_eq!(cmd.image_id, None);
    }

    #[test]
    fn test_parse_kitty_header_transmit() {
        let cmd = parse_kitty_header("a=t,f=32,i=1,s=2,v=2");
        assert_eq!(cmd.action, KittyAction::Transmit);
        assert_eq!(cmd.format, KittyFormat::Rgba);
        assert_eq!(cmd.width, Some(2));
        assert_eq!(cmd.height, Some(2));
    }

    #[test]
    fn test_parse_kitty_header_delete() {
        let cmd = parse_kitty_header("a=d,d=i,i=5");
        assert_eq!(cmd.action, KittyAction::Delete);
        assert_eq!(cmd.delete_target, Some('i'));
        assert_eq!(cmd.image_id, Some(5));
    }

    #[test]
    fn test_parse_kitty_header_chunked() {
        let cmd = parse_kitty_header("a=T,f=100,m=1");
        assert!(cmd.more_chunks);
        let cmd2 = parse_kitty_header("m=0");
        assert!(!cmd2.more_chunks);
    }

    #[test]
    fn test_parse_kitty_header_rgb() {
        let cmd = parse_kitty_header("a=T,f=24,s=4,v=4");
        assert_eq!(cmd.format, KittyFormat::Rgb);
    }

    #[test]
    fn test_parse_kitty_header_place() {
        let cmd = parse_kitty_header("a=p,i=10,c=5,r=3");
        assert_eq!(cmd.action, KittyAction::Place);
        assert_eq!(cmd.image_id, Some(10));
        assert_eq!(cmd.cell_cols, Some(5));
        assert_eq!(cmd.cell_rows, Some(3));
    }

    // -- Kitty accumulator --

    #[test]
    fn test_kitty_accumulator_single_chunk() {
        let mut acc = KittyAccumulator::new();
        // 2x1 RGBA image: red + blue pixels, base64 encoded.
        let pixel_data: [u8; 8] = [255, 0, 0, 255, 0, 0, 255, 255];
        let b64 = base64::engine::general_purpose::STANDARD.encode(pixel_data);

        let result = acc.feed("a=T,f=32,s=2,v=1,i=1", &b64);
        assert!(result.is_some());

        let (cmd, data) = result.expect("should have result");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(data.len(), 8);
        assert_eq!(data, pixel_data.to_vec());
    }

    #[test]
    fn test_kitty_accumulator_chunked() {
        let mut acc = KittyAccumulator::new();
        let pixel_data: [u8; 8] = [255, 0, 0, 255, 0, 255, 0, 255];
        let b64 = base64::engine::general_purpose::STANDARD.encode(pixel_data);

        // Split base64 string into two chunks.
        let mid = b64.len() / 2;
        let chunk1 = &b64[..mid];
        let chunk2 = &b64[mid..];

        let result1 = acc.feed("a=T,f=32,s=2,v=1,i=2,m=1", chunk1);
        assert!(result1.is_none());

        let result2 = acc.feed("m=0", chunk2);
        assert!(result2.is_some());

        let (cmd, data) = result2.expect("should have result");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(data, pixel_data.to_vec());
    }

    // -- Image store --

    #[test]
    fn test_image_store_basic() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let img = TerminalImage {
            id: 1,
            data: vec![255; 32 * 16 * 4],
            width: 32,
            height: 16,
        };
        store.store_image(img);

        assert!(store.get_image(1).is_some());
        assert!(store.get_image(2).is_none());

        store.add_placement(ImagePlacement {
            image_id: 1,
            col: 0,
            row: 0,
            cell_cols: 0,
            cell_rows: 0,
            src_width: 32,
            src_height: 16,
        });

        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].cell_cols, 4); // 32 / 8
        assert_eq!(store.placements()[0].cell_rows, 1); // 16 / 16

        store.delete_image(1);
        assert!(store.get_image(1).is_none());
        assert_eq!(store.placements().len(), 0);
    }

    #[test]
    fn test_image_store_auto_id() {
        let mut store = ImageStore::new();
        assert_eq!(store.next_id(), 1);
        assert_eq!(store.next_id(), 2);
        assert_eq!(store.next_id(), 3);
    }

    #[test]
    fn test_image_store_delete_all() {
        let mut store = ImageStore::new();
        store.store_image(TerminalImage {
            id: 1,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.store_image(TerminalImage {
            id: 2,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.delete_all();
        assert!(store.images().is_empty());
        assert!(store.placements().is_empty());
    }

    // -- Sixel decoding --

    #[test]
    fn test_sixel_decode_simple() {
        // A simple 1x6 red column: sixel char `~` = 0x7E - 0x3F = 0x3F = 63 = all 6 bits set.
        // Set color 0 to red, then draw one `~` character.
        let data = b"#0;2;100;0;0~";
        let result = sixel::decode_sixel(data);
        assert!(result.is_some());
        let img = result.expect("should decode");
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // Check first pixel is red.
        assert_eq!(img.data[0], 255); // R
        assert_eq!(img.data[1], 0); // G
        assert_eq!(img.data[2], 0); // B
        assert_eq!(img.data[3], 255); // A
    }

    #[test]
    fn test_sixel_decode_repeat() {
        // Draw 3 columns of all-on pixels using repeat syntax.
        let data = b"#0;2;0;100;0!3~";
        let result = sixel::decode_sixel(data);
        assert!(result.is_some());
        let img = result.expect("should decode");
        assert_eq!(img.width, 3);
        assert_eq!(img.height, 6);
        // Check pixel at (2, 0) is green.
        let offset = (0 * 3 + 2) * 4;
        assert_eq!(img.data[offset as usize], 0); // R
        assert_eq!(img.data[offset as usize + 1], 255); // G
        assert_eq!(img.data[offset as usize + 2], 0); // B
    }

    #[test]
    fn test_sixel_decode_newline() {
        // Two rows of sixels separated by `-`.
        let data = b"#0;2;100;100;100~-~";
        let result = sixel::decode_sixel(data);
        assert!(result.is_some());
        let img = result.expect("should decode");
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 12);
    }

    #[test]
    fn test_sixel_decode_partial_bits() {
        // Sixel `@` = 0x40 - 0x3F = 1 = only bit 0 set (top pixel only).
        let data = b"#0;2;100;100;0@";
        let result = sixel::decode_sixel(data);
        assert!(result.is_some());
        let img = result.expect("should decode");
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // Pixel (0, 0) should be yellow.
        assert_eq!(img.data[0], 255);
        assert_eq!(img.data[1], 255);
        assert_eq!(img.data[2], 0);
        assert_eq!(img.data[3], 255);
        // Pixel (0, 1) should be transparent (unset).
        assert_eq!(img.data[7], 0); // A channel of pixel (0,1)
    }

    #[test]
    fn test_sixel_decode_empty() {
        let result = sixel::decode_sixel(b"");
        assert!(result.is_none());
    }

    // -- APC extractor --

    #[test]
    fn test_apc_extractor_basic() {
        let mut ext = ApcExtractor::new();
        // ESC _ G payload ESC backslash
        let data = b"\x1b_Ghello\x1b\\world";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 1);
        assert_eq!(result.apc_payloads[0], b"Ghello");
        assert_eq!(result.passthrough, b"world");
    }

    #[test]
    fn test_apc_extractor_no_apc() {
        let mut ext = ApcExtractor::new();
        let data = b"Hello, world!";
        let result = ext.process(data);
        assert!(result.apc_payloads.is_empty());
        assert_eq!(result.passthrough, data.to_vec());
    }

    #[test]
    fn test_apc_extractor_esc_not_apc() {
        let mut ext = ApcExtractor::new();
        // ESC [ is CSI, not APC.
        let data = b"\x1b[1;2H";
        let result = ext.process(data);
        assert!(result.apc_payloads.is_empty());
        assert_eq!(result.passthrough, data.to_vec());
    }

    #[test]
    fn test_apc_extractor_multiple_apc() {
        let mut ext = ApcExtractor::new();
        let data = b"\x1b_Gfirst\x1b\\between\x1b_Gsecond\x1b\\after";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 2);
        assert_eq!(result.apc_payloads[0], b"Gfirst");
        assert_eq!(result.apc_payloads[1], b"Gsecond");
        assert_eq!(result.passthrough, b"betweenafter");
    }

    #[test]
    fn test_apc_extractor_split_across_chunks() {
        let mut ext = ApcExtractor::new();
        // First chunk: start of APC.
        let result1 = ext.process(b"\x1b_Gpar");
        assert!(result1.apc_payloads.is_empty());
        // Second chunk: end of APC.
        let result2 = ext.process(b"tial\x1b\\done");
        assert_eq!(result2.apc_payloads.len(), 1);
        assert_eq!(result2.apc_payloads[0], b"Gpartial");
        assert_eq!(result2.passthrough, b"done");
    }

    #[test]
    fn test_apc_extractor_st_c1() {
        let mut ext = ApcExtractor::new();
        // Using C1 ST (0x9C) instead of ESC \.
        let data = b"\x1b_Gdata\x9crest";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 1);
        assert_eq!(result.apc_payloads[0], b"Gdata");
        assert_eq!(result.passthrough, b"rest");
    }

    // -- Kitty full pipeline --

    #[test]
    fn test_kitty_process_rgba() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        let cmd = KittyCommand {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgba,
            image_id: Some(10),
            width: Some(2),
            height: Some(2),
            transmission: 'd',
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &pixels, &mut store, 5, 3);

        assert!(store.get_image(10).is_some());
        let img = store.get_image(10).expect("image exists");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.data.len(), 16);

        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].col, 5);
        assert_eq!(store.placements()[0].row, 3);
    }

    #[test]
    fn test_kitty_process_rgb() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![
            255, 0, 0, // red
            0, 255, 0, // green
        ];

        let cmd = KittyCommand {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgb,
            image_id: Some(20),
            width: Some(2),
            height: Some(1),
            transmission: 'd',
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &pixels, &mut store, 0, 0);

        let img = store.get_image(20).expect("image exists");
        assert_eq!(img.data.len(), 8); // 2 pixels * 4 bytes (RGBA)
        assert_eq!(img.data[3], 255); // Alpha added
        assert_eq!(img.data[7], 255);
    }

    #[test]
    fn test_kitty_process_delete() {
        let mut store = ImageStore::new();
        store.store_image(TerminalImage {
            id: 1,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.add_placement(ImagePlacement {
            image_id: 1,
            col: 0,
            row: 0,
            cell_cols: 1,
            cell_rows: 1,
            src_width: 2,
            src_height: 2,
        });

        let cmd = KittyCommand {
            action: KittyAction::Delete,
            image_id: Some(1),
            delete_target: Some('i'),
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &[], &mut store, 0, 0);
        assert!(store.get_image(1).is_none());
        assert!(store.placements().is_empty());
    }

    #[test]
    fn test_kitty_transmit_then_place() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![0; 16]; // 2x2 RGBA

        // Transmit only.
        let cmd_t = KittyCommand {
            action: KittyAction::Transmit,
            format: KittyFormat::Rgba,
            image_id: Some(5),
            width: Some(2),
            height: Some(2),
            transmission: 'd',
            ..KittyCommand::default()
        };
        process_kitty_command(&cmd_t, &pixels, &mut store, 0, 0);
        assert!(store.get_image(5).is_some());
        assert!(store.placements().is_empty());

        // Place.
        let cmd_p = KittyCommand {
            action: KittyAction::Place,
            image_id: Some(5),
            cell_cols: Some(3),
            cell_rows: Some(2),
            ..KittyCommand::default()
        };
        process_kitty_command(&cmd_p, &[], &mut store, 10, 5);
        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].col, 10);
        assert_eq!(store.placements()[0].row, 5);
        assert_eq!(store.placements()[0].cell_cols, 3);
        assert_eq!(store.placements()[0].cell_rows, 2);
    }
}
