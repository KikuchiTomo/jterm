//! jterm-dev — development mode terminal.
//!
//! A simple passthrough terminal for testing the PTY and VT parser.
//! Puts the host terminal in raw mode, spawns a PTY, and forwards I/O.
//! The VT parser runs in parallel to build an internal cell grid.

use jterm_pty::{Pty, PtyConfig, PtySize};
use jterm_vt::Terminal;

use nix::sys::termios::{self, SetArg, Termios};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Save and restore the host terminal's termios on drop.
struct RawMode {
    original: Termios,
    fd: i32,
}

impl RawMode {
    fn enter() -> std::io::Result<Self> {
        let fd = std::io::stdin().as_raw_fd();
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let original = termios::tcgetattr(borrowed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut raw = original.clone();
        termios::cfmakeraw(&mut raw);
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        termios::tcsetattr(borrowed, SetArg::TCSANOW, &raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(Self { original, fd })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let borrowed = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let _ = termios::tcsetattr(borrowed, SetArg::TCSANOW, &self.original);
    }
}

/// Get current terminal size.
fn get_terminal_size() -> PtySize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        PtySize {
            cols: ws.ws_col,
            rows: ws.ws_row,
        }
    } else {
        PtySize::default()
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let size = get_terminal_size();
    let config = PtyConfig {
        size,
        ..PtyConfig::default()
    };

    eprintln!(
        "\x1b[2mjterm-dev: shell={}, size={}x{}\x1b[0m",
        config.shell, size.cols, size.rows
    );

    let pty = match Pty::spawn(&config) {
        Ok(pty) => pty,
        Err(e) => {
            eprintln!("failed to spawn PTY: {e}");
            std::process::exit(1);
        }
    };

    // Enter raw mode on the host terminal.
    let _raw = match RawMode::enter() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to enter raw mode: {e}");
            std::process::exit(1);
        }
    };

    let running = Arc::new(AtomicBool::new(true));

    // Register SIGWINCH handler.
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, running.clone()).ok();

    // Thread: stdin → PTY (forward user input).
    let running_clone = running.clone();
    let master_fd = pty.master_fd();
    let input_thread = std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        while running_clone.load(Ordering::Relaxed) {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let borrowed = unsafe { BorrowedFd::borrow_raw(master_fd) };
                    if nix::unistd::write(borrowed, &buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        running_clone.store(false, Ordering::Relaxed);
    });

    // Main loop: PTY → stdout (forward terminal output).
    let mut vt_parser = vte::Parser::new();
    let mut term = Terminal::new(size.cols as usize, size.rows as usize);
    let mut buf = [0u8; 65536];
    let mut stdout = std::io::stdout();
    let mut last_size = size;

    while running.load(Ordering::Relaxed) {
        // Check for resize.
        let current_size = get_terminal_size();
        if current_size.cols != last_size.cols || current_size.rows != last_size.rows {
            last_size = current_size;
            let _ = pty.resize(current_size);
            term.resize(current_size.cols as usize, current_size.rows as usize);
        }

        match pty.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let data = &buf[..n];

                // Feed through VT parser to build internal state.
                term.feed(&mut vt_parser, data);

                // Write raw output to stdout for display.
                let _ = stdout.write_all(data);
                let _ = stdout.flush();
            }
            Err(_) => break,
        }
    }

    running.store(false, Ordering::Relaxed);
    let _ = input_thread.join();
}
