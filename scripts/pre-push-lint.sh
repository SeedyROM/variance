#!/usr/bin/env bash
# Run cargo fmt/clippy/check at pre-push, but skip if pre-commit already verified
# this exact tree. If --no-verify was used, the stamp will be stale and checks run.

set -euo pipefail

STAMP_FILE=".git/.lint-stamp"

stamp=$(cat "$STAMP_FILE" 2>/dev/null || true)
head_tree=$(git rev-parse 'HEAD^{tree}' 2>/dev/null || true)

if [ -n "$stamp" ] && [ "$stamp" = "$head_tree" ]; then
    echo "pre-push lint: already verified at pre-commit (tree $stamp), skipping."
    exit 0
fi

echo "pre-push lint: stamp missing or stale, running checks..."
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check
