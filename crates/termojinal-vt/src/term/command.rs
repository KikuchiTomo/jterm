//! Command history tracking via OSC 133 shell integration.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use super::Terminal;

/// A structured record of a single shell command and its output region.
///
/// Built from OSC 133 shell integration markers (A/B/C/D).
/// Enables command-level navigation, timeline UI, and session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    /// Monotonically increasing command ID within a session.
    pub id: u64,
    /// The command text entered by the user (extracted from grid between B and C markers).
    pub command_text: String,
    /// Working directory at the time the command was executed (from OSC 7).
    pub cwd: String,
    /// When the command started executing (OSC 133 C received).
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Duration in milliseconds (computed when OSC 133 D is received).
    pub duration_ms: Option<u64>,
    /// Exit code (parsed from OSC 133 D parameter).
    pub exit_code: Option<i32>,
    /// Absolute scrollback line where command output begins.
    pub scrollback_line_start: usize,
    /// Absolute scrollback line where command output ends (set when next prompt starts).
    pub scrollback_line_end: Option<usize>,
    /// Absolute scrollback line of the prompt for this command.
    pub prompt_line: usize,
}

/// Transient state accumulated between OSC 133 markers while a command is in progress.
#[derive(Debug, Clone)]
pub(crate) struct PendingCommand {
    /// Absolute line of the prompt (OSC 133 A).
    pub(crate) prompt_abs_line: usize,
    /// Absolute line where command input starts (OSC 133 B).
    pub(crate) command_start_abs_line: usize,
    /// Column where command input starts (OSC 133 B).
    pub(crate) command_start_col: usize,
    /// Working directory at command start.
    pub(crate) cwd: String,
    /// When the command was executed (OSC 133 C).
    pub(crate) started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Absolute line where output begins (OSC 133 C).
    pub(crate) output_start_abs_line: Option<usize>,
}

impl Terminal {
    /// Compute the current absolute line number for a given screen row.
    pub(crate) fn abs_line(&self, screen_row: usize) -> usize {
        self.total_scrolled_lines + screen_row
    }

    /// Get the full command history.
    pub fn command_history(&self) -> &VecDeque<CommandRecord> {
        &self.command_history
    }

    /// Total scrolled lines (for converting between absolute and relative positions).
    pub fn total_scrolled_lines(&self) -> usize {
        self.total_scrolled_lines
    }

    /// Enable or disable command history tracking.
    pub fn set_command_history_enabled(&mut self, enabled: bool) {
        self.command_history_enabled = enabled;
    }

    /// Set maximum command history size.
    pub fn set_max_command_history(&mut self, max: usize) {
        self.max_command_history = max;
    }

    /// Convert an absolute line number to a scroll_offset.
    fn abs_line_to_scroll_offset(&self, abs_line: usize) -> usize {
        if abs_line >= self.total_scrolled_lines {
            0
        } else {
            let scrollback_idx = self.total_scrolled_lines - 1 - abs_line;
            scrollback_idx + 1
        }
    }

    /// Compute the absolute line at the current viewport top.
    fn viewport_top_abs(&self) -> usize {
        if self.scroll_offset == 0 {
            self.total_scrolled_lines + self.rows
        } else {
            self.total_scrolled_lines.saturating_sub(self.scroll_offset)
        }
    }

    /// Binary-search: find the last command whose prompt_line < target (S2).
    fn find_prev_command_idx(&self, target_abs: usize) -> Option<usize> {
        if self.command_history.is_empty() {
            return None;
        }
        let pos = self
            .command_history
            .partition_point(|cmd| cmd.prompt_line < target_abs);
        if pos == 0 {
            None
        } else {
            Some(pos - 1)
        }
    }

    /// Binary-search: find the first command whose prompt_line > target (S2).
    fn find_next_command_idx(&self, target_abs: usize) -> Option<usize> {
        let pos = self
            .command_history
            .partition_point(|cmd| cmd.prompt_line <= target_abs);
        if pos < self.command_history.len() {
            Some(pos)
        } else {
            None
        }
    }

    /// Jump to the previous command's output from the current scroll position.
    pub fn jump_to_prev_command(&mut self) -> Option<&CommandRecord> {
        let current_abs = self.viewport_top_abs();
        let idx = self.find_prev_command_idx(current_abs)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Jump to the next command's output from the current scroll position.
    pub fn jump_to_next_command(&mut self) -> Option<&CommandRecord> {
        if self.scroll_offset == 0 {
            return None;
        }
        let current_abs = self.viewport_top_abs();
        let idx = self.find_next_command_idx(current_abs)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Jump to a specific command by ID.
    pub fn jump_to_command(&mut self, id: u64) -> Option<&CommandRecord> {
        let idx = self.command_history.iter().position(|cmd| cmd.id == id)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Return the command that is currently visible at the top of the viewport.
    pub fn current_visible_command(&self) -> Option<(usize, &CommandRecord)> {
        let view_abs = self.viewport_top_abs();
        let pos = self
            .command_history
            .partition_point(|cmd| cmd.prompt_line <= view_abs);
        if pos == 0 {
            return None;
        }
        Some((pos - 1, &self.command_history[pos - 1]))
    }

    /// Extract command text from the grid. Returns empty if start has scrolled off (C4).
    pub(crate) fn extract_command_text(&self, start_abs_line: usize, start_col: usize) -> String {
        if start_abs_line < self.total_scrolled_lines {
            return String::new();
        }
        let start_row = start_abs_line - self.total_scrolled_lines;
        let end_col = self.cursor_col;
        let end_row = self.cursor_row;
        let grid = self.grid();
        if start_row >= grid.rows() {
            return String::new();
        }
        let mut text = String::new();
        for row in start_row..=end_row.min(grid.rows().saturating_sub(1)) {
            let col_start = if row == start_row { start_col } else { 0 };
            let col_end = if row == end_row { end_col } else { grid.cols() };
            for col in col_start..col_end.min(grid.cols()) {
                let cell = grid.cell(col, row);
                if cell.width > 0 {
                    text.push(cell.c);
                }
            }
            if row != end_row {
                text.push('\n');
            }
        }
        text.trim().to_string()
    }

    /// Push a command record to history with O(1) eviction (C2/W1: shared helper).
    pub(crate) fn push_command_record(&mut self, record: CommandRecord) {
        self.command_history.push_back(record);
        while self.command_history.len() > self.max_command_history {
            self.command_history.pop_front();
        }
    }

    /// Finalize a pending command and add it to history (A->A path, no D received).
    pub(crate) fn finalize_pending_command(&mut self, current_abs_line: usize) {
        if let Some(pending) = self.pending_command.take() {
            if let (Some(started_at), Some(output_start)) =
                (pending.started_at, pending.output_start_abs_line)
            {
                let id = self.next_command_id;
                self.next_command_id += 1;
                // C1: consume stored command text
                let cmd_text = self.pending_command_text.take().unwrap_or_default();
                let record = CommandRecord {
                    id,
                    command_text: cmd_text,
                    cwd: pending.cwd,
                    timestamp: started_at,
                    duration_ms: None,
                    exit_code: None,
                    scrollback_line_start: output_start,
                    scrollback_line_end: Some(current_abs_line),
                    prompt_line: pending.prompt_abs_line,
                };
                self.push_command_record(record);
            }
        }
    }
}
