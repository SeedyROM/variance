# Variance Project Instructions

## Project Context

Variance is a P2P Discord alternative in Rust, correcting architectural flaws from the original Go implementation in `../variance-go/`. The Go design misused Kademlia DHT as a database; this implementation uses IPFS/IPNS for identity storage, custom libp2p protocols for queries, and multi-layer caching.

**Key architectural documents:**
- `docs/ARCHITECTURE-CORRECTIONS.md` - Critical: explains what was wrong and correct approach
- `docs/QUICK-REFERENCE.md` - Workspace structure and patterns
- `docs/PROTOCOL-GUIDE.md` - Protobuf usage

**Reference (flawed):** `../variance-go/docs/` - Original design docs, contains DHT misuse

## Technology Stack

- **Rust 2021**, latest stable dependencies (not training data versions)
- **libp2p 0.55+** - Kademlia DHT, GossipSub, custom protocols
- **Protocol Buffers** (prost) - All P2P communication
- **Tokio** - Async runtime
- **Axum 0.8** - HTTP API for Tauri frontend
- **snafu** - Error handling (preferred over anyhow for libraries)
- **tracing/tracing-subscriber** - Structured logging

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
- No new feature should be untested
- No lengthy documentation as substitute for tests
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

### DHT Usage (Critical)

**NEVER use DHT for data storage.** DHT is for peer/content routing only.

**Correct:**
- Provider records: "who has X?"
- Peer discovery
- Content routing

**Wrong:**
- Storing DID documents directly
- Storing messages
- Storing user data

### Identity System

- Store DID documents in **IPFS** (immutable, content-addressed)
- Use **IPNS** for mutable pointers to latest DID
- DHT stores provider records only
- Custom libp2p protocol for direct peer queries
- Multi-layer cache (memory → disk → network)

### Message Storage

- Direct messages: libp2p streams with Double Ratchet
- Group messages: GossipSub with AES-256-GCM group keys
- Offline messages: **Relay nodes** with local DB, NOT DHT
- TTL: 30 days on relay storage

### Protobuf

All P2P communication uses Protocol Buffers (defined in `crates/variance-proto/proto/`):
- `identity.proto` - DID, resolution protocol
- `messaging.proto` - Direct/group messages, receipts
- `media.proto` - WebRTC signaling

Auto-generated via `prost-build` in build.rs.

## Workspace Structure

```
crates/
├── variance-proto/      # Protobuf schemas (foundation)
├── variance-p2p/        # libp2p core
├── variance-identity/   # DID & identity (uses IPFS/IPNS)
├── variance-messaging/  # Chat (Direct + GossipSub)
├── variance-media/      # WebRTC signaling
├── variance-app/        # HTTP API & state (axum)
└── variance-cli/        # Binary entry point
```

**Dependency flow:** cli → app → (messaging, media, identity) → p2p → proto

## Common Patterns

### Multi-Layer Caching

```rust
// L1: Hot (5 min) → L2: Warm (1 hour) → L3: Disk (24 hour) → Network
if let Some(v) = l1.get(key) { return Ok(v); }
if let Some(v) = l2.get(key) { l1.insert(key, v.clone()); return Ok(v); }
// ... continue down layers
```

### Custom libp2p Protocols

Use `request_response::cbor::Behaviour` with protobuf types:

```rust
const PROTOCOL: &str = "/variance/identity/1.0.0";
let protocol = request_response::cbor::Behaviour::<Request, Response>::new(...);
```

### IPFS/IPNS Flow

```rust
// Store identity
let cid = ipfs.add_json(&did_doc).await?;
ipfs.name_publish(&cid, &keypair).await?;

// Update identity
let new_cid = ipfs.add_json(&updated_doc).await?;
ipfs.name_publish(&new_cid, &keypair).await?;  // Same key, new CID
```

## Development Workflow

```bash
# Build with protobuf codegen
cargo build

# Check specific crate
cargo check -p variance-identity

# Run with debug logs
RUST_LOG=variance=debug cargo run --bin variance -- start
```

## Key Differences from Go Implementation

| Aspect | Go (Wrong) | Rust (Correct) |
|--------|-----------|----------------|
| Identity storage | DHT values | IPFS + IPNS |
| Offline messages | DHT values | Relay nodes + local DB |
| Username lookup | Brute-force 9999 DHT queries | Provider records + custom protocol |
| Serialization | JSON | Protocol Buffers |
| Caching | None | Multi-layer (memory + disk) |

## References

- [libp2p specs](https://github.com/libp2p/specs)
- [IPFS concepts](https://docs.ipfs.tech/concepts/)
- [Kademlia paper](https://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf)
- [W3C DID spec](https://w3c-ccg.github.io/did-spec/)
