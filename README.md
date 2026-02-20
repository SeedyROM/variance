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
│   └── variance-cli/        # CLI binary
├── app/                     # Tauri desktop application (React/TypeScript)
│   ├── src/                 # UI components (onboarding, conversations, messages)
│   └── src-tauri/          # Tauri host (embeds variance_app in-process, manages state)
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
| `variance-app` | HTTP API & state | axum, variance-p2p |
| `variance-cli` | CLI entry point | variance-app, clap |

**Note:** As of 2026-02-15, the dependency flow was corrected. `variance-p2p` now depends on the business logic crates (not vice versa) to wire up protocol handlers.

## Technology Stack

- **Language**: Rust 2021 edition
- **P2P**: libp2p 0.55 (Kademlia DHT, GossipSub, custom protocols)
- **Async**: Tokio 1.42
- **HTTP**: Axum 0.8
- **Serialization**: Protocol Buffers (prost)
- **Tracing**: tracing + tracing-subscriber
- **Errors**: snafu
- **Crypto**: ed25519-dalek, vodozemac 0.9 (Olm/Double Ratchet for DMs), AES-256-GCM (group messages)
- **Storage**: sled (embedded KV store)

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
cargo build -p variance-cli

# Build release
cargo build --release
```

### Run

```bash
# Generate an identity first (saves to .variance/identity.json)
# ⚠️ IMPORTANT: Write down the 12-word recovery phrase shown!
cargo run --bin variance -- identity generate

# Initialize configuration (optional)
cargo run --bin variance -- config init

# Start the node (automatically loads identity from .variance/identity.json)
cargo run --bin variance -- start
```

**Recovery:** If you lose your identity file, you can recover it using:
```bash
cargo run --bin variance -- identity recover
```

For detailed CLI usage, see [docs/CLI-USAGE.md](docs/CLI-USAGE.md).

## CLI Commands

The `variance` CLI provides three main command groups:

### Identity Management
```bash
variance identity generate  # Create new DID with 12-word recovery phrase
variance identity recover   # Recover identity from recovery phrase
variance identity show      # Display identity information
```

### Configuration Management
```bash
variance config init  # Create default configuration file
variance config show  # Display current configuration
```

### Node Operations
```bash
variance start  # Start the node (auto-loads identity from .variance/identity.json)
```

**See [docs/CLI-USAGE.md](docs/CLI-USAGE.md) for complete command reference, options, and examples.**

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

Set log level via `RUST_LOG`:
```bash
# Info level (default)
RUST_LOG=variance=info cargo run --bin variance -- start

# Debug level
RUST_LOG=variance=debug cargo run --bin variance -- start

# Trace level with libp2p debug
RUST_LOG=variance=trace,libp2p=debug cargo run --bin variance -- start
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

### Usage
- [CLI-USAGE.md](docs/CLI-USAGE.md) - Complete command-line interface reference

## Project Status

**Phase**: Core Protocol Implementation

**Recently Completed (2026-02-20):**
- ✅ Real-time message delivery fixed — WebSocket inbound tick + `IntersectionObserver`-based infinite scroll
- ✅ Cursor-based message pagination — reverse storage scan (newest-first), `?before=<ts>` API param, scroll-to-top loads older pages
- ✅ TOCTOU session init race fixed — `session_init_lock: Mutex<()>` in `DirectMessageHandler` serializes concurrent session creation
- ✅ Conversation switching — `key={activePeerDid}` forces component remount; `refetchOnMount: "always"` ensures fresh fetch
- ✅ Large enum variants boxed — `Box<IdentityRequest>`, `Box<IdentityResponse>`, `Box<DirectMessage>` in P2P event types

**Previously Completed (2026-02-17):**
- ✅ vodozemac 0.9 migration (replaces unmaintained double-ratchet-2)
- ✅ Complete messaging stack: receipts, typing indicators, message storage
- ✅ Full HTTP REST API (identity, messages, calls, signaling, receipts, typing)
- ✅ WebSocket event delivery for Tauri frontend
- ✅ 232 tests passing across all crates

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
- [x] AES-256-GCM for group messages
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
- [x] Complete REST API endpoints (identity, messages, calls, signaling, receipts, typing)
- [x] Cursor-based message pagination (`?before=<ts>`, newest-first storage scan)
- [x] WebSocket event delivery for Tauri frontend
- [x] Real-time message delivery (inbound tick → component `refetch()`)
- [x] Event subscription system
- [x] CLI with identity management
- [x] Tauri desktop app (onboarding, identity generation/recovery, conversations, messages UI)
- [x] Infinite scroll (scroll-to-top loads older message pages via `IntersectionObserver`)

**Testing & Documentation:**
- [x] Unit tests for all handlers
- [x] Integration tests for protocol flows
- [x] Architecture documentation
- [x] CHANGELOG tracking progress

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
- Browser extension

---

**Note**: This project learns from the mistakes in the [variance-go](https://github.com/SeedyROM/variance-go) implementation. See ARCHITECTURE-CORRECTIONS.md for details on what we're doing differently.
