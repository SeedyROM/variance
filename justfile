# Variance Project Justfile
# Run `just --list` to see all available commands

# Default recipe - show help
default:
    @just --list

# === Rust Commands ===

# Run all cargo tests
test:
    cargo test --all-features

# Run tests for a specific package
test-package package:
    cargo test -p {{package}}

# Build the project (debug mode)
build:
    cargo build --all-features

# Build the project (release mode)
build-release:
    cargo build --release --all-features

# Run cargo check on all targets
check:
    cargo check --all-targets --all-features

# Run clippy with warnings as errors
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Format all Rust code
fmt:
    cargo fmt --all

# Check if code is formatted
fmt-check:
    cargo fmt --all -- --check

# Run the CLI
run *ARGS:
    cargo run --bin variance -- {{ARGS}}

# Clean build artifacts
clean:
    cargo clean
    cd app && pnpm run tauri clean || true

# === Frontend/Tauri Commands ===

# Run the Tauri desktop app in dev mode
dev:
    cd app && pnpm run tauri:dev

# Build the Tauri desktop app
tauri-build:
    cd app && pnpm run tauri:build

# Run the Vite dev server (frontend only, no Tauri)
frontend-dev:
    cd app && pnpm run dev

# Build the frontend (no Tauri)
frontend-build:
    cd app && pnpm run build

# Preview the built frontend
frontend-preview:
    cd app && pnpm run preview

# Install frontend dependencies
frontend-install:
    cd app && pnpm install

# Format frontend code with Prettier
frontend-fmt:
    cd app && pnpm exec prettier --write "src/**/*.{ts,tsx,css,json}"

# === Combined Commands ===

# Run all checks (format check, clippy, tests)
all: fmt-check clippy test
    @echo "✅ All checks passed!"

# Format all code (Rust + Frontend)
fmt-all: fmt frontend-fmt
    @echo "✅ All code formatted!"

# Pre-commit checks (runs what pre-commit would run)
pre-commit: fmt clippy
    @echo "✅ Pre-commit checks passed!"

# === Development Workflow ===

# Quick check before committing
quick: fmt clippy
    @echo "✅ Quick checks passed!"

# Full CI check (what CI would run)
ci: fmt-check clippy test build
    @echo "✅ CI checks passed!"

# Setup the project (install dependencies)
setup:
    @echo "Installing Rust dependencies..."
    cargo build
    @echo "Installing frontend dependencies..."
    cd app && pnpm install
    @echo "✅ Setup complete!"

# Run two instances of the app for testing (using the dev script)
dev-two:
    cd app/scripts && ./dev-two-instances.sh

# === Documentation ===

# Generate and open Rust documentation
doc:
    cargo doc --all-features --no-deps --open

# Generate documentation without opening
doc-build:
    cargo doc --all-features --no-deps

# === Protobuf ===

# Rebuild protobuf files (forces build.rs to run)
proto:
    cargo clean -p variance-proto
    cargo build -p variance-proto
