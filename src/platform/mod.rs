//! Platform-specific code, organized by target OS.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;

// ---------------------------------------------------------------------------
// Fallback implementations for non-macOS platforms
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub(crate) fn copy_to_clipboard_with_rtf(plain_text: &str, _rtf_text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(plain_text);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn set_dock_icon() {}
