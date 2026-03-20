use thiserror::Error;

#[derive(Error, Debug)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    Open(String),

    #[error("fork failed: {0}")]
    Fork(String),

    #[error("PTY I/O error: {0}")]
    Io(String),

    #[error("PTY resize failed: {0}")]
    Resize(String),

    #[error("signal error: {0}")]
    Signal(String),
}
