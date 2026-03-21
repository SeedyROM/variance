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

# Write a shared relay config into each instance's data dir so both peers can
# reach the relay at startup. Users can edit config.toml in the data dir to
# change relay peers permanently; this only writes it if the file is absent.
RELAY_PEER_ID="${VARIANCE_RELAY_PEER_ID:-12D3KooWRHcV1jjQg5E39ZAVckaCTXVFrizrvGZQbJ5LbLqpC6GB}"
RELAY_MULTIADDR="${VARIANCE_RELAY_MULTIADDR:-/ip4/127.0.0.1/tcp/4001}"

for DIR in "$ALICE_DIR" "$BOB_DIR"; do
  if [[ ! -f "$DIR/config.toml" ]]; then
    cat > "$DIR/config.toml" <<TOML
[server]
host = "127.0.0.1"
port = 3000

[p2p]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []

[[p2p.relay_peers]]
peer_id = "$RELAY_PEER_ID"
multiaddr = "$RELAY_MULTIADDR"

[identity]
ipfs_api = "http://127.0.0.1:5001"
cache_ttl_secs = 3600

[media]
stun_servers = ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"]
turn_servers = []

[storage]
group_message_max_age_days = 30
TOML
    echo "  ✓ Wrote relay config → $DIR/config.toml"
  fi
done

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
