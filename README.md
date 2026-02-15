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
| `variance-p2p` | libp2p networking | libp2p, tokio |
| `variance-identity` | DID & identity | variance-p2p, ed25519-dalek |
| `variance-messaging` | Messaging logic | variance-identity, ulid |
| `variance-media` | WebRTC signaling | variance-p2p |
| `variance-app` | HTTP API & state | axum, all crates |
| `variance-cli` | CLI entry point | variance-app, clap |

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
# Run CLI
cargo run --bin variance -- start

# Generate identity
cargo run --bin variance -- gen-identity
```

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
RUST_LOG=debug cargo run --bin variance -- start
RUST_LOG=variance=trace,libp2p=debug cargo run --bin variance -- start
```

## Architecture Documentation

- [ARCHITECTURE-CORRECTIONS.md](docs/ARCHITECTURE-CORRECTIONS.md) - **Read this first!** Explains why the Go design was wrong and how we fix it
- [Go docs](../variance-go/docs/) - Original architecture docs (for reference, contains flaws)

## Project Status

**Phase**: Foundation / Early Development

- [x] Workspace structure
- [x] Protobuf schemas
- [ ] libp2p node setup
- [ ] DID generation and IPNS publishing
- [ ] Identity resolution protocol
- [ ] Caching layer
- [ ] Direct messaging
- [ ] Group messaging
- [ ] WebRTC signaling
- [ ] HTTP API
- [ ] Tauri desktop app

## License

MIT OR Apache-2.0

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
