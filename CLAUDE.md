# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Context

Variance is a P2P Discord alternative in Rust using IPFS/IPNS for identity storage, custom libp2p protocols for queries, and multi-layer caching.

**Key architectural documents:**
- `docs/QUICK-REFERENCE.md` - Workspace structure and patterns
- `docs/PROTOCOL-GUIDE.md` - Protobuf usage

## Technology Stack

- **Rust 2021**, latest stable dependencies (not training data versions)
- **libp2p 0.55+** - Kademlia DHT, GossipSub, custom protocols
- **Protocol Buffers** (prost) - All P2P communication
- **Tokio** - Async runtime
- **Axum 0.8** - HTTP API for Tauri frontend
- **snafu** - Error handling (preferred over anyhow for libraries)
- **tracing/tracing-subscriber** - Structured logging
- **vodozemac 0.9** - Olm/Double Ratchet for direct message encryption (Matrix-compatible)
- **openmls** - RFC 9420 MLS for group message encryption
- **sled** - Embedded KV store for local persistence

## Development Workflow

```bash
# Build (triggers protobuf codegen)
cargo build

# Check specific crate
cargo check -p variance-identity

# Run all tests
just test                          # cargo test --all-features
just test-package variance-messaging  # tests for one crate

# Run a single test
cargo test -p variance-messaging test_name

# Lint and format
just clippy    # cargo clippy --all-targets --all-features -- -D warnings
just fmt       # cargo fmt --all

# Run all checks (format + clippy + test)
just all

# Tauri desktop app
just dev          # Dev mode with hot reload
just dev-two      # Two instances for P2P testing
just tauri-build  # Release bundle

# CLI (headless/debugging)
RUST_LOG=variance=debug cargo run --bin variance -- start

# Force protobuf rebuild
just proto
```

Frontend (in `app/`): managed with `pnpm`. Run `just frontend-install` once, then `just frontend-dev`.

## Workspace Structure

```
crates/
├── variance-proto/      # Protobuf schemas (foundation)
├── variance-p2p/        # libp2p core + protocol handlers
├── variance-identity/   # DID & identity (uses IPFS/IPNS)
├── variance-messaging/  # Chat (Direct + GossipSub/MLS)
├── variance-media/      # WebRTC signaling
├── variance-app/        # HTTP API & state (axum)
├── variance-relay/      # Standalone relay server binary
└── variance-cli/        # Standalone CLI (headless/debugging only)
app/
├── src/                 # React/TypeScript UI
└── src-tauri/           # Tauri desktop host (workspace member: variance-desktop)
```

**Primary runtime:** The Tauri desktop app (`variance-desktop`) embeds `variance-app` in-process — no sidecar process. The React frontend communicates via Tauri commands (FFI). `variance-cli` is for headless operation and debugging only.

**Dependency flow:** cli → app → p2p → (identity, messaging, media) → proto
*(app also depends directly on identity, messaging, media for HTTP API handlers)*

## Key Architecture: NodeCommand / EventChannels

The P2P node runs in its own task and is not `Send`/`Sync`. Communication happens through two channel types:

- **`NodeCommand`** (tokio `mpsc`): app layer → swarm. E.g., `SendDirectMessage`, `PublishGroupMessage`, `BroadcastUsernameChange`. Commands use `oneshot` channels for responses.
- **`EventChannels`** (tokio `broadcast`): swarm → app layer. Events like `IdentityEvent`, `SignalingEvent`, `RenameEvent`, `OfflineMessageEvent`.
- **`EventRouter`** (`crates/variance-app/src/event_router.rs`): subscribes to all `EventChannels` at startup and forwards events to WebSocket clients via `WebSocketManager`. This is where inbound P2P events become frontend-visible state changes.

When adding a new P2P feature:
1. Add the `NodeCommand` variant in `crates/variance-p2p/src/commands.rs`
2. Add the `Event` variant in `crates/variance-p2p/src/events.rs`
3. Handle both in the swarm loop (or a protocols handler)
4. Subscribe to the event in `EventRouter` and forward to `WsMessage`

## Message Storage

`LocalMessageStorage` (sled-backed) stores:
- Direct messages: keyed as `nonce || AES-256-GCM ciphertext` (locally encrypted)
- Group messages: same pattern
- Offline relay queue: `OfflineMessageEnvelope` protobuf, 30-day TTL
- Group metadata and MLS provider state

Messages use ULID for IDs (lexicographically sortable by time). Pagination is cursor-based via `before: Option<i64>` (timestamp ms) for direct messages.

MLS group state is serialized and persisted in sled under `local_did` after every MLS operation.

## Code Standards

### TODOs and Placeholders

- **No TODOs** unless:
  - Required by current task
  - Needed for later work (with specific reason)
  - Document reason when added

### Implementation Quality

- **No shortcuts or simplifications** without explicit reasons
- When simplifying, document:
  - Why the simplification is made
  - What the "correct" approach would be
  - When it should be revisited
- Prefer correct implementation over quick placeholder

### Code Comments

- **Minimal summaries** unless user requests detail
- Focus comments on "why" not "what"
- Architecture decisions belong in docs, not code comments

### Default Values

- **Inline constant defaults** directly in `Default` trait implementations
- **Only use helper functions** when defaults require logic, computation, or take arguments
- Constants belong inline; logic belongs in functions
- Examples:
  - ✅ Good: `port: 8080` (constant, inline it)
  - ❌ Bad: `port: default_port()` where `fn default_port() -> u16 { 8080 }` (no logic, inline it)
  - ✅ Good: `id: Uuid::new_v4()` (requires computation, inline it)
  - ✅ Good: `timestamp: SystemTime::now()` (requires syscall, inline it)
  - ✅ Good: `config: load_from_env()` where logic is complex (logic, use function)
  - ❌ Bad: `addrs: default_addrs()` where `fn default_addrs() -> Vec<String> { vec![...] }` (constant list, inline it)

### Testing Requirements

- **All features require tests** - unit tests and/or integration tests
- Test what matters: behavior, edge cases, error paths
- Keep tests focused and maintainable
- **Do not test generated code** - protobuf compilation output is prost-build's responsibility, not ours

### Error Handling

Use `snafu` for library crates, structured errors:

```rust
use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Failed to resolve {username}: {source}"))]
    Resolution {
        username: String,
        source: Box<dyn std::error::Error>,
    },
}
```

Use `anyhow` only for application binaries (variance-app, variance-cli).

## Architecture Principles

### DHT Usage

DHT is for peer/content routing only:
- Provider records: "who has X?"
- Peer discovery
- Content routing

### Identity System

- Store DID documents in **IPFS** (immutable, content-addressed)
- Use **IPNS** for mutable pointers to latest DID
- DHT stores provider records only
- Custom libp2p protocol for direct peer queries
- Multi-layer cache (memory → disk → network)

### Message Storage

- Direct messages: vodozemac Olm (Double Ratchet) — session init uses PreKey messages; follow-up messages use Normal type
- Group messages: GossipSub with OpenMLS (RFC 9420) — per-message forward secrecy, post-compromise security
- Offline messages: **Relay nodes** (`variance-relay` binary) with local DB
- TTL: 30 days on relay storage

### Protobuf

All P2P communication uses Protocol Buffers (defined in `crates/variance-proto/proto/`):
- `identity.proto` - DID, resolution protocol, `UsernameChanged`
- `messaging.proto` - Direct/group messages, receipts
- `media.proto` - WebRTC signaling

Auto-generated via `prost-build` in build.rs.

## References

- [libp2p specs](https://github.com/libp2p/specs)
- [IPFS concepts](https://docs.ipfs.tech/concepts/)
- [Kademlia paper](https://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf)
- [W3C DID spec](https://w3c-ccg.github.io/did-spec/)
