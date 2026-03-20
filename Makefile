.PHONY: all install build run-dev clean test fmt lint check

# Default: build in release mode
all: build

# Install all required tools and dependencies
install:
	@echo "==> Checking Rust toolchain..."
	@command -v rustc >/dev/null 2>&1 || { echo "Installing Rust..."; curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; }
	@echo "==> Rust $$(rustc --version)"
	@if command -v rustup >/dev/null 2>&1; then \
		echo "==> Installing cargo components..."; \
		rustup component add clippy rustfmt; \
	else \
		echo "==> rustup not found (Homebrew Rust?), skipping component install"; \
	fi
	@echo "==> Fetching crate dependencies..."
	cargo fetch
	@echo "==> Done."

# Build in release mode
build:
	cargo build --release

# Run in development mode (debug build, direct PTY)
run-dev:
	RUST_LOG=info cargo run --bin jterm-dev

# Run in development mode with debug logging
run-dev-debug:
	RUST_LOG=debug cargo run --bin jterm-dev

# Run the session daemon
run-daemon:
	RUST_LOG=info cargo run -p jterm-session --bin jtermd

# Run all tests
test:
	cargo test --workspace

# Format code
fmt:
	cargo fmt --all

# Lint with clippy
lint:
	cargo clippy --workspace -- -D warnings

# Check without building
check:
	cargo check --workspace

# Clean build artifacts
clean:
	cargo clean
