# Variance

A peer-to-peer Discord alternative built with Rust, libp2p, and WebRTC.

## Architecture

Variance is a multi-crate workspace implementing a decentralized communication platform with:

- **End-to-end encrypted messaging** (1-on-1 and groups)
- **WebRTC audio/video calls** with NAT traversal
- **Decentralized identity** using W3C DIDs and IPFS/IPNS
- **No central servers** (optional relay/TURN infrastructure)

## Workspace Structure

```
variance/
├── crates/
│   ├── variance-proto/      # Protocol Buffer definitions
│   ├── variance-p2p/        # libp2p networking core
│   ├── variance-identity/   # DID-based identity system
│   ├── variance-messaging/  # Chat and messaging
│   ├── variance-media/      # WebRTC media handling
│   ├── variance-app/        # Application logic & HTTP API
│   └── variance-cli/        # Standalone CLI (headless/debugging only)
├── app/                     # Tauri desktop application (React/TypeScript)
│   ├── src/                 # UI components (onboarding, conversations, messages)
│   └── src-tauri/          # Tauri host (embeds variance_app in-process via FFI)
├── docs/                    # Architecture documentation
└── Cargo.toml              # Workspace manifest
```

### Crate Overview

| Crate | Purpose | Key Dependencies |
|-------|---------|------------------|
| `variance-proto` | Protobuf schemas | prost |
| `variance-p2p` | libp2p networking + protocol handlers | libp2p, tokio, variance-{identity,messaging,media} |
| `variance-identity` | DID & identity logic | variance-proto, ed25519-dalek |
| `variance-messaging` | Messaging logic | variance-proto, variance-identity, ulid |
| `variance-media` | WebRTC signaling logic | variance-proto, ed25519-dalek |
| `variance-app` | HTTP API & state | axum, variance-p2p, variance-{identity,messaging,media} |
| `variance-cli` | Standalone CLI (headless/debugging) | variance-app, clap |
| `variance-desktop` | Tauri desktop host (primary runtime) | tauri, variance-app |

**Architecture note:** The Tauri desktop app embeds `variance-app` in-process — there is no sidecar. The React frontend talks to the Rust node directly via Tauri commands (FFI). `variance-cli` exists for headless operation, debugging, and testing only.

**Note:** As of 2026-02-15, the dependency flow was corrected. `variance-p2p` now depends on the business logic crates (not vice versa) to wire up protocol handlers.

## Technology Stack

- **Language**: Rust 2021 edition
- **P2P**: libp2p 0.55 (Kademlia DHT, GossipSub, custom protocols)
- **Async**: Tokio 1.42
- **HTTP**: Axum 0.8
- **Serialization**: Protocol Buffers (prost)
- **Tracing**: tracing + tracing-subscriber
- **Errors**: snafu
- **Crypto**: ed25519-dalek, vodozemac 0.9 (Olm/Double Ratchet for DMs), OpenMLS / RFC 9420 (group messages)
- **Storage**: sled (embedded KV store)
- **Desktop**: Tauri 2.x (React/TypeScript frontend)
- **Identity**: BIP39 (mnemonic recovery), IPFS/IPNS (storage)
- **IDs**: ULID (messages, calls), chrono (timestamps)

## Getting Started

### Prerequisites

- Rust 1.75+ (with 2021 edition)
- Protocol Buffers compiler (`protoc`)

### Install protoc

**macOS**:
```bash
brew install protobuf
```

**Linux**:
```bash
apt install -y protobuf-compiler  # Debian/Ubuntu
dnf install -y protobuf-compiler  # Fedora
```

**Windows**:
```powershell
choco install protoc
```

### Build

```bash
# Build all crates
cargo build

# Build specific crate
cargo build -p variance-app

# Build release
cargo build --release
```

### Run

The primary way to use Variance is through the **Tauri desktop app**, which embeds the node in-process.

Using the justfile (recommended):
```bash
just dev          # Run the desktop app in dev mode
just tauri-build  # Build a release bundle
just frontend-dev # Run just the frontend (no Tauri/node)
just dev-two      # Run two instances for P2P testing
```

Or directly via pnpm in the `app/` directory:
```bash
cd app
pnpm install      # First time only
pnpm run tauri:dev   # Dev mode with hot reload
pnpm run tauri:build # Release bundle
pnpm run dev         # Frontend only (Vite dev server, no Tauri)
```

The desktop app handles identity generation, configuration, and node startup automatically through its onboarding flow.

### CLI (Debugging & Testing Only)

The `variance` CLI exists for headless operation, debugging, and testing (e.g. generating identities without the UI, running a second node for P2P testing):

```bash
# Generate an identity
cargo run --bin variance -- identity generate

# Start a headless node
cargo run --bin variance -- start

# Show identity info
cargo run --bin variance -- identity show
```

See [docs/CLI-USAGE.md](docs/CLI-USAGE.md) for the full command reference.

## Development

### Running Tests

```bash
cargo test
```

### Pre-Commit Hooks

This project uses [pre-commit](https://pre-commit.com/) to automatically run checks before commits and pushes. The hooks ensure code quality by running formatters, linters, and tests.

**Installation:**

1. Install pre-commit (if not already installed):
   ```bash
   # macOS
   brew install pre-commit

   # Linux/macOS with pip
   pip install pre-commit

   # Or use pipx
   pipx install pre-commit
   ```

2. Install the git hooks in your local repository:
   ```bash
   pre-commit install
   pre-commit install --hook-type pre-push
   ```

**What Gets Checked:**

On every commit:
- **Trailing whitespace removal** - Cleans up line endings
- **End-of-file fixer** - Ensures files end with a newline
- **YAML/TOML validation** - Checks config files for syntax errors
- **Large file detection** - Prevents files >500KB from being committed
- **Merge conflict markers** - Catches unresolved conflicts
- **`cargo fmt`** - Auto-formats Rust code
- **`cargo clippy`** - Lints Rust code with auto-fixes (treats warnings as errors)
- **`cargo check`** - Verifies code compiles

On every push:
- **`cargo test`** - Runs the full test suite

**Manual Usage:**

```bash
# Run all hooks on all files (useful after installation)
pre-commit run --all-files

# Run only on staged files
pre-commit run

# Run specific hook
pre-commit run cargo-fmt --all-files
pre-commit run clippy --all-files

# Skip hooks for a specific commit (not recommended)
git commit --no-verify
```

**Updating Hooks:**

```bash
# Update hook versions to latest
pre-commit autoupdate
```

The pre-commit configuration is in [`.pre-commit-config.yaml`](.pre-commit-config.yaml).

### Code Generation (Protobufs)

Protobufs are automatically compiled during build via `prost-build` in `variance-proto/build.rs`.

To manually trigger:
```bash
cargo build -p variance-proto
```

### Logging

`RUST_LOG` controls log verbosity for both the desktop app and the CLI — the node is the same code either way:

```bash
# Desktop app
RUST_LOG=variance=debug just dev

# CLI (headless)
RUST_LOG=variance=debug cargo run --bin variance -- start

# Trace level with libp2p internals
RUST_LOG=variance=trace,libp2p=debug just dev
```

## Key Design Decisions

This Rust implementation **corrects critical architectural flaws** from the [original Go implementation](https://github.com/SeedyROM/variance-go):

1. **DHT is NOT a database**: We use Kademlia DHT for peer discovery only, not data storage
2. **IPFS/IPNS for identity**: DID documents stored in IPFS, mutable pointers via IPNS
3. **Custom libp2p protocols**: Direct peer queries instead of expensive DHT lookups
4. **Protobuf everywhere**: Type-safe schemas for all P2P communication

See [docs/ARCHITECTURE-CORRECTIONS.md](docs/ARCHITECTURE-CORRECTIONS.md) for the full breakdown.

## Documentation

### Architecture
- [ARCHITECTURE-CORRECTIONS.md](docs/ARCHITECTURE-CORRECTIONS.md) - **Read this first!** Explains why the Go design was wrong and how we fix it
- [variance-go](https://github.com/SeedyROM/variance-go) - Original broken implementation (for reference)

### Usage as standalone node (debugging/testing only)
- [CLI-USAGE.md](docs/CLI-USAGE.md) - CLI reference (debugging/testing only)

### Implementation Checklist

**Foundation:**
- [x] Workspace structure
- [x] Protobuf schemas (identity, messaging, media)
- [x] libp2p node setup (DHT, GossipSub, mDNS, Identify, Ping)
- [x] Custom protocol codecs (request/response)
- [x] Protocol handlers with event channels

**Identity System:**
- [x] DID generation (ed25519 keys)
- [x] Identity resolution protocol handler
- [x] In-memory caching layer
- [x] IPFS/IPNS integration for persistent storage (untested)
- [x] DHT provider records for username discovery (untested)

**Messaging:**
- [x] Protobuf message schemas
- [x] Olm Double Ratchet encryption for direct messages (vodozemac 0.9)
- [x] OpenMLS (RFC 9420) for group messages — per-message forward secrecy, post-compromise security
- [x] Offline message relay protocol handler
- [x] Local storage backend (sled) with ULID-sorted history
- [x] GossipSub integration for groups
- [x] Read receipts and delivery status
- [x] Typing indicators

**Media (WebRTC):**
- [x] Signaling protocol handler
- [x] Offer/Answer/ICE/Control message handling
- [x] Message signing and verification
- [x] Call state management (Ringing → Connecting → Active → Ended)
- [ ] 🚧 WebRTC peer connection (media stream negotiation)
- [ ] 🚧 STUN/TURN configuration

**Application Layer:**
- [x] HTTP API framework (axum)
- [x] Complete REST API endpoints (identity, messages, groups, calls, signaling, receipts, typing, presence)
- [x] Cursor-based message pagination (`?before=<ts>`, newest-first storage scan)
- [x] WebSocket event delivery for Tauri frontend
- [x] Real-time message delivery (inbound tick → component `refetch()`)
- [x] Event subscription system
- [x] CLI with identity management
- [x] Tauri desktop app (onboarding, identity generation/recovery, conversations, messages UI)
- [x] Infinite scroll (scroll-to-top loads older message pages via `IntersectionObserver`)
- [x] Group management API (create, invite, join, leave, list members)
- [x] Presence status endpoint
- [x] Username resolution endpoint

**Testing & Documentation:**
- [x] Unit tests for all handlers
- [x] Integration tests for protocol flows
- [x] Architecture documentation

### Next Priorities

1. **Integration Testing** — IPFS/IPNS and DHT provider records are implemented but untested end-to-end; integration tests need a live daemon
2. **WebRTC Peer Connection** — Signaling protocol is complete; wire up actual media stream negotiation and STUN/TURN server configuration
3. **Relay Node Selection** — Infrastructure for discovery and failover between relay nodes
4. **Call UI** — Tauri frontend has message/conversation UI; call screens and media controls not yet wired

## License

AGPL-3.0 License. See [LICENSE](LICENSE) for details.

## Contributing

This is an early-stage project. Contributions welcome once foundation is stable.

Key areas for future work:
- End-to-end integration testing (IPFS/IPNS, DHT provider records)
- WebRTC peer connection and STUN/TURN
- Relay node selection and failover
- Mobile support (iOS/Android)

---

**Note**: This project learns from the mistakes in the [variance-go](https://github.com/SeedyROM/variance-go) implementation. See ARCHITECTURE-CORRECTIONS.md for details on what we're doing differently.
