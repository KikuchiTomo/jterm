//! Global hotkey monitoring via macOS CGEventTap.
//!
//! A `ListenOnly` CGEventTap requires the **Input Monitoring** permission
//! (macOS 10.15+).  We check via `CGPreflightListenEventAccess()` and, if
//! needed, prompt via `CGRequestListenEventAccess()`.
//!
//! Accessibility permission (`AXIsProcessTrustedWithOptions`) is checked as a
//! secondary requirement -- some macOS versions also gate event taps behind it.
//!
//! If both checks pass but CGEventTapCreate still returns NULL, we gracefully
//! degrade by logging a warning and continuing without hotkey support.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

/// Events that can be triggered by global hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Cmd+Shift+P -- open the Command Palette.
    CommandPalette,
    /// Cmd+Shift+A -- open the Allow Flow panel.
    AllowFlowPanel,
    /// Ctrl+` -- toggle the Quick Terminal visor window.
    QuickTerminal,
}

/// Error type for hotkey operations.
#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("input monitoring permission denied -- CGEventTap cannot be created")]
    InputMonitoringDenied,
    #[error("failed to create run loop source from event tap")]
    RunLoopSourceFailed,
    #[error("failed to start hotkey monitor: {0}")]
    StartFailed(String),
}

/// Handle to the global hotkey monitor. Dropping this stops the event tap.
pub struct GlobalHotkey {
    running: Arc<AtomicBool>,
    _thread: thread::JoinHandle<()>,
}

impl GlobalHotkey {
    /// Start monitoring for global hotkeys.
    ///
    /// The `callback` is invoked on a dedicated background thread whenever a
    /// registered hotkey combination is pressed.
    ///
    /// Returns `Ok(GlobalHotkey)` on success, or `Err` if the event tap could
    /// not be created (typically due to missing Input Monitoring permission).
    pub fn start(
        callback: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // We try creating the event tap on the dedicated thread and report
        // success/failure back via a oneshot channel.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), HotkeyError>>(1);

        let thread = thread::Builder::new()
            .name("termojinal-hotkey".into())
            .spawn(move || {
                platform::run_event_tap(callback, running_clone, tx);
            })
            .map_err(|e| HotkeyError::StartFailed(e.to_string()))?;

        // Wait for the background thread to report whether the tap was created.
        match rx.recv() {
            Ok(Ok(())) => Ok(GlobalHotkey {
                running,
                _thread: thread,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(HotkeyError::StartFailed(
                "hotkey thread exited prematurely".into(),
            )),
        }
    }

    /// Stop monitoring for global hotkeys.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // The CFRunLoop will exit on the next iteration of the polling loop.
    }
}

impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// macOS virtual keycode for 'P'.
const KEYCODE_P: i64 = 35;
/// macOS virtual keycode for 'A'.
const KEYCODE_A: i64 = 0;
/// macOS virtual keycode for '`' (backtick / grave accent, US layout).
const KEYCODE_BACKTICK: i64 = 50;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, EventField,
    };

    /// Check whether Cmd and Shift are both held (ignoring other modifiers).
    fn is_cmd_shift(flags: CGEventFlags) -> bool {
        flags.contains(CGEventFlags::CGEventFlagCommand)
            && flags.contains(CGEventFlags::CGEventFlagShift)
    }

    /// Check whether Control is held (ignoring other modifiers).
    fn is_ctrl(flags: CGEventFlags) -> bool {
        flags.contains(CGEventFlags::CGEventFlagControl)
    }

    // ---- Core Graphics Input Monitoring APIs (macOS 10.15+) ----

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        /// Returns `true` if the calling process has Input Monitoring permission.
        /// Does **not** prompt the user.
        fn CGPreflightListenEventAccess() -> bool;

        /// Prompts the user to grant Input Monitoring permission (shows the
        /// system dialog).  Returns `true` if permission was already granted.
        fn CGRequestListenEventAccess() -> bool;
    }

    // ---- Accessibility helpers (secondary check) ----

    /// Check Accessibility permission without prompting.
    fn is_accessibility_granted() -> bool {
        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::CFString;

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrustedWithOptions(
                options: core_foundation::base::CFTypeRef,
            ) -> bool;
        }

        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::false_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) }
    }

    /// Show the Accessibility permission dialog (call once).
    fn prompt_accessibility() {
        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::CFString;

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrustedWithOptions(
                options: core_foundation::base::CFTypeRef,
            ) -> bool;
        }

        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()); }
    }

    pub(super) fn run_event_tap(
        callback: impl Fn(HotkeyEvent) + Send + 'static,
        running: Arc<AtomicBool>,
        tx: std::sync::mpsc::SyncSender<Result<(), HotkeyError>>,
    ) {
        // 1. Check Input Monitoring permission (required for ListenOnly CGEventTap).
        let input_monitoring = unsafe { CGPreflightListenEventAccess() };
        if !input_monitoring {
            log::info!("Input Monitoring permission not granted — requesting");
            let granted = unsafe { CGRequestListenEventAccess() };
            if !granted {
                log::warn!("Input Monitoring permission was not granted by the user");
            }
        }

        // 2. Secondary: check Accessibility permission (some macOS versions
        //    also require it).  Only prompt if not already granted.
        if !is_accessibility_granted() {
            log::info!("Accessibility permission not granted — showing dialog");
            prompt_accessibility();
        }

        // 3. Try creating the CGEventTap regardless — it will return NULL if
        //    the required permissions are still missing.
        let tap = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![CGEventType::KeyDown],
            move |_proxy, _event_type, event| {
                let flags = event.get_flags();
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

                if is_cmd_shift(flags) {
                    match keycode {
                        KEYCODE_P => {
                            callback(HotkeyEvent::CommandPalette);
                        }
                        KEYCODE_A => {
                            callback(HotkeyEvent::AllowFlowPanel);
                        }
                        _ => {}
                    }
                }

                // Ctrl+` — toggle Quick Terminal.
                if is_ctrl(flags) && keycode == KEYCODE_BACKTICK {
                    callback(HotkeyEvent::QuickTerminal);
                }

                // ListenOnly tap -- always return None (we don't modify events).
                None
            },
        );

        let tap = match tap {
            Ok(tap) => tap,
            Err(()) => {
                // CGEventTapCreate returned NULL -- Input Monitoring permission denied.
                let _ = tx.send(Err(HotkeyError::InputMonitoringDenied));
                return;
            }
        };

        // Create a CFRunLoopSource from the CGEventTap's mach port.
        let loop_source = match tap.mach_port.create_runloop_source(0) {
            Ok(source) => source,
            Err(()) => {
                let _ = tx.send(Err(HotkeyError::RunLoopSourceFailed));
                return;
            }
        };

        let current_loop = CFRunLoop::get_current();
        unsafe {
            current_loop.add_source(&loop_source, kCFRunLoopCommonModes);
        }

        // Enable the event tap.
        tap.enable();

        // Report success to the caller.
        let _ = tx.send(Ok(()));

        // Run the loop, checking periodically if we should stop.
        while running.load(Ordering::SeqCst) {
            unsafe {
                CFRunLoop::run_in_mode(
                    kCFRunLoopCommonModes,
                    std::time::Duration::from_millis(500),
                    false,
                );
            }
        }

        current_loop.stop();
        log::info!("global hotkey monitor stopped");
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) fn run_event_tap(
        _callback: impl Fn(HotkeyEvent) + Send + 'static,
        _running: Arc<AtomicBool>,
        tx: std::sync::mpsc::SyncSender<Result<(), HotkeyError>>,
    ) {
        let _ = tx.send(Err(HotkeyError::StartFailed(
            "global hotkeys are only supported on macOS".into(),
        )));
    }
}
