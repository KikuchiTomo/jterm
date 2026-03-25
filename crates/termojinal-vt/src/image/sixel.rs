//! Sixel Graphics decoding.

use std::collections::HashMap;

use super::{DecodedImage, ImagePlacement, ImageStore, TerminalImage};

/// Sixel color register.
#[derive(Debug, Clone, Copy)]
struct SixelColor {
    r: u8,
    g: u8,
    b: u8,
}

/// Decode sixel data into an RGBA pixel buffer.
///
/// Sixel format: each character in the range `?` (0x3F) to `~` (0x7E)
/// encodes 6 vertical pixels. Color is set via `#<register>` commands.
/// `$` returns to the beginning of the current row of sixels.
/// `-` moves to the next row of sixels (6 pixels down).
/// `!<count><char>` is a repeat introducer.
pub fn decode_sixel(data: &[u8]) -> Option<DecodedImage> {
    // Default palette: start with VGA 16 colors.
    let mut palette: HashMap<u16, SixelColor> = HashMap::new();
    init_default_sixel_palette(&mut palette);

    let mut current_color: u16 = 0;
    let mut x: u32 = 0;
    let mut y: u32 = 0;
    let mut max_x: u32 = 0;
    let mut max_y: u32 = 0;

    // First pass: determine dimensions.
    {
        let mut px = 0u32;
        let mut py = 0u32;
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            match b {
                b'$' => {
                    px = 0;
                }
                b'-' => {
                    px = 0;
                    py += 6;
                }
                b'#' => {
                    // Skip color command.
                    i += 1;
                    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                        i += 1;
                    }
                    continue;
                }
                b'!' => {
                    // Repeat: !<count><char>
                    i += 1;
                    let mut count = 0u32;
                    while i < data.len() && data[i].is_ascii_digit() {
                        count = count * 10 + (data[i] - b'0') as u32;
                        i += 1;
                    }
                    if i < data.len() && data[i] >= 0x3F && data[i] <= 0x7E {
                        px += count;
                        if px > max_x {
                            max_x = px;
                        }
                        if py + 6 > max_y {
                            max_y = py + 6;
                        }
                    }
                    i += 1;
                    continue;
                }
                0x3F..=0x7E => {
                    px += 1;
                    if px > max_x {
                        max_x = px;
                    }
                    if py + 6 > max_y {
                        max_y = py + 6;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    if max_x == 0 || max_y == 0 {
        return None;
    }

    let width = max_x;
    let height = max_y;
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // Second pass: draw pixels.
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            b'$' => {
                x = 0;
            }
            b'-' => {
                x = 0;
                y += 6;
            }
            b'#' => {
                // Color command: #<register> or #<register>;<type>;<p1>;<p2>;<p3>
                i += 1;
                let mut reg = 0u16;
                while i < data.len() && data[i].is_ascii_digit() {
                    reg = reg * 10 + (data[i] - b'0') as u16;
                    i += 1;
                }
                if i < data.len() && data[i] == b';' {
                    // Color definition.
                    i += 1;
                    let mut params = Vec::new();
                    let mut num = 0u16;
                    let mut has_num = false;
                    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                        if data[i] == b';' {
                            params.push(num);
                            num = 0;
                            has_num = false;
                        } else {
                            num = num * 10 + (data[i] - b'0') as u16;
                            has_num = true;
                        }
                        i += 1;
                    }
                    if has_num {
                        params.push(num);
                    }
                    if params.len() >= 4 {
                        let color_type = params[0];
                        let (r, g, b_val) = if color_type == 2 {
                            // RGB percentages (0-100).
                            let rp = params.get(1).copied().unwrap_or(0).min(100);
                            let gp = params.get(2).copied().unwrap_or(0).min(100);
                            let bp = params.get(3).copied().unwrap_or(0).min(100);
                            (
                                (rp as u32 * 255 / 100) as u8,
                                (gp as u32 * 255 / 100) as u8,
                                (bp as u32 * 255 / 100) as u8,
                            )
                        } else if color_type == 1 {
                            // HLS (Hue/Lightness/Saturation).
                            let h = params.get(1).copied().unwrap_or(0);
                            let l = params.get(2).copied().unwrap_or(0);
                            let s = params.get(3).copied().unwrap_or(0);
                            hls_to_rgb(h, l, s)
                        } else {
                            (0, 0, 0)
                        };
                        palette.insert(reg, SixelColor { r, g, b: b_val });
                    }
                }
                current_color = reg;
                continue;
            }
            b'!' => {
                // Repeat: !<count><char>
                i += 1;
                let mut count = 0u32;
                while i < data.len() && data[i].is_ascii_digit() {
                    count = count * 10 + (data[i] - b'0') as u32;
                    i += 1;
                }
                if i < data.len() && data[i] >= 0x3F && data[i] <= 0x7E {
                    let sixel_val = data[i] - 0x3F;
                    let color = palette.get(&current_color).copied().unwrap_or(SixelColor {
                        r: 255,
                        g: 255,
                        b: 255,
                    });
                    for rep in 0..count {
                        draw_sixel_column(
                            &mut pixels,
                            width,
                            height,
                            x + rep,
                            y,
                            sixel_val,
                            &color,
                        );
                    }
                    x += count;
                }
                i += 1;
                continue;
            }
            0x3F..=0x7E => {
                let sixel_val = b - 0x3F;
                let color = palette.get(&current_color).copied().unwrap_or(SixelColor {
                    r: 255,
                    g: 255,
                    b: 255,
                });
                draw_sixel_column(&mut pixels, width, height, x, y, sixel_val, &color);
                x += 1;
            }
            _ => {}
        }
        i += 1;
    }

    Some(DecodedImage {
        data: pixels,
        width,
        height,
    })
}

/// Draw a single sixel column (6 vertical pixels) into the pixel buffer.
fn draw_sixel_column(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    sixel_val: u8,
    color: &SixelColor,
) {
    for bit in 0..6u32 {
        if sixel_val & (1 << bit) != 0 {
            let py = y + bit;
            if x < width && py < height {
                let offset = ((py * width + x) * 4) as usize;
                if offset + 3 < pixels.len() {
                    pixels[offset] = color.r;
                    pixels[offset + 1] = color.g;
                    pixels[offset + 2] = color.b;
                    pixels[offset + 3] = 255;
                }
            }
        }
    }
}

/// Initialize the default VGA 16-color sixel palette.
fn init_default_sixel_palette(palette: &mut HashMap<u16, SixelColor>) {
    let vga16: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0: black
        (0, 0, 170),     // 1: blue
        (170, 0, 0),     // 2: red
        (0, 170, 0),     // 3: green
        (170, 0, 170),   // 4: magenta
        (0, 170, 170),   // 5: cyan
        (170, 170, 0),   // 6: yellow
        (170, 170, 170), // 7: white
        (85, 85, 85),    // 8: bright black
        (85, 85, 255),   // 9: bright blue
        (255, 85, 85),   // 10: bright red
        (85, 255, 85),   // 11: bright green
        (255, 85, 255),  // 12: bright magenta
        (85, 255, 255),  // 13: bright cyan
        (255, 255, 85),  // 14: bright yellow
        (255, 255, 255), // 15: bright white
    ];
    for (i, (r, g, b)) in vga16.iter().enumerate() {
        palette.insert(
            i as u16,
            SixelColor {
                r: *r,
                g: *g,
                b: *b,
            },
        );
    }
}

/// Convert HLS (Hue 0-360, Lightness 0-100, Saturation 0-100) to RGB.
pub(crate) fn hls_to_rgb(h: u16, l: u16, s: u16) -> (u8, u8, u8) {
    let h = (h % 360) as f64;
    let l = l.min(100) as f64 / 100.0;
    let s = s.min(100) as f64 / 100.0;

    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hk = h / 360.0;

    let to_rgb = |t: f64| -> u8 {
        let t = if t < 0.0 {
            t + 1.0
        } else if t > 1.0 {
            t - 1.0
        } else {
            t
        };
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().min(255.0).max(0.0) as u8
    };

    (to_rgb(hk + 1.0 / 3.0), to_rgb(hk), to_rgb(hk - 1.0 / 3.0))
}

/// Process a complete Sixel DCS sequence.
///
/// The `data` should be the sixel data portion (after the `q` introducer).
pub fn process_sixel(data: &[u8], store: &mut ImageStore, cursor_col: usize, cursor_row: usize) {
    match decode_sixel(data) {
        Some(decoded) => {
            let id = store.next_id();
            let w = decoded.width;
            let h = decoded.height;
            store.store_image(TerminalImage {
                id,
                data: decoded.data,
                width: w,
                height: h,
            });
            store.add_placement(ImagePlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row as isize,
                cell_cols: 0,
                cell_rows: 0,
                src_width: w,
                src_height: h,
            });
            log::debug!("sixel: decoded and placed id={id} {w}x{h}");
        }
        None => {
            log::trace!("sixel: failed to decode data");
        }
    }
}
