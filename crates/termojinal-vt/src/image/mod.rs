//! Image display protocol support for termojinal.
//!
//! Implements three terminal image protocols:
//! - **Kitty Graphics Protocol** — APC-based, supports direct transmission (PNG/RGB/RGBA),
//!   chunked transfer, placement, and deletion.
//! - **iTerm2 Inline Images** — OSC 1337-based, supports PNG/JPEG with base64 encoding.
//! - **Sixel Graphics** — DCS-based legacy format encoding 6 vertical pixels per character.
//!
//! Decoded images are stored in an `ImageStore` and referenced by the renderer for
//! GPU texture upload and display.

pub mod iterm2;
pub mod kitty;
pub mod sixel;
mod tests;

use std::collections::HashMap;

pub use kitty::{
    parse_kitty_header, process_kitty_command, ApcExtractResult, ApcExtractor, KittyAccumulator,
    KittyAction, KittyCommand, KittyFormat,
};
pub use iterm2::{parse_iterm2_image, Iterm2Accumulator};
pub use sixel::process_sixel;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A decoded terminal image (RGBA pixel data).
#[derive(Debug, Clone)]
pub struct TerminalImage {
    /// Unique image ID (assigned by protocol or auto-generated).
    pub id: u32,
    /// RGBA pixel data (4 bytes per pixel).
    pub data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Where an image is placed on the terminal grid.
#[derive(Debug, Clone)]
pub struct ImagePlacement {
    /// Image ID this placement refers to.
    pub image_id: u32,
    /// Column position (0-based).
    pub col: usize,
    /// Row position (signed: negative means partially scrolled off the top).
    pub row: isize,
    /// How many cell columns the image spans.
    pub cell_cols: usize,
    /// How many cell rows the image spans.
    pub cell_rows: usize,
    /// Source image width in pixels (for scaling).
    pub src_width: u32,
    /// Source image height in pixels (for scaling).
    pub src_height: u32,
}

/// Central store for decoded images and their placements.
pub struct ImageStore {
    images: HashMap<u32, TerminalImage>,
    next_id: u32,
    /// Active placements on the current screen.
    placements: Vec<ImagePlacement>,
    /// Cell dimensions (set by the application; used to compute cell_cols/cell_rows).
    cell_width_px: u32,
    cell_height_px: u32,
    /// Dirty flag: set when images/placements change and the renderer needs to update.
    dirty: bool,
}

impl ImageStore {
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            next_id: 1,
            placements: Vec::new(),
            cell_width_px: 8,
            cell_height_px: 16,
            dirty: false,
        }
    }

    /// Set the cell dimensions (in pixels) used for computing placement cell spans.
    pub fn set_cell_size(&mut self, width: u32, height: u32) {
        if width > 0 {
            self.cell_width_px = width;
        }
        if height > 0 {
            self.cell_height_px = height;
        }
    }

    /// Allocate the next auto-generated image ID.
    pub fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Store a decoded image. If an image with the same ID already exists, it is replaced.
    pub fn store_image(&mut self, image: TerminalImage) {
        self.images.insert(image.id, image);
        self.dirty = true;
    }

    /// Get an image by ID.
    pub fn get_image(&self, id: u32) -> Option<&TerminalImage> {
        self.images.get(&id)
    }

    /// Add a placement. Computes cell_cols/cell_rows from pixel dimensions if not provided.
    pub fn add_placement(&mut self, mut placement: ImagePlacement) {
        if placement.cell_cols == 0 && placement.src_width > 0 {
            placement.cell_cols = (placement.src_width as usize + self.cell_width_px as usize - 1)
                / self.cell_width_px as usize;
        }
        if placement.cell_rows == 0 && placement.src_height > 0 {
            placement.cell_rows = (placement.src_height as usize + self.cell_height_px as usize
                - 1)
                / self.cell_height_px as usize;
        }
        // Ensure at least 1x1 cell.
        placement.cell_cols = placement.cell_cols.max(1);
        placement.cell_rows = placement.cell_rows.max(1);
        self.placements.push(placement);
        self.dirty = true;
    }

    /// Delete an image and all its placements by ID.
    pub fn delete_image(&mut self, id: u32) {
        self.images.remove(&id);
        self.placements.retain(|p| p.image_id != id);
        self.dirty = true;
    }

    /// Delete all images and placements.
    pub fn delete_all(&mut self) {
        self.images.clear();
        self.placements.clear();
        self.dirty = true;
    }

    /// Get all current placements.
    pub fn placements(&self) -> &[ImagePlacement] {
        &self.placements
    }

    /// Get all stored images.
    pub fn images(&self) -> &HashMap<u32, TerminalImage> {
        &self.images
    }

    /// Check and clear the dirty flag.
    pub fn take_dirty(&mut self) -> bool {
        let was_dirty = self.dirty;
        self.dirty = false;
        was_dirty
    }

    /// Check if there are any placements.
    pub fn has_placements(&self) -> bool {
        !self.placements.is_empty()
    }

    /// Adjust all image placements when the terminal scrolls up by `lines` rows.
    ///
    /// Shifts all placement rows up and removes images that have fully scrolled
    /// off the top of the screen.  Also garbage-collects images that no longer
    /// have any placements.
    pub fn scroll_up(&mut self, lines: usize) {
        if lines == 0 || self.placements.is_empty() {
            return;
        }
        let lines = lines as isize;
        // Shift all placement rows up (row can go negative = scrolled off top).
        for p in &mut self.placements {
            p.row -= lines;
        }
        // Remove placements whose bottom edge is above the screen top.
        self.placements.retain(|p| p.row + p.cell_rows as isize > 0);
        // Garbage-collect images that no longer have any placements.
        let placed_ids: Vec<u32> = self.placements.iter().map(|p| p.image_id).collect();
        self.images.retain(|id, _| placed_ids.contains(id));
        self.dirty = true;
    }

    /// Limit image placement size to fit within the visible terminal grid.
    ///
    /// Called after computing cell_cols/cell_rows so images don't extend
    /// beyond the terminal dimensions.
    pub fn cap_placement_size(&mut self, max_cols: usize, max_rows: usize) {
        if let Some(p) = self.placements.last_mut() {
            if p.cell_cols > max_cols {
                p.cell_cols = max_cols;
            }
            if p.cell_rows > max_rows {
                p.cell_rows = max_rows;
            }
        }
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Image format detection and decoding
// ---------------------------------------------------------------------------

/// Intermediate decoded image before storing.
pub struct DecodedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Check if data starts with PNG magic bytes.
pub(crate) fn is_png(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

/// Check if data starts with JPEG magic bytes.
pub(crate) fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

/// Decode PNG data to RGBA pixels using the `image` crate.
pub(crate) fn decode_png(data: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        data: rgba.into_raw(),
    })
}

/// Decode JPEG data to RGBA pixels using the `image` crate.
pub(crate) fn decode_jpeg(data: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        data: rgba.into_raw(),
    })
}
