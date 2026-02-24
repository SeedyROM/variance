#!/usr/bin/env bash
# dev-two-instances.sh
#
# Launches two Variance desktop instances with separate data directories,
# giving each a different identity. On first run each instance goes through
# the normal onboarding flow to generate its identity.
#
# Usage:
#   ./dev-two-instances.sh             # debug build then launch (fast)
#   ./dev-two-instances.sh --release   # release build then launch (slow)
#   ./dev-two-instances.sh --no-build  # skip build, use existing binary
#
# Override the data directories:
#   VARIANCE_ALICE_DIR=/tmp/alice VARIANCE_BOB_DIR=/tmp/bob ./dev-two-instances.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ALICE_DIR="${VARIANCE_ALICE_DIR:-/tmp/variance-alice}"
BOB_DIR="${VARIANCE_BOB_DIR:-/tmp/variance-bob}"

MODE="debug"
case "${1:-}" in
  --no-build) MODE="no-build" ;;
  --release)  MODE="release" ;;
esac

if [[ "$MODE" == "release" ]]; then
  BINARY="$ROOT_DIR/target/release/bundle/macos/Variance.app/Contents/MacOS/variance-desktop"
else
  BINARY="$ROOT_DIR/target/debug/bundle/macos/Variance.app/Contents/MacOS/variance-desktop"
fi

# ── build ─────────────────────────────────────────────────────────────────────

if [[ "$MODE" == "debug" ]]; then
  echo "▶ Building (debug)..."
  (cd "$SCRIPT_DIR/.." && pnpm tauri build --debug --bundles app)
  echo ""
elif [[ "$MODE" == "release" ]]; then
  echo "▶ Building (release)..."
  (cd "$SCRIPT_DIR/.." && pnpm tauri build --bundles app)
  echo ""
fi

if [[ ! -x "$BINARY" ]]; then
  echo "✗ Binary not found: $BINARY" >&2
  echo "  Run without --no-build to build first." >&2
  exit 1
fi

# ── launch ────────────────────────────────────────────────────────────────────

cleanup() {
  echo ""
  echo "Stopping instances..."
  kill "$ALICE_PID" "$BOB_PID" 2>/dev/null || true
  wait "$ALICE_PID" "$BOB_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

mkdir -p "$ALICE_DIR" "$BOB_DIR"

echo "Launching Alice → $ALICE_DIR"
RUST_LOG=debug VARIANCE_DATA_DIR="$ALICE_DIR" "$BINARY" >/tmp/variance-alice.log 2>&1 &
ALICE_PID=$!

sleep 1.5

echo "Launching Bob   → $BOB_DIR"
RUST_LOG=debug VARIANCE_DATA_DIR="$BOB_DIR" "$BINARY" >/tmp/variance-bob.log 2>&1 &
BOB_PID=$!

echo ""
echo "Alice PID: $ALICE_PID  (logs: /tmp/variance-alice.log)"
echo "Bob   PID: $BOB_PID  (logs: /tmp/variance-bob.log)"
echo "Press Ctrl+C to stop both."
echo ""

wait -n "$ALICE_PID" "$BOB_PID" 2>/dev/null || wait "$ALICE_PID" "$BOB_PID" 2>/dev/null || true
