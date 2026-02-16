# Variance

A peer-to-peer Discord alternative built with Rust, libp2p, and WebRTC.

## Architecture

Variance is a multi-crate workspace implementing a decentralized communication platform with:

- **End-to-end encrypted messaging** (1-on-1 and groups)
- **WebRTC audio/video calls** with NAT traversal
- **Decentralized identity** using W3C DIDs and IPFS/IPNS
- **No central servers** (optional relay/TURN infrastructure)

### Key Design Decisions

This Rust implementation **corrects critical architectural flaws** from the original Go design:

1. **DHT is NOT a database**: We use Kademlia DHT for peer discovery only, not data storage
2. **IPFS/IPNS for identity**: DID documents stored in IPFS, mutable pointers via IPNS
3. **Custom libp2p protocols**: Direct peer queries instead of expensive DHT lookups
4. **Protobuf everywhere**: Type-safe schemas for all P2P communication
5. **Multi-layer caching**: 80%+ cache hit rate for identity lookups

See [docs/ARCHITECTURE-CORRECTIONS.md](docs/ARCHITECTURE-CORRECTIONS.md) for details.

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
- **Crypto**: ed25519-dalek, x25519-dalek
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

## Documentation

### Architecture
- [ARCHITECTURE-CORRECTIONS.md](docs/ARCHITECTURE-CORRECTIONS.md) - **Read this first!** Explains why the Go design was wrong and how we fix it
- [Go docs](../variance-go/docs/) - Original architecture docs (for reference, contains flaws)

### Usage
- [CLI-USAGE.md](docs/CLI-USAGE.md) - Complete command-line interface reference

## Project Status

**Phase**: Core Protocol Implementation

**Recently Completed (2026-02-15):**
- ✅ Protocol handlers with business logic integration
- ✅ Event channel system for application layer
- ✅ Integration test coverage (36/36 passing)
- ✅ Fixed circular dependencies in crate graph

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
- [ ] 🚧 IPFS/IPNS integration for persistent storage
- [ ] 🚧 DHT provider records for username discovery
- [ ] 🚧 Multi-layer cache (disk + network fallback)

**Messaging:**
- [x] Protobuf message schemas
- [x] Double Ratchet encryption for direct messages (full implementation)
- [x] AES-256-GCM for group messages (full implementation)
- [x] Offline message relay protocol handler
- [x] Local storage backend (sled)
- [x] GossipSub integration for groups
- [ ] 🚧 Read receipts and typing indicators
- [ ] 🚧 Message delivery acknowledgments

**Media (WebRTC):**
- [x] Signaling protocol handler
- [x] Offer/Answer/ICE/Control message handling
- [x] Message signing and verification
- [ ] 🚧 Full call manager implementation
- [ ] 🚧 WebRTC peer connection integration
- [ ] 🚧 STUN/TURN configuration

**Application Layer:**
- [x] HTTP API framework (axum)
- [x] Event subscription system
- [x] CLI with identity management
- [ ] 🚧 Complete REST API endpoints
- [ ] 🚧 WebSocket events for Tauri
- [ ] 🚧 Tauri desktop application

**Testing & Documentation:**
- [x] Unit tests for all handlers
- [x] Integration tests for protocol flows
- [x] Architecture documentation
- [x] CHANGELOG tracking progress

### Next Priorities

1. **IPFS/IPNS Integration** - Persistent identity storage
2. **Call Manager** - Full WebRTC peer connection stack
3. **Public API** - Expose protocol functionality to application layer
4. **Message Delivery** - Wire up event subscriptions to deliver messages
5. **Tauri Integration** - Desktop app with event subscriptions

## License

AGPL-3.0 License. See [LICENSE](LICENSE) for details.

## Contributing

This is an early-stage project. Contributions welcome once foundation is stable.

Key areas for future work:
- IPFS/IPNS integration
- Double Ratchet implementation for DMs
- Relay node infrastructure
- Mobile support (iOS/Android)
- Browser extension

---

**Note**: This project learns from the mistakes in the [variance-go](../variance-go) implementation. See ARCHITECTURE-CORRECTIONS.md for details on what we're doing differently.
