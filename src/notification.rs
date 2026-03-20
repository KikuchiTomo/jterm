//! macOS desktop notifications via Notification Center API.
//!
//! Uses `mac-notification-sys` which wraps `NSUserNotificationCenter` /
//! `UNUserNotificationCenter` natively.

/// Bundle identifier — matches Info.plist in the .app bundle.
fn bundle_id() -> &'static str {
    if cfg!(debug_assertions) {
        "com.termojinal.app.dev"
    } else {
        "com.termojinal.app"
    }
}

/// Initialize the notification system. Call once at startup.
pub fn init() {
    let bid = bundle_id();
    if let Err(e) = mac_notification_sys::set_application(bid) {
        log::warn!("notification init failed for {bid}: {e}");
    }
}

/// Send a macOS desktop notification.
///
/// `title` — notification title (e.g. "termojinal").
/// `body`  — notification body text.
/// `sound` — if `true`, plays the default notification sound.
pub fn send_notification(title: &str, body: &str, sound: bool) {
    let mut notif = mac_notification_sys::Notification::new();
    if sound {
        notif.sound("default");
    }

    if let Err(e) = mac_notification_sys::send_notification(title, None, body, Some(&notif)) {
        log::warn!("notification failed: {e}");
    }
}
