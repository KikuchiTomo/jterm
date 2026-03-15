//! jtermd daemon — manages sessions and listens for connections.

use crate::{SessionError, SessionManager};
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Mutex;

/// The daemon state.
pub struct Daemon {
    manager: Arc<Mutex<SessionManager>>,
    socket_path: String,
}

impl Daemon {
    pub fn new() -> Result<Self, SessionError> {
        let manager = SessionManager::new()?;
        let socket_path = socket_path();

        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            socket_path,
        })
    }

    /// Run the daemon event loop.
    pub async fn run(&self) -> Result<(), SessionError> {
        // Clean up any stale socket file.
        if std::path::Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path).ok();
        }

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(&self.socket_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| SessionError::Io(e))?;

        log::info!("jtermd listening on {}", self.socket_path);

        // Restore any saved sessions.
        {
            let manager = self.manager.lock().await;
            match manager.load_saved_states() {
                Ok(states) => {
                    log::info!("found {} saved sessions", states.len());
                    // In Phase 1, we log them but don't re-attach (no PTY to restore to).
                    for state in &states {
                        log::info!(
                            "  session {}: shell={}, cwd={}",
                            state.name,
                            state.shell,
                            state.cwd
                        );
                    }
                }
                Err(e) => {
                    log::warn!("failed to load saved sessions: {e}");
                }
            }
        }

        // Periodically reap dead sessions.
        let manager = self.manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let mut mgr = manager.lock().await;
                let dead = mgr.reap_dead();
                for id in &dead {
                    log::info!("reaped dead session: {id}");
                }
            }
        });

        // Accept connections (Phase 1: basic loop, full IPC in Phase 2).
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let manager = self.manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, manager).await {
                            log::error!("connection error: {e}");
                        }
                    });
                }
                Err(e) => {
                    log::error!("accept error: {e}");
                }
            }
        }
    }

    pub fn manager(&self) -> &Arc<Mutex<SessionManager>> {
        &self.manager
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Clean up socket file.
        std::fs::remove_file(&self.socket_path).ok();
    }
}

/// Handle a single IPC connection.
async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
) -> Result<(), SessionError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(SessionError::Io)?;

    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    log::debug!("IPC request: {request}");

    // Phase 1: simple text protocol.
    let response = match request.trim() {
        "list" => {
            let mgr = manager.lock().await;
            let ids = mgr.list();
            if ids.is_empty() {
                "no sessions\n".to_string()
            } else {
                ids.join("\n") + "\n"
            }
        }
        "ping" => "pong\n".to_string(),
        _ => format!("unknown command: {}\n", request.trim()),
    };

    stream
        .write_all(response.as_bytes())
        .await
        .map_err(SessionError::Io)?;

    Ok(())
}

/// Get the Unix socket path for jtermd.
pub fn socket_path() -> String {
    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir
        .join("jterm")
        .join("jtermd.sock")
        .to_string_lossy()
        .to_string()
}
