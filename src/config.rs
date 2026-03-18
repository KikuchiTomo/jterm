//! Configuration loading for jterm.
//!
//! Loads settings from `~/.config/jterm/config.toml` with sane defaults.

use serde::Deserialize;

/// Top-level jterm configuration.
#[derive(Debug, Deserialize)]
pub struct JtermConfig {
    #[serde(default)]
    pub tab_bar: TabBarConfig,
}

impl Default for JtermConfig {
    fn default() -> Self {
        Self {
            tab_bar: TabBarConfig::default(),
        }
    }
}

/// Tab bar configuration section (`[tab_bar]`).
#[derive(Debug, Deserialize)]
pub struct TabBarConfig {
    /// Format string for tab title.
    ///
    /// Available variables: `{title}`, `{cwd}`, `{cwd_base}`, `{pid}`, `{index}`.
    /// Use `|` as a fallback separator — first non-empty value wins.
    /// Example: `"{title|cwd_base|Tab {index}}"`.
    #[serde(default = "default_tab_format")]
    pub format: String,

    /// Show the tab bar even when a workspace has a single tab.
    #[serde(default)]
    pub always_show: bool,

    /// Tab bar position: `"top"` or `"bottom"`.
    #[allow(dead_code)]
    #[serde(default = "default_tab_position")]
    pub position: String,

    /// Maximum tab width in pixels.
    #[serde(default = "default_max_width")]
    pub max_width: f32,
}

fn default_tab_format() -> String {
    "{title|cwd_base|Tab {index}}".into()
}
fn default_tab_position() -> String {
    "top".into()
}
fn default_max_width() -> f32 {
    200.0
}

impl Default for TabBarConfig {
    fn default() -> Self {
        Self {
            format: default_tab_format(),
            always_show: false,
            position: default_tab_position(),
            max_width: default_max_width(),
        }
    }
}

/// Load the jterm config from `~/.config/jterm/config.toml`.
///
/// Returns the default configuration if the file does not exist or cannot be parsed.
pub fn load_config() -> JtermConfig {
    let path = dirs::config_dir()
        .unwrap_or_default()
        .join("jterm")
        .join("config.toml");
    if let Ok(content) = std::fs::read_to_string(&path) {
        toml::from_str(&content).unwrap_or_default()
    } else {
        JtermConfig::default()
    }
}

/// Format a tab title using the user's format string with fallback chains.
///
/// The format string supports `|`-separated fallback chains within `{}`.
/// For example, `"{title|cwd_base|Tab {index}}"` tries `title` first,
/// then `cwd_base`, then the literal `"Tab {index}"` (with `{index}` expanded).
pub fn format_tab_title(format: &str, title: &str, cwd: &str, index: usize) -> String {
    // Check if the format is a single `{...}` (common case).
    let trimmed = format.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.matches('{').count() == 1 {
        // Simple fallback chain: {a|b|c}
        let inner = &trimmed[1..trimmed.len() - 1];
        let alternatives: Vec<&str> = inner.split('|').collect();
        for alt in &alternatives {
            let alt = alt.trim();
            let resolved = resolve_variable(alt, title, cwd, index);
            if !resolved.is_empty() {
                return resolved;
            }
        }
        // All alternatives empty — return the last one literally expanded.
        return format!("Tab {}", index);
    }

    // General case: expand variables in the format string.
    expand_variables(format, title, cwd, index)
}

/// Resolve a single variable name to its value.
fn resolve_variable(var: &str, title: &str, cwd: &str, index: usize) -> String {
    match var {
        "title" => title.to_string(),
        "cwd" => cwd.to_string(),
        "cwd_base" => std::path::Path::new(cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        "index" => index.to_string(),
        other => {
            // Could be a literal with embedded variables, e.g. "Tab {index}".
            expand_variables(other, title, cwd, index)
        }
    }
}

/// Expand `{variable}` placeholders in a string.
fn expand_variables(s: &str, title: &str, cwd: &str, index: usize) -> String {
    let cwd_base = std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("~");

    s.replace("{title}", title)
        .replace("{cwd}", cwd)
        .replace("{cwd_base}", cwd_base)
        .replace("{index}", &index.to_string())
}
