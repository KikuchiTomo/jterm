//! Session management for termojinal.
//!
//! Manages PTY sessions with JSON persistence and daemon support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

pub mod daemon;
pub mod hotkey;
pub mod persistence;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("PTY error: {0}")]
    Pty(#[from] termojinal_pty::PtyError),
}

/// Serializable session state for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: DateTime<Utc>,
    pub pid: Option<i32>,
}

impl SessionState {
    pub fn new(shell: &str, cwd: &str, cols: u16, rows: u16) -> Self {
        let id = Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            name: format!("session-{}", &id[..8]),
            shell: shell.to_string(),
            cwd: cwd.to_string(),
            env: termojinal_pty::default_env(),
            cols,
            rows,
            created_at: Utc::now(),
            pid: None,
        }
    }
}

/// A live session: state + active PTY.
pub struct Session {
    pub state: SessionState,
    pub pty: termojinal_pty::Pty,
}

impl Session {
    /// Create a new session by spawning a PTY.
    pub fn spawn(state: SessionState) -> Result<Self, SessionError> {
        let config = termojinal_pty::PtyConfig {
            shell: state.shell.clone(),
            size: termojinal_pty::PtySize {
                cols: state.cols,
                rows: state.rows,
            },
            env: state.env.clone(),
            working_dir: Some(state.cwd.clone()),
        };

        let pty = termojinal_pty::Pty::spawn(&config)?;
        let mut state = state;
        state.pid = Some(pty.pid().as_raw());

        Ok(Session { state, pty })
    }

    /// Resize the session's PTY.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.state.cols = cols;
        self.state.rows = rows;
        self.pty
            .resize(termojinal_pty::PtySize { cols, rows })
            .map_err(SessionError::from)
    }

    /// Check if the session's process is still alive.
    pub fn is_alive(&self) -> bool {
        self.pty.is_alive()
    }

    /// Update the session's current working directory (e.g. from OSC 7).
    pub fn update_cwd(&mut self, cwd: &str) {
        self.state.cwd = cwd.to_string();
    }
}

/// Manages multiple sessions.
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    /// Externally-spawned sessions (e.g. UI-owned PTYs) tracked by pane ID.
    /// The daemon does not own the PTY — it only records the state so that
    /// `tm list` can report them.
    tracked: HashMap<u64, SessionState>,
    persistence: persistence::SessionStore,
}

impl SessionManager {
    pub fn new() -> Result<Self, SessionError> {
        let persistence = persistence::SessionStore::new()?;
        Ok(Self {
            sessions: HashMap::new(),
            tracked: HashMap::new(),
            persistence,
        })
    }

    /// Create and spawn a new session.
    pub fn create_session(
        &mut self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<&Session, SessionError> {
        let state = SessionState::new(shell, cwd, cols, rows);
        let id = state.id.clone();
        let session = Session::spawn(state)?;
        self.persistence.save(&session.state)?;
        self.sessions.insert(id.clone(), session);
        Ok(self.sessions.get(&id).unwrap())
    }

    /// Get a session by ID.
    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// Get a mutable session by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    /// Remove a session.
    pub fn remove(&mut self, id: &str) -> Result<(), SessionError> {
        self.sessions.remove(id);
        self.persistence.remove(id)?;
        Ok(())
    }

    /// List all session IDs (daemon-owned + externally tracked).
    pub fn list(&self) -> Vec<&str> {
        self.sessions
            .keys()
            .map(|s| s.as_str())
            .chain(self.tracked.values().map(|s| s.id.as_str()))
            .collect()
    }

    /// List full details for all sessions (daemon-owned + externally tracked).
    pub fn list_details(&self) -> Vec<&SessionState> {
        self.sessions
            .values()
            .map(|s| &s.state)
            .chain(self.tracked.values())
            .collect()
    }

    /// Save all session states to disk.
    pub fn save_all(&self) -> Result<(), SessionError> {
        for session in self.sessions.values() {
            self.persistence.save(&session.state)?;
        }
        Ok(())
    }

    /// Load saved session states from disk (does not reattach PTYs).
    pub fn load_saved_states(&self) -> Result<Vec<SessionState>, SessionError> {
        self.persistence.load_all()
    }

    /// Remove a saved session file from disk without affecting live sessions.
    /// Used to clean up stale session files on daemon startup.
    pub fn remove_saved(&self, id: &str) -> Result<(), SessionError> {
        self.persistence.remove(id)
    }

    /// Update a session's CWD (e.g. when OSC 7 is received) and persist it.
    pub fn update_session_cwd(&mut self, id: &str, cwd: &str) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.update_cwd(cwd);
            self.persistence.save(&session.state)?;
        }
        Ok(())
    }

    /// Clean up dead sessions (daemon-owned and externally tracked).
    pub fn reap_dead(&mut self) -> Vec<String> {
        // Reap daemon-owned sessions.
        let dead: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !s.is_alive())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.sessions.remove(id);
            let _ = self.persistence.remove(id);
        }

        // Reap externally tracked sessions whose PIDs are no longer alive.
        let dead_tracked: Vec<u64> = self
            .tracked
            .iter()
            .filter(|(_, s)| {
                let Some(pid) = s.pid else {
                    return true;
                };
                use nix::sys::signal;
                use nix::unistd::Pid;
                signal::kill(Pid::from_raw(pid), None).is_err()
            })
            .map(|(pane_id, _)| *pane_id)
            .collect();
        for pane_id in &dead_tracked {
            if let Some(state) = self.tracked.remove(pane_id) {
                let _ = self.persistence.remove(&state.id);
            }
        }

        // Return all reaped IDs.
        dead.into_iter()
            .chain(dead_tracked.iter().map(|id| format!("tracked-pane-{id}")))
            .collect()
    }

    /// Register an externally-spawned session (UI-owned PTY).
    ///
    /// The daemon does not own or manage the PTY — it only records the
    /// session state so that `tm list` reports it.  The session is keyed
    /// by `pane_id` so the UI can unregister it later.
    pub fn register_external_session(
        &mut self,
        pane_id: u64,
        pid: i32,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> String {
        let mut state = SessionState::new(shell, cwd, cols, rows);
        state.pid = Some(pid);
        state.name = format!("pane-{}", pane_id);
        let id = state.id.clone();
        self.persistence.save(&state).ok();
        self.tracked.insert(pane_id, state);
        id
    }

    /// Unregister an externally-spawned session by pane ID.
    pub fn unregister_external_session(&mut self, pane_id: u64) -> bool {
        if let Some(state) = self.tracked.remove(&pane_id) {
            let _ = self.persistence.remove(&state.id);
            true
        } else {
            false
        }
    }

    /// Kill all sessions (daemon-owned and externally tracked).
    /// Daemon-owned sessions are dropped (SIGHUP sent to PTY child).
    /// Externally tracked sessions are sent SIGKILL.
    pub fn kill_all(&mut self) -> usize {
        let count = self.sessions.len() + self.tracked.len();

        // Kill externally tracked sessions by sending SIGKILL to their PIDs.
        for state in self.tracked.values() {
            if let Some(pid) = state.pid {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
        }

        // Remove all persistence files.
        let _ = self.persistence.clear();

        // Clear all sessions (dropping Session sends SIGHUP via PTY drop).
        self.sessions.clear();
        self.tracked.clear();

        count
    }

    /// Gracefully exit a session by ID.
    /// Returns `Ok(None)` if the session was exited cleanly.
    /// Returns `Ok(Some(proc_name))` if a foreground process is running
    /// (caller should confirm before forcing).
    /// Returns `Err` if the session was not found.
    pub fn exit_session(&mut self, id: &str) -> Result<Option<String>, SessionError> {
        // Check daemon-owned sessions first.
        if let Some(session) = self.sessions.get(id) {
            // Check for foreground child process.
            let pid = session.pty.pid().as_raw();
            if let Some(proc_name) = detect_foreground_child_of(pid) {
                return Ok(Some(proc_name));
            }
            // No foreground child — remove the session (PTY drop sends SIGHUP).
            self.sessions.remove(id);
            let _ = self.persistence.remove(id);
            return Ok(None);
        }

        // Check externally tracked sessions.
        let tracked_entry = self.tracked.iter()
            .find(|(_, s)| s.id == id)
            .map(|(pane_id, s)| (*pane_id, s.pid));
        if let Some((pane_id, pid)) = tracked_entry {
            if let Some(pid) = pid {
                // Check for foreground child.
                if let Some(proc_name) = detect_foreground_child_of(pid) {
                    return Ok(Some(proc_name));
                }
                // Send SIGHUP to the tracked process.
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGHUP);
            }
            self.tracked.remove(&pane_id);
            let _ = self.persistence.remove(id);
            return Ok(None);
        }

        Err(SessionError::NotFound(id.to_string()))
    }

    /// Force-exit a session by ID, regardless of running processes.
    pub fn force_exit_session(&mut self, id: &str) -> Result<(), SessionError> {
        // Check daemon-owned sessions.
        if self.sessions.remove(id).is_some() {
            let _ = self.persistence.remove(id);
            return Ok(());
        }

        // Check externally tracked sessions.
        let tracked_entry = self.tracked.iter()
            .find(|(_, s)| s.id == id)
            .map(|(pane_id, s)| (*pane_id, s.pid));
        if let Some((pane_id, pid)) = tracked_entry {
            if let Some(pid) = pid {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
            self.tracked.remove(&pane_id);
            let _ = self.persistence.remove(id);
            return Ok(());
        }

        Err(SessionError::NotFound(id.to_string()))
    }
}

/// Detect if a process has a foreground child (i.e. something is running in the shell).
/// Returns the child process name if found, or `None` if the shell is idle.
fn detect_foreground_child_of(pid: i32) -> Option<String> {
    use std::process::Command;
    // Use pgrep to find child processes of the given PID.
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let child_pid = stdout.lines().next()?.trim().parse::<i32>().ok()?;
    // Get the process name of the child.
    let output = Command::new("ps")
        .args(["-p", &child_pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}
