use crate::*;
use config::{color_or, parse_hex_color};
use status::{expand_status_variables, segment_is_empty, StatusContext};

/// Build the `StatusContext` for the current frame by collecting variable values.
pub(crate) fn build_status_context(state: &mut AppState) -> StatusContext {
    let cache = &mut state.status_cache;
    let (time, date) = cache.time_date();
    let time = time.to_string();
    let date = date.to_string();

    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let tab_idx = ws.active_tab;
    let tab = &ws.tabs[tab_idx];
    let focused_id = tab.layout.focused();

    // Extract user, host from focused pane's OSC 7 URI (file://user@host/path).
    // Fallback 1: detect SSH from child process tree (parse `ssh user@host` args).
    // Fallback 2: cached global $USER / gethostname.
    let focused_pane = tab.panes.get(&focused_id);
    let (user, host) = {
        let mut u = cache.user.clone();
        let mut h = cache.host.clone();
        let mut found = false;

        // Try OSC 7 URI first.
        if let Some(pane) = focused_pane {
            let uri = &pane.terminal.osc.cwd_uri;
            if let Some(rest) = uri.strip_prefix("file://") {
                if let Some(slash_idx) = rest.find('/') {
                    let authority = &rest[..slash_idx];
                    if !authority.is_empty() {
                        if let Some(at_idx) = authority.find('@') {
                            let pu = &authority[..at_idx];
                            let ph = &authority[at_idx + 1..];
                            if !pu.is_empty() {
                                u = pu.to_string();
                            }
                            if !ph.is_empty() {
                                h = ph.to_string();
                            }
                            found = true;
                        } else {
                            h = authority.to_string();
                            found = true;
                        }
                    }
                }
            }

            // If OSC 7 didn't provide host info, use cached SSH detection
            // (populated during git cache refresh, not every frame).
            if !found {
                let gc = &state.pane_git_cache;
                if !gc.ssh_host.is_empty() {
                    h = gc.ssh_host.clone();
                    if !gc.ssh_user.is_empty() {
                        u = gc.ssh_user.clone();
                    }
                }
            }
        }
        (u, h)
    };

    // Shell from focused pane's PTY shell command (basename).
    let shell = focused_pane
        .map(|p| {
            std::path::Path::new(&p.shell)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_else(|| cache.shell.clone());

    // CWD: prefer OSC 7, otherwise use cached value from lsof (updated every refresh).
    // Send current pane info to the background status collector (non-blocking).
    let osc_cwd = focused_pane
        .map(|p| p.terminal.osc.cwd.clone())
        .unwrap_or_default();
    let pty_pid = focused_pane.map(|p| p.shell_pid).unwrap_or(0);
    state.status_collector.update_request(pty_pid, &osc_cwd);

    // Read latest snapshot from background thread (non-blocking).
    let snap = state.status_collector.get();
    state.pane_git_cache.update_from_snapshot(&snap);

    let gc = &state.pane_git_cache;
    let cwd = if !osc_cwd.is_empty() {
        osc_cwd
    } else {
        gc.cwd.clone()
    };
    let cwd_short = if let Ok(home) = std::env::var("HOME") {
        if cwd.starts_with(&home) {
            format!("~{}", &cwd[home.len()..])
        } else {
            cwd.clone()
        }
    } else {
        cwd.clone()
    };
    let git_branch = gc.git_branch.clone();
    let git_worktree = gc.git_worktree.clone();
    let git_stash = if gc.git_stash > 0 {
        format!("{}", gc.git_stash)
    } else {
        String::new()
    };
    let git_ahead = format!("{}", gc.git_ahead);
    let git_behind = format!("{}", gc.git_behind);
    let git_dirty = format!("{}", gc.git_dirty);
    let git_untracked = format!("{}", gc.git_untracked);
    let git_status = {
        let mut parts = Vec::new();
        if gc.git_ahead > 0 {
            parts.push(format!("\u{21E1}{}", gc.git_ahead));
        }
        if gc.git_behind > 0 {
            parts.push(format!("\u{21E3}{}", gc.git_behind));
        }
        if gc.git_dirty > 0 {
            parts.push(format!("!{}", gc.git_dirty));
        }
        if gc.git_untracked > 0 {
            parts.push(format!("?{}", gc.git_untracked));
        }
        parts.join(" ")
    };

    // Ports from WorkspaceInfo.
    let info = state.workspace_infos.get(ws_idx);
    let ports = info
        .map(|i| {
            i.ports
                .iter()
                .map(|p| format!(":{p}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    // PID of the focused pane's PTY.
    let pid = focused_pane
        .map(|p| p.shell_pid.to_string())
        .unwrap_or_default();

    // Pane size (cols x rows) of the focused pane.
    let pane_size = focused_pane
        .map(|p| {
            let g = p.terminal.grid();
            format!("{}x{}", g.cols(), g.rows())
        })
        .unwrap_or_default();

    let font_size = format!("{}", state.font_size as u32);

    let workspace = ws.name.clone();
    let workspace_index = format!("{}", ws_idx + 1);
    let tab_name = tab.display_title.clone();
    let tab_index = format!("{}", tab_idx + 1);

    let git_remote = gc.git_remote.clone();

    StatusContext {
        user,
        host,
        cwd,
        cwd_short,
        git_branch,
        git_status,
        git_remote,
        git_worktree,
        git_stash,
        git_ahead,
        git_behind,
        git_dirty,
        git_untracked,
        ports,
        shell,
        pid,
        pane_size,
        font_size,
        workspace,
        workspace_index,
        tab: tab_name,
        tab_index,
        time,
        date,
    }
}

/// Calculate the display width of a string in cell units (accounting for wide chars).
///
/// When `cjk` is true, Unicode East Asian Ambiguous width characters are treated
/// as 2-cell wide. This should match the renderer's and terminal's width calculation.
pub(crate) fn str_display_width(s: &str, cjk: bool) -> usize {
    s.chars().map(|c| termojinal_vt::char_width(c, cjk)).sum()
}

/// Render the bottom status bar.
pub(crate) fn render_status_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32, phys_h: f32) {
    let cfg = state.config.status_bar.clone();
    if !cfg.enabled {
        return;
    }

    let ctx = build_status_context(state);

    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    // Bar height: at least cell_h + padding.
    let bar_h = effective_status_bar_height(state);
    let bar_x = sidebar_w.floor();
    let bar_w = (phys_w - bar_x).floor();
    let bar_y = (phys_h - bar_h).floor();

    // Draw full status bar background.
    let status_bg = parse_hex_color(&cfg.background).unwrap_or([0.1, 0.1, 0.14, 1.0]);
    let bar_yi = bar_y as u32;
    let bar_hi = bar_h as u32;
    state
        .renderer
        .submit_separator(view, bar_x as u32, bar_yi, bar_w as u32, bar_hi, status_bg);

    // Draw top border if enabled.
    if cfg.top_border {
        let border_color = color_or(
            &cfg.top_border_color,
            [
                (status_bg[0] + 0.08).min(1.0),
                (status_bg[1] + 0.08).min(1.0),
                (status_bg[2] + 0.08).min(1.0),
                1.0,
            ],
        );
        state
            .renderer
            .submit_separator(view, bar_x as u32, bar_yi, bar_w as u32, 1, border_color);
    }

    // Optically center text within the bar.
    let descent = state.renderer.cell_size().descent.abs();
    let optical_offset = (descent * 0.4).round();
    let text_y = (bar_y + (bar_h - cell_h) / 2.0 + optical_offset).floor();

    // Segment horizontal padding (each side).
    let seg_pad = if cfg.padding_x > 0.0 {
        cfg.padding_x
    } else {
        cell_w
    };

    // --- Expand all segments and compute widths ---
    let cjk = state.renderer.cjk_width;
    let expand_segs =
        |segs: &[config::StatusSegment]| -> Vec<(String, [f32; 4], [f32; 4], f32, f32)> {
            segs.iter()
                .filter_map(|seg| {
                    let text = expand_status_variables(&seg.content, &ctx);
                    if segment_is_empty(&text) {
                        return None;
                    }
                    let fg = parse_hex_color(&seg.fg).unwrap_or([0.8, 0.8, 0.8, 1.0]);
                    let bg = parse_hex_color(&seg.bg).unwrap_or(status_bg);
                    let text_w = (str_display_width(&text, cjk) as f32 * cell_w).ceil();
                    let seg_w = text_w + seg_pad * 2.0;
                    Some((text, fg, bg, seg_w, text_w))
                })
                .collect()
        };

    let left_segs = expand_segs(&cfg.left);
    let right_segs = expand_segs(&cfg.right);

    // --- Render segments ---
    let text_yi = text_y as u32;

    let render_seg = |state: &mut AppState,
                      xi: u32,
                      text: &str,
                      fg: [f32; 4],
                      bg: [f32; 4],
                      seg_w: f32,
                      text_w: f32| {
        let wi = seg_w as u32;
        state
            .renderer
            .submit_separator(view, xi, bar_yi, wi, bar_hi, bg);
        let text_x = xi as f32 + ((seg_w - text_w) / 2.0).floor();
        state.renderer.render_text_clipped(
            view,
            text,
            text_x,
            text_yi as f32,
            fg,
            bg,
            Some((xi, bar_yi, wi, bar_hi)),
        );
    };

    let mut xi = bar_x as u32;
    for (text, fg, bg, seg_w, text_w) in &left_segs {
        render_seg(state, xi, text, *fg, *bg, *seg_w, *text_w);
        xi += *seg_w as u32;
    }

    let total_right: u32 = right_segs.iter().map(|(_, _, _, sw, _)| *sw as u32).sum();
    let mut xi = (bar_x as u32 + bar_w as u32).saturating_sub(total_right);
    for (text, fg, bg, seg_w, text_w) in &right_segs {
        render_seg(state, xi, text, *fg, *bg, *seg_w, *text_w);
        xi += *seg_w as u32;
    }
}
