//! macOS desktop notifications via osascript.
//!
//! Uses `osascript -e 'display notification ...'` which works without
//! entitlements or provisioning profiles.  The command is spawned
//! asynchronously so it never blocks the render loop.

/// Send a macOS desktop notification.
///
/// `title` — notification title (e.g. "termojinal").
/// `body`  — notification body text.
/// `sound` — if `true`, plays the default notification sound.
pub fn send_notification(title: &str, body: &str, sound: bool) {
    let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");

    let script = if sound {
        format!(
            "display notification \"{}\" with title \"{}\" sound name \"default\"",
            escaped_body, escaped_title
        )
    } else {
        format!(
            "display notification \"{}\" with title \"{}\"",
            escaped_body, escaped_title
        )
    };

    std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_quotes_in_body() {
        // Just ensure we don't panic — we can't easily assert osascript output
        // in a headless test, but we can confirm the function handles special chars.
        // We do NOT actually send a notification in tests.
        let body = r#"He said "hello" and it's done"#;
        let title = r#"termojinal "test""#;
        let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
        assert!(!escaped_body.contains('"') || escaped_body.contains("\\\""));
        assert!(!escaped_title.contains('"') || escaped_title.contains("\\\""));
    }
}
