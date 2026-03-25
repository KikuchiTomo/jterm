//! iTerm2 Inline Images (OSC 1337) implementation.

use base64::Engine as _;

use super::{decode_jpeg, decode_png, is_jpeg, is_png, ImagePlacement, ImageStore, TerminalImage};

/// Parsed iTerm2 image metadata from the initial `MultipartFile=` or `File=` header.
#[derive(Debug, Clone, Default)]
struct Iterm2Params {
    inline: bool,
    width: Option<String>,
    height: Option<String>,
    preserve_aspect: bool,
}

/// Accumulator for iTerm2 multipart image transfer.
///
/// The multipart protocol works as follows:
///   1. `OSC 1337 ; MultipartFile=<params> ST` — begin transfer (metadata, no pixel data)
///   2. `OSC 1337 ; FilePart=<base64chunk> ST` — one or more data chunks
///   3. `OSC 1337 ; FileEnd ST`                — finalize: decode & place the image
#[derive(Debug, Default)]
pub struct Iterm2Accumulator {
    /// Parsed parameters from the initial MultipartFile header.
    params: Option<Iterm2Params>,
    /// Accumulated base64-encoded data across FilePart chunks.
    base64_data: String,
}

impl Iterm2Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new multipart transfer. Parses the `MultipartFile=<params>` payload.
    pub fn begin(&mut self, payload: &str) {
        let rest = match payload.strip_prefix("MultipartFile=") {
            Some(r) => r,
            None => {
                log::trace!("iterm2 multipart: missing 'MultipartFile=' prefix");
                return;
            }
        };

        let params = parse_iterm2_params(rest);
        self.params = Some(params);
        self.base64_data.clear();
        log::debug!("iterm2 multipart: begin");
    }

    /// Accumulate a `FilePart=<base64chunk>` chunk.
    pub fn add_part(&mut self, payload: &str) {
        let chunk = match payload.strip_prefix("FilePart=") {
            Some(c) => c,
            None => {
                log::trace!("iterm2 multipart: missing 'FilePart=' prefix");
                return;
            }
        };
        self.base64_data.push_str(chunk);
    }

    /// Finalize on `FileEnd`: decode the accumulated data and place the image.
    /// Returns `true` if an image was successfully placed.
    pub fn finish(&mut self, store: &mut ImageStore, cursor_col: usize, cursor_row: usize) -> bool {
        let params = match self.params.take() {
            Some(p) => p,
            None => {
                log::trace!("iterm2 multipart: FileEnd without prior MultipartFile");
                self.base64_data.clear();
                return false;
            }
        };

        if !params.inline {
            log::trace!("iterm2 multipart: inline=0, not displaying");
            self.base64_data.clear();
            return false;
        }

        let b64 = std::mem::take(&mut self.base64_data);
        let raw_bytes = match base64::engine::general_purpose::STANDARD.decode(&b64) {
            Ok(b) => b,
            Err(e) => {
                log::trace!("iterm2 multipart: base64 decode error: {e}");
                return false;
            }
        };

        let decoded = if is_png(&raw_bytes) {
            decode_png(&raw_bytes)
        } else if is_jpeg(&raw_bytes) {
            decode_jpeg(&raw_bytes)
        } else {
            log::trace!("iterm2 multipart: unsupported format (not PNG or JPEG)");
            None
        };

        let decoded = match decoded {
            Some(d) => d,
            None => {
                log::trace!("iterm2 multipart: failed to decode image data");
                return false;
            }
        };

        let id = store.next_id();
        let img_w = decoded.width;
        let img_h = decoded.height;

        let cell_cols = params
            .width
            .and_then(|w| parse_iterm2_dimension(&w))
            .unwrap_or(0);
        let cell_rows = params
            .height
            .and_then(|h| parse_iterm2_dimension(&h))
            .unwrap_or(0);

        store.store_image(TerminalImage {
            id,
            data: decoded.data,
            width: img_w,
            height: img_h,
        });

        store.add_placement(ImagePlacement {
            image_id: id,
            col: cursor_col,
            row: cursor_row as isize,
            cell_cols,
            cell_rows,
            src_width: img_w,
            src_height: img_h,
        });

        log::debug!("iterm2 multipart: stored and placed id={id} {img_w}x{img_h}");
        true
    }

    /// Whether a multipart transfer is currently in progress.
    pub fn is_active(&self) -> bool {
        self.params.is_some()
    }
}

/// Parse iTerm2 key=value parameters from a params string (without the `File=` or
/// `MultipartFile=` prefix, and without the `:base64data` suffix if present).
fn parse_iterm2_params(params_str: &str) -> Iterm2Params {
    let mut result = Iterm2Params {
        inline: false,
        width: None,
        height: None,
        preserve_aspect: true,
    };

    for kv in params_str.split(';') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let value = parts.next().unwrap_or("");
        match key {
            "inline" => result.inline = value == "1",
            "width" => result.width = Some(value.to_string()),
            "height" => result.height = Some(value.to_string()),
            "preserveAspectRatio" => result.preserve_aspect = value != "0",
            _ => {}
        }
    }

    result
}

/// Parse an iTerm2 inline image OSC 1337 payload.
///
/// Format: `File=<params>:<base64data>`
/// Params are semicolon-separated `key=value` pairs.
pub fn parse_iterm2_image(
    payload: &str,
    store: &mut ImageStore,
    cursor_col: usize,
    cursor_row: usize,
) {
    // The payload after "File=" is: params:base64data
    let rest = match payload.strip_prefix("File=") {
        Some(r) => r,
        None => {
            log::trace!("iterm2 image: missing 'File=' prefix");
            return;
        }
    };

    let (params_str, b64_data) = match rest.rfind(':') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => {
            log::trace!("iterm2 image: missing ':' separator");
            return;
        }
    };

    // Parse params.
    let mut inline = false;
    let mut _name = String::new();
    let mut _size: Option<usize> = None;
    let mut width_param: Option<String> = None;
    let mut height_param: Option<String> = None;
    let mut _preserve_aspect = true;

    for kv in params_str.split(';') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let value = parts.next().unwrap_or("");
        match key {
            "inline" => inline = value == "1",
            "name" => {
                _name = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
            }
            "size" => _size = value.parse().ok(),
            "width" => width_param = Some(value.to_string()),
            "height" => height_param = Some(value.to_string()),
            "preserveAspectRatio" => _preserve_aspect = value != "0",
            _ => {}
        }
    }

    if !inline {
        log::trace!("iterm2 image: inline=0, not displaying");
        return;
    }

    // Decode base64 data.
    let raw_bytes = match base64::engine::general_purpose::STANDARD.decode(b64_data) {
        Ok(b) => b,
        Err(e) => {
            log::trace!("iterm2 image: base64 decode error: {e}");
            return;
        }
    };

    // Detect format and decode to RGBA.
    let decoded = if is_png(&raw_bytes) {
        decode_png(&raw_bytes)
    } else if is_jpeg(&raw_bytes) {
        decode_jpeg(&raw_bytes)
    } else {
        log::trace!("iterm2 image: unsupported format (not PNG or JPEG)");
        None
    };

    let decoded = match decoded {
        Some(d) => d,
        None => {
            log::trace!("iterm2 image: failed to decode image data");
            return;
        }
    };

    let id = store.next_id();
    let img_w = decoded.width;
    let img_h = decoded.height;

    // Parse width/height params for cell sizing.
    let cell_cols = width_param
        .and_then(|w| parse_iterm2_dimension(&w))
        .unwrap_or(0);
    let cell_rows = height_param
        .and_then(|h| parse_iterm2_dimension(&h))
        .unwrap_or(0);

    store.store_image(TerminalImage {
        id,
        data: decoded.data,
        width: img_w,
        height: img_h,
    });

    store.add_placement(ImagePlacement {
        image_id: id,
        col: cursor_col,
        row: cursor_row as isize,
        cell_cols,
        cell_rows,
        src_width: img_w,
        src_height: img_h,
    });

    log::debug!("iterm2 image: stored and placed id={id} {img_w}x{img_h}");
}

/// Parse an iTerm2 dimension string (e.g., "80px", "10", "auto").
/// Returns cell count or 0 for auto-sizing.
fn parse_iterm2_dimension(s: &str) -> Option<usize> {
    if s == "auto" || s.is_empty() {
        return Some(0);
    }
    if let Some(px) = s.strip_suffix("px") {
        // Pixel dimension — we'd need cell size to convert.
        // Return 0 to let add_placement compute from src dimensions.
        let _px_val: u32 = px.parse().ok()?;
        Some(0)
    } else {
        // Treat as cell count.
        s.parse().ok()
    }
}
