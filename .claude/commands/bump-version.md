Bump the patch version, commit, tag, and push.

Steps:
1. Detect the current version from the root Cargo.toml (e.g. `0.9.5-beta`)
2. Increment the patch number (e.g. `0.9.5-beta` -> `0.9.6-beta`)
3. Update ALL Cargo.toml files (excluding target/ and .claude/) and dist/macos/Info.plist
4. Build the daemon and tm binaries: `cargo build --release -p termojinal-session --bin termojinald -p termojinal-ipc --bin tm`
5. `cargo check` to verify
6. Stage all changes, commit with message: `chore: bump version to <new_version>`
7. Tag with `v<new_version>`
8. Push main and the tag to origin
9. Do NOT add Co-Authored-By lines to the commit message
