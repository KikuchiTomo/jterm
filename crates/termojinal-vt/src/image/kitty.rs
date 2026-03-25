//! Kitty Graphics Protocol implementation.

use base64::Engine as _;

use super::{decode_png, DecodedImage, ImagePlacement, ImageStore, TerminalImage};

/// Kitty Graphics action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    /// `a=t` — Transmit image data (store but don't display).
    Transmit,
    /// `a=T` — Transmit and display at cursor.
    TransmitAndDisplay,
    /// `a=p` — Display a previously transmitted image.
    Place,
    /// `a=d` — Delete image(s).
    Delete,
    /// `a=q` — Query support.
    Query,
}

/// Kitty Graphics data format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFormat {
    /// `f=24` — RGB (3 bytes per pixel).
    Rgb,
    /// `f=32` — RGBA (4 bytes per pixel).
    Rgba,
    /// `f=100` — PNG compressed.
    Png,
}

/// Parsed Kitty Graphics command header.
#[derive(Debug, Clone)]
pub struct KittyCommand {
    pub action: KittyAction,
    pub format: KittyFormat,
    /// Image ID (`i=`).
    pub image_id: Option<u32>,
    /// Pixel width (`s=`).
    pub width: Option<u32>,
    /// Pixel height (`v=`).
    pub height: Option<u32>,
    /// Cell columns for placement (`c=`).
    pub cell_cols: Option<usize>,
    /// Cell rows for placement (`r=`).
    pub cell_rows: Option<usize>,
    /// More data chunks follow (`m=1`).
    pub more_chunks: bool,
    /// Transmission type (only `d` = direct is supported).
    pub transmission: char,
    /// Delete target for `a=d`.
    pub delete_target: Option<char>,
    /// Quiet mode (`q=`). 0 = normal, 1 = suppress OK, 2 = suppress all.
    pub quiet: u8,
}

impl Default for KittyCommand {
    fn default() -> Self {
        Self {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgba,
            image_id: None,
            width: None,
            height: None,
            cell_cols: None,
            cell_rows: None,
            more_chunks: false,
            transmission: 'd',
            delete_target: None,
            quiet: 0,
        }
    }
}

/// Parse a Kitty Graphics payload header (everything before the `;`).
///
/// The header consists of comma-separated `key=value` pairs.
pub fn parse_kitty_header(header: &str) -> KittyCommand {
    let mut cmd = KittyCommand::default();

    for kv in header.split(',') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k.trim(),
            None => continue,
        };
        let value = parts.next().unwrap_or("").trim();

        match key {
            "a" => {
                cmd.action = match value {
                    "t" => KittyAction::Transmit,
                    "T" => KittyAction::TransmitAndDisplay,
                    "p" => KittyAction::Place,
                    "d" => KittyAction::Delete,
                    "q" => KittyAction::Query,
                    _ => KittyAction::TransmitAndDisplay,
                };
            }
            "f" => {
                cmd.format = match value {
                    "24" => KittyFormat::Rgb,
                    "32" => KittyFormat::Rgba,
                    "100" => KittyFormat::Png,
                    _ => KittyFormat::Rgba,
                };
            }
            "t" => {
                cmd.transmission = value.chars().next().unwrap_or('d');
            }
            "i" => {
                cmd.image_id = value.parse().ok();
            }
            "s" => {
                cmd.width = value.parse().ok();
            }
            "v" => {
                cmd.height = value.parse().ok();
            }
            "c" => {
                cmd.cell_cols = value.parse().ok();
            }
            "r" => {
                cmd.cell_rows = value.parse().ok();
            }
            "m" => {
                cmd.more_chunks = value == "1";
            }
            "d" => {
                cmd.delete_target = value.chars().next();
            }
            "q" => {
                cmd.quiet = value.parse().unwrap_or(0);
            }
            _ => {
                log::trace!("kitty graphics: unknown key '{key}'");
            }
        }
    }

    cmd
}

/// Accumulator for chunked Kitty Graphics transmissions.
#[derive(Debug, Default)]
pub struct KittyAccumulator {
    /// The command header from the first chunk.
    pub command: Option<KittyCommand>,
    /// Accumulated base64-encoded data across chunks.
    pub base64_data: String,
}

impl KittyAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a parsed APC payload. Returns `Some(KittyCommand, decoded_bytes)` when
    /// the final chunk arrives (`m=0`).
    pub fn feed(&mut self, header: &str, base64_chunk: &str) -> Option<(KittyCommand, Vec<u8>)> {
        let cmd = parse_kitty_header(header);

        if self.command.is_none() {
            self.command = Some(cmd.clone());
        }

        self.base64_data.push_str(base64_chunk);

        if cmd.more_chunks {
            // Update more_chunks on the stored command.
            if let Some(ref mut stored) = self.command {
                stored.more_chunks = true;
            }
            return None;
        }

        // Final chunk: decode all accumulated data.
        let full_cmd = self.command.take().unwrap_or(cmd);
        let b64 = std::mem::take(&mut self.base64_data);

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap_or_default();

        Some((full_cmd, decoded))
    }

    /// Reset the accumulator (e.g., on error).
    pub fn reset(&mut self) {
        self.command = None;
        self.base64_data.clear();
    }
}

/// Process a complete Kitty Graphics command with decoded payload.
///
/// `cursor_col` and `cursor_row` are the current cursor position for placement.
pub fn process_kitty_command(
    cmd: &KittyCommand,
    payload: &[u8],
    store: &mut ImageStore,
    cursor_col: usize,
    cursor_row: usize,
) {
    match cmd.action {
        KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
            // Only support direct transmission (t=d).
            if cmd.transmission != 'd' {
                log::trace!(
                    "kitty graphics: unsupported transmission type '{}'",
                    cmd.transmission
                );
                return;
            }

            let rgba_data = match cmd.format {
                KittyFormat::Png => match decode_png(payload) {
                    Some(img) => img,
                    None => {
                        log::trace!("kitty graphics: failed to decode PNG");
                        return;
                    }
                },
                KittyFormat::Rgba => {
                    let w = cmd.width.unwrap_or(0);
                    let h = cmd.height.unwrap_or(0);
                    if w == 0 || h == 0 {
                        log::trace!("kitty graphics: RGBA format requires s= and v=");
                        return;
                    }
                    let expected = (w * h * 4) as usize;
                    if payload.len() != expected {
                        log::trace!(
                            "kitty graphics: RGBA size mismatch: expected {expected}, got {}",
                            payload.len()
                        );
                        return;
                    }
                    DecodedImage {
                        data: payload.to_vec(),
                        width: w,
                        height: h,
                    }
                }
                KittyFormat::Rgb => {
                    let w = cmd.width.unwrap_or(0);
                    let h = cmd.height.unwrap_or(0);
                    if w == 0 || h == 0 {
                        log::trace!("kitty graphics: RGB format requires s= and v=");
                        return;
                    }
                    let expected = (w * h * 3) as usize;
                    if payload.len() != expected {
                        log::trace!(
                            "kitty graphics: RGB size mismatch: expected {expected}, got {}",
                            payload.len()
                        );
                        return;
                    }
                    // Convert RGB to RGBA.
                    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                    for chunk in payload.chunks_exact(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    DecodedImage {
                        data: rgba,
                        width: w,
                        height: h,
                    }
                }
            };

            let id = cmd.image_id.unwrap_or_else(|| store.next_id());

            let image = TerminalImage {
                id,
                data: rgba_data.data,
                width: rgba_data.width,
                height: rgba_data.height,
            };

            let should_place = cmd.action == KittyAction::TransmitAndDisplay;
            let img_w = image.width;
            let img_h = image.height;
            store.store_image(image);

            if should_place {
                store.add_placement(ImagePlacement {
                    image_id: id,
                    col: cursor_col,
                    row: cursor_row as isize,
                    cell_cols: cmd.cell_cols.unwrap_or(0),
                    cell_rows: cmd.cell_rows.unwrap_or(0),
                    src_width: img_w,
                    src_height: img_h,
                });
            }

            log::debug!(
                "kitty graphics: stored image id={id} {}x{} ({})",
                img_w,
                img_h,
                if should_place {
                    "placed"
                } else {
                    "transmit only"
                }
            );
        }
        KittyAction::Place => {
            let id = match cmd.image_id {
                Some(id) => id,
                None => {
                    log::trace!("kitty graphics: place requires i=");
                    return;
                }
            };
            let image = match store.get_image(id) {
                Some(img) => img,
                None => {
                    log::trace!("kitty graphics: image id={id} not found");
                    return;
                }
            };
            let img_w = image.width;
            let img_h = image.height;
            store.add_placement(ImagePlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row as isize,
                cell_cols: cmd.cell_cols.unwrap_or(0),
                cell_rows: cmd.cell_rows.unwrap_or(0),
                src_width: img_w,
                src_height: img_h,
            });
            log::debug!("kitty graphics: placed image id={id} at ({cursor_col}, {cursor_row})");
        }
        KittyAction::Delete => {
            match cmd.delete_target {
                Some('a') | None => {
                    // Delete all images.
                    if let Some(id) = cmd.image_id {
                        store.delete_image(id);
                        log::debug!("kitty graphics: deleted image id={id}");
                    } else {
                        store.delete_all();
                        log::debug!("kitty graphics: deleted all images");
                    }
                }
                Some('i') => {
                    if let Some(id) = cmd.image_id {
                        store.delete_image(id);
                        log::debug!("kitty graphics: deleted image id={id}");
                    }
                }
                Some(target) => {
                    log::trace!("kitty graphics: unsupported delete target '{target}'");
                }
            }
        }
        KittyAction::Query => {
            log::trace!("kitty graphics: query (not sending response yet)");
        }
    }
}

// ---------------------------------------------------------------------------
// APC sequence extraction (pre-processor for vte)
// ---------------------------------------------------------------------------

/// State for extracting APC sequences from a byte stream before vte processing.
///
/// The vte crate (0.13) does not dispatch APC content — it enters
/// `SosPmApcString` and ignores all bytes until ST. We pre-scan the stream
/// and extract APC payloads (used by Kitty Graphics Protocol) before passing
/// the remaining bytes to vte.
#[derive(Debug)]
pub struct ApcExtractor {
    state: ApcState,
    buffer: Vec<u8>,
}

#[derive(Debug, PartialEq)]
enum ApcState {
    /// Normal pass-through.
    Ground,
    /// Saw ESC (0x1B), waiting for `_` (0x5F) to start APC.
    Escape,
    /// Inside APC body, accumulating until ST.
    InApc,
    /// Inside APC body, saw ESC — waiting for `\` to end (ESC \).
    InApcEscape,
}

/// Result of processing bytes through the APC extractor.
pub struct ApcExtractResult {
    /// Bytes that should be fed to the vte parser (APC sequences stripped out).
    pub passthrough: Vec<u8>,
    /// Complete APC payloads that were extracted.
    pub apc_payloads: Vec<Vec<u8>>,
}

impl ApcExtractor {
    pub fn new() -> Self {
        Self {
            state: ApcState::Ground,
            buffer: Vec::new(),
        }
    }

    /// Process a chunk of bytes, extracting any APC sequences.
    ///
    /// Returns the bytes to pass through to vte and any complete APC payloads.
    pub fn process(&mut self, data: &[u8]) -> ApcExtractResult {
        let mut passthrough = Vec::with_capacity(data.len());
        let mut apc_payloads = Vec::new();

        for &byte in data {
            match self.state {
                ApcState::Ground => {
                    if byte == 0x1B {
                        self.state = ApcState::Escape;
                    } else {
                        passthrough.push(byte);
                    }
                }
                ApcState::Escape => {
                    if byte == b'_' {
                        // Start of APC.
                        self.state = ApcState::InApc;
                        self.buffer.clear();
                    } else {
                        // Not APC — pass the ESC and this byte through.
                        passthrough.push(0x1B);
                        passthrough.push(byte);
                        self.state = ApcState::Ground;
                    }
                }
                ApcState::InApc => {
                    if byte == 0x1B {
                        self.state = ApcState::InApcEscape;
                    } else if byte == 0x9C {
                        // ST (single byte C1 form).
                        apc_payloads.push(std::mem::take(&mut self.buffer));
                        self.state = ApcState::Ground;
                    } else {
                        self.buffer.push(byte);
                    }
                }
                ApcState::InApcEscape => {
                    if byte == b'\\' {
                        // ESC \ = ST — end of APC.
                        apc_payloads.push(std::mem::take(&mut self.buffer));
                        self.state = ApcState::Ground;
                    } else {
                        // Not ST — the ESC was part of the APC body.
                        self.buffer.push(0x1B);
                        self.buffer.push(byte);
                        self.state = ApcState::InApc;
                    }
                }
            }
        }

        ApcExtractResult {
            passthrough,
            apc_payloads,
        }
    }

    /// Reset the extractor state (e.g., on terminal reset).
    pub fn reset(&mut self) {
        self.state = ApcState::Ground;
        self.buffer.clear();
    }
}

impl Default for ApcExtractor {
    fn default() -> Self {
        Self::new()
    }
}
