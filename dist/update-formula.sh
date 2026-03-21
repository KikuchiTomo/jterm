#!/usr/bin/env bash
set -euo pipefail

# Update Homebrew formula and cask with version and sha256 values.
# Usage: ./dist/update-formula.sh <version> <cli_sha256> <app_sha256>
#
# Updates both dist/homebrew/termojinal.rb (Formula) and
# dist/homebrew/termojinal-cask.rb (Cask).

VERSION="${1:?Usage: update-formula.sh <version> <cli_sha> <app_sha>}"
CLI_SHA="${2:?}"
APP_SHA="${3:?}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FORMULA="${REPO_ROOT}/dist/homebrew/termojinal.rb"
CASK="${REPO_ROOT}/dist/homebrew/termojinal-cask.rb"

if [[ ! -f "$FORMULA" ]]; then
    echo "Error: $FORMULA not found" >&2
    exit 1
fi
if [[ ! -f "$CASK" ]]; then
    echo "Error: $CASK not found" >&2
    exit 1
fi

# Validate inputs (defense against injection)
if ! echo "$CLI_SHA" | grep -qE '^[0-9a-f]{64}$'; then
    echo "Error: CLI_SHA is not a valid sha256 hash" >&2
    exit 1
fi
if ! echo "$APP_SHA" | grep -qE '^[0-9a-f]{64}$'; then
    echo "Error: APP_SHA is not a valid sha256 hash" >&2
    exit 1
fi
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$'; then
    echo "Error: VERSION is not valid semver" >&2
    exit 1
fi

# --- Update Formula (CLI tools) ---
awk -v version="$VERSION" -v cli_sha="$CLI_SHA" '
    /TERMOJINAL_VERSION = / {
        print "  TERMOJINAL_VERSION = \"" version "\""
        next
    }
    /^  version / {
        print "  version TERMOJINAL_VERSION"
        next
    }
    /cli-macos-universal\.tar\.gz/ {
        print
        replace_next = "cli"
        next
    }
    replace_next == "cli" && /sha256/ {
        print "  sha256 \"" cli_sha "\""
        replace_next = ""
        next
    }
    { print }
' "$FORMULA" > "${FORMULA}.tmp" && mv "${FORMULA}.tmp" "$FORMULA"

# Verify Formula
if ! grep -q "$CLI_SHA" "$FORMULA"; then
    echo "Error: CLI sha256 not found in formula after update" >&2
    exit 1
fi

echo "[ok] Formula updated: v${VERSION}"

# --- Update Cask (.app bundle) ---
awk -v version="$VERSION" -v app_sha="$APP_SHA" '
    /^  version / {
        print "  version \"" version "\""
        next
    }
    /^  sha256 / {
        print "  sha256 \"" app_sha "\""
        next
    }
    /releases\/download\/v/ {
        # URL line — version is interpolated via #{version}, no change needed
        print
        next
    }
    { print }
' "$CASK" > "${CASK}.tmp" && mv "${CASK}.tmp" "$CASK"

# Verify Cask
if ! grep -q "$APP_SHA" "$CASK"; then
    echo "Error: APP sha256 not found in cask after update" >&2
    exit 1
fi

echo "[ok] Cask updated: v${VERSION}"
