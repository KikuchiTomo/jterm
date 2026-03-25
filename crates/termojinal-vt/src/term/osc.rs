//! OSC (Operating System Command) dispatch handling.

use base64::Engine as _;

use crate::image;

use super::command::{CommandRecord, PendingCommand};
use super::modes::ClipboardEvent;
use super::Terminal;

impl Terminal {
    /// Handle the `osc_dispatch` callback from vte::Perform.
    pub(crate) fn do_osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        let cmd = std::str::from_utf8(params[0]).unwrap_or("");

        match cmd {
            "0" | "2" => {
                if let Some(title) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(title) {
                        self.osc.title = s.to_string();
                        log::debug!("title: {s}");
                    }
                }
            }
            // OSC 1 — Set icon name (often treated same as title).
            "1" => {
                if let Some(name) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(name) {
                        log::trace!("icon name: {s}");
                        // Many terminals treat icon name = title.
                        // We store it in the title for simplicity.
                    }
                }
            }
            // OSC 10 — Query/set default foreground color.
            "10" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with current foreground color.
                            // Use a sensible default (white-ish for dark themes).
                            log::trace!("OSC 10 query -> default fg");
                            self.queue_response(b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\".to_vec());
                        } else {
                            log::trace!("OSC 10 set fg: {s}");
                            // Setting foreground color — store for theme-aware apps.
                            // Actual color application depends on the renderer.
                        }
                    }
                }
            }
            // OSC 11 — Query/set default background color.
            "11" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with current background color.
                            // Use a sensible default (dark for dark themes).
                            log::trace!("OSC 11 query -> default bg");
                            self.queue_response(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\".to_vec());
                        } else {
                            log::trace!("OSC 11 set bg: {s}");
                        }
                    }
                }
            }
            // OSC 12 — Query/set cursor color.
            "12" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with cursor color.
                            log::trace!("OSC 12 query -> cursor color");
                            self.queue_response(b"\x1b]12;rgb:cccc/cccc/cccc\x1b\\".to_vec());
                        } else {
                            log::trace!("OSC 12 set cursor color: {s}");
                        }
                    }
                }
            }
            "7" => {
                if let Some(uri) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(uri) {
                        self.osc.cwd_uri = s.to_string();
                        let path = s
                            .strip_prefix("file://")
                            .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
                            .unwrap_or(s);
                        self.osc.cwd = path.to_string();
                        log::debug!("cwd: {path}");
                    }
                }
            }
            "9" => {
                if let Some(msg) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 9): {s}");
                    }
                }
            }
            "99" => {
                if let Some(msg) = params.last() {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 99): {s}");
                    }
                }
            }
            "133" => {
                if let Some(sub) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(sub) {
                        match s.chars().next() {
                            Some('A') => {
                                self.osc.prompt_start = Some((self.cursor_col, self.cursor_row));
                                log::debug!("OSC 133: prompt start");

                                if self.command_history_enabled {
                                    let current_abs = self.abs_line(self.cursor_row);
                                    // Finalize previous pending command if any
                                    self.finalize_pending_command(current_abs);
                                    // Start tracking a new command
                                    self.pending_command = Some(PendingCommand {
                                        prompt_abs_line: current_abs,
                                        command_start_abs_line: 0,
                                        command_start_col: 0,
                                        cwd: self.osc.cwd.clone(),
                                        started_at: None,
                                        output_start_abs_line: None,
                                    });
                                }
                            }
                            Some('B') => {
                                self.osc.command_start = Some((self.cursor_col, self.cursor_row));
                                log::debug!("OSC 133: command start");

                                if self.command_history_enabled {
                                    let abs = self.abs_line(self.cursor_row);
                                    let col = self.cursor_col;
                                    if let Some(ref mut pending) = self.pending_command {
                                        pending.command_start_abs_line = abs;
                                        pending.command_start_col = col;
                                    }
                                }
                            }
                            Some('C') => {
                                log::debug!("OSC 133: command executed");

                                if self.command_history_enabled {
                                    // Extract command text from grid (between B and C)
                                    // C4: pass abs_line directly; extract_command_text handles bounds
                                    let cmd_text = if let Some(ref pending) = self.pending_command {
                                        let start_abs = pending.command_start_abs_line;
                                        let start_col = pending.command_start_col;
                                        self.extract_command_text(start_abs, start_col)
                                    } else {
                                        String::new()
                                    };

                                    let abs = self.abs_line(self.cursor_row);
                                    if let Some(ref mut pending) = self.pending_command {
                                        pending.started_at = Some(chrono::Utc::now());
                                        pending.output_start_abs_line = Some(abs);
                                    }

                                    self.pending_command_text = Some(cmd_text);
                                }
                            }
                            Some('D') => {
                                log::debug!("OSC 133: command finished");

                                if self.command_history_enabled {
                                    // Parse exit code from parameters (e.g., "D;0" or "D;1")
                                    // W4: robust exit code parsing via strip_prefix
                                    let exit_code = s
                                        .strip_prefix('D')
                                        .and_then(|r| r.strip_prefix(';'))
                                        .and_then(|r| r.parse::<i32>().ok());

                                    let current_abs = self.abs_line(self.cursor_row);

                                    // W1: use shared helper for record creation
                                    if let Some(pending) = self.pending_command.take() {
                                        if let (Some(started_at), Some(output_start)) =
                                            (pending.started_at, pending.output_start_abs_line)
                                        {
                                            let duration_ms = chrono::Utc::now()
                                                .signed_duration_since(started_at)
                                                .num_milliseconds()
                                                .max(0)
                                                as u64;
                                            let id = self.next_command_id;
                                            self.next_command_id += 1;
                                            let cmd_text = self
                                                .pending_command_text
                                                .take()
                                                .unwrap_or_default();
                                            let record = CommandRecord {
                                                id,
                                                command_text: cmd_text,
                                                cwd: pending.cwd,
                                                timestamp: started_at,
                                                duration_ms: Some(duration_ms),
                                                exit_code,
                                                scrollback_line_start: output_start,
                                                scrollback_line_end: Some(current_abs),
                                                prompt_line: pending.prompt_abs_line,
                                            };
                                            self.push_command_record(record);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            // OSC 8 — Hyperlinks: OSC 8 ; params ; URI ST
            "8" => {
                // params[1] = hyperlink params (e.g. "id=xyz"), params[2] = URI
                let uri = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if uri.is_empty() {
                    // End hyperlink.
                    self.current_hyperlink = None;
                    log::trace!("OSC 8: end hyperlink");
                } else {
                    // Start hyperlink.
                    self.current_hyperlink = Some(uri.to_string());
                    log::trace!("OSC 8: start hyperlink uri={uri}");
                }
            }
            // OSC 52 — Clipboard: OSC 52 ; selection ; data ST
            "52" => {
                let selection = params
                    .get(1)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("c")
                    .to_string();
                let raw_data = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if raw_data == "?" {
                    self.clipboard_event = Some(ClipboardEvent::Query { selection });
                    log::trace!(
                        "OSC 52: query clipboard selection={}",
                        &self
                            .clipboard_event
                            .as_ref()
                            .map(|e| match e {
                                ClipboardEvent::Query { selection } => selection.as_str(),
                                _ => "",
                            })
                            .unwrap_or("")
                    );
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(raw_data) {
                        Ok(bytes) => {
                            let decoded = String::from_utf8_lossy(&bytes).to_string();
                            log::trace!("OSC 52: set clipboard selection={selection}");
                            self.clipboard_event = Some(ClipboardEvent::Set {
                                selection,
                                data: decoded,
                            });
                        }
                        Err(e) => {
                            log::trace!("OSC 52: invalid base64: {e}");
                        }
                    }
                }
            }
            "777" => {
                if let Some(msg) = params.get(2) {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 777): {s}");
                    }
                }
            }
            // OSC 1337 — iTerm2 proprietary sequences (inline images, etc.)
            //
            // vte splits OSC payloads at ';', so an iTerm2 sequence like:
            //   OSC 1337 ; File=inline=1;size=123:BASE64 ST
            // arrives as params = ["1337", "File=inline=1", "size=123:BASE64"].
            // We must rejoin params[1..] with ';' to reconstruct the full payload.
            "1337" => {
                if params.len() < 2 {
                    return;
                }
                // Rejoin all params after "1337" to reconstruct the original payload.
                let payload = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");

                if payload.starts_with("File=") {
                    // Check if this is a file transfer (inline=0) or inline image.
                    let is_inline = payload.contains("inline=1");
                    let is_non_inline =
                        payload.contains("inline=0") || !payload.contains("inline=");

                    if is_inline {
                        // Legacy single-sequence inline image.
                        let col = self.cursor_col;
                        let row = self.cursor_row;
                        let before = self.image_store.placements().len();
                        image::parse_iterm2_image(&payload, &mut self.image_store, col, row);
                        // Advance cursor past the image so text flows below it.
                        if self.image_store.placements().len() > before {
                            self.image_store.cap_placement_size(self.cols, self.rows);
                            let cell_rows = self
                                .image_store
                                .placements()
                                .last()
                                .map(|p| p.cell_rows)
                                .unwrap_or(1);
                            self.advance_cursor_past_image(cell_rows);
                        }
                    } else if is_non_inline {
                        // File transfer: decode and emit a FileTransferEvent.
                        if let Some(rest) = payload.strip_prefix("File=") {
                            if let Some(colon_idx) = rest.rfind(':') {
                                let params_str = &rest[..colon_idx];
                                let b64_data = &rest[colon_idx + 1..];

                                // Parse file name from params.
                                let mut file_name = String::new();
                                for kv in params_str.split(';') {
                                    let mut parts = kv.splitn(2, '=');
                                    let key = parts.next().unwrap_or("");
                                    let value = parts.next().unwrap_or("");
                                    if key == "name" {
                                        file_name = base64::engine::general_purpose::STANDARD
                                            .decode(value)
                                            .ok()
                                            .and_then(|b| String::from_utf8(b).ok())
                                            .unwrap_or_default();
                                    }
                                }

                                // Decode file data.
                                if let Ok(data) =
                                    base64::engine::general_purpose::STANDARD.decode(b64_data)
                                {
                                    self.file_transfer_event =
                                        Some(super::modes::FileTransferEvent {
                                            name: file_name,
                                            data,
                                        });
                                    log::debug!("iTerm2 file transfer: received file");
                                }
                            }
                        }
                    }
                } else if payload.starts_with("MultipartFile=") {
                    // Multipart transfer: begin (metadata, no pixel data).
                    self.iterm2_accumulator.begin(&payload);
                } else if payload.starts_with("FilePart=") {
                    // Multipart transfer: data chunk.
                    self.iterm2_accumulator.add_part(&payload);
                } else if payload == "FileEnd" {
                    // Multipart transfer: finalize.
                    let col = self.cursor_col;
                    let row = self.cursor_row;
                    let placed = self
                        .iterm2_accumulator
                        .finish(&mut self.image_store, col, row);
                    if placed {
                        self.image_store.cap_placement_size(self.cols, self.rows);
                        let cell_rows = self
                            .image_store
                            .placements()
                            .last()
                            .map(|p| p.cell_rows)
                            .unwrap_or(1);
                        self.advance_cursor_past_image(cell_rows);
                    }
                } else {
                    log::trace!(
                        "OSC 1337: unhandled sub-command: {}",
                        &payload[..payload.len().min(30)]
                    );
                }
            }
            _ => {
                log::trace!("unhandled OSC: {cmd}");
            }
        }
    }
}
