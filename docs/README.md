# Variance Documentation

## Quick Start

This Rust implementation corrects critical architectural flaws from the Go design in [variance-go](https://github.com/SeedyROM/variance-go).

**TL;DR Changes:**
- ✅ DHT for peer discovery only (not data storage)
- ✅ IPFS/IPNS for identity documents (implemented, untested)
- ✅ DHT provider records for username discovery (implemented, untested)
- ✅ Custom libp2p protocols for queries
- ✅ Protobuf for all P2P communication

**Recent Progress (2026-02-20):**
- ✅ Real-time message delivery — WebSocket inbound tick drives component `refetch()`; no DID string matching
- ✅ Cursor-based pagination — `?before=<timestamp_ms>`, newest-first sled scan, scroll-to-top `IntersectionObserver`
- ✅ TOCTOU race fix — `session_init_lock: Mutex<()>` in `DirectMessageHandler` prevents concurrent session overwrites
- ✅ Conversation switching — `key={activePeerDid}` + `refetchOnMount: "always"` eliminates stale message cache
- ✅ Large enum boxing — `Box<IdentityRequest>`, `Box<IdentityResponse>`, `Box<DirectMessage>` in P2P event variants

**Previously (2026-02-17):**
- ✅ vodozemac 0.9 (replaces double-ratchet-2) — Olm Double Ratchet for DMs
- ✅ Complete messaging: receipts, typing indicators, sled storage
- ✅ Full HTTP REST API — all endpoints implemented
- ✅ WebSocket event delivery for Tauri frontend
- ✅ Tauri desktop app — onboarding, conversations, messages UI
- ✅ 232 tests passing across all crates
- 🚧 IPFS/IPNS + DHT integration testing (implemented, need live daemon)
- 🚧 WebRTC peer connection (signaling done, media stream pending)

## Documentation Index

1. **[ARCHITECTURE-CORRECTIONS.md](./ARCHITECTURE-CORRECTIONS.md)** ⭐ **Start here**
   - Why the Go design was wrong
   - Correct approaches (IPFS/IPNS, custom protocols, caching)
   - Implementation checklist

2. **[QUICK-REFERENCE.md](./QUICK-REFERENCE.md)**
   - Workspace structure
   - Crate responsibilities
   - Key design patterns
   - Event channel system (NEW)

3. **[PROTOCOL-GUIDE.md](./PROTOCOL-GUIDE.md)**
   - Protobuf schemas explained
   - How data flows between components
   - IPC between Tauri app and P2P backend

4. **[CHANGELOG.md](./CHANGELOG.md)** 🆕
   - Recent implementation progress
   - Protocol handlers & event system (2026-02-15)
   - Breaking changes and migration guides

## Original Go Docs (Reference Only)

See [variance-go](https://github.com/SeedyROM/variance-go) for the original design:

| Doc | Status | Notes |
|-----|--------|-------|
| 01-SYSTEM-OVERVIEW.md | ✅ Still valid | High-level architecture is correct |
| 02-TECHNOLOGY-CHOICES.md | ⚠️ Partially valid | Go→Rust, but principles apply |
| 03-IDENTITY-SYSTEM.md | ❌ **Flawed** | DHT misuse - see ARCHITECTURE-CORRECTIONS |
| 04-MESSAGING-ARCHITECTURE.md | ⚠️ Partially valid | Concepts OK, DHT usage wrong |
| 05-MEDIA-ARCHITECTURE.md | ✅ Still valid | WebRTC approach is sound |
| 06-DEPLOYMENT-OPERATIONS.md | ✅ Still valid | Infrastructure needs unchanged |
| 07-IMPLEMENTATION-PHASES.md | ⚠️ Reference | Phases differ due to architecture changes |
| 08-SUBNETS-INDEXING-CACHING.md | ✅ Still valid | Good ideas, now implemented correctly |

## What Changed and Why

### Identity Resolution

**Go Design (Wrong):**
```go
// Storing directly in DHT - BAD!
dht.PutValue(ctx, "username:alice:0001", []byte(did))
```

**Rust Design (Correct):**
```rust
// 1. Store DID document in IPFS
let cid = ipfs.add(&did_doc).await?;

// 2. Publish IPNS pointer (mutable)
ipfs.name_publish(&cid, &keypair).await?;

// 3. DHT only stores "who has alice#0001?" (provider records)
dht.provide(ipns_key).await?;

// 4. Query via custom protocol (cached)
let response = identity_protocol.request(peer, IdentityRequest {
    username: "alice#0001"
}).await?;
```

### Message Storage

**Go Design (Wrong):**
```go
// Offline messages in DHT - BAD!
dht.PutValue(ctx, "inbox:bob:msg-123", encryptedMsg)
```

**Rust Design (Correct):**
```rust
// Use relay nodes with local storage
relay.store_offline_message(OfflineMessageEnvelope {
    recipient_did,
    message,
    expires_at: now() + 30.days(),
}).await?;
```

## Development Workflow

```bash
# Build everything
cargo build

# Run node
cargo run --bin variance -- start

# Check specific crate
cargo check -p variance-identity

# Run tests
cargo test

# Generate protobuf code (automatic on build)
cargo build -p variance-proto
```

## Next Steps

1. Read [ARCHITECTURE-CORRECTIONS.md](./ARCHITECTURE-CORRECTIONS.md)
2. Explore protobuf schemas in `crates/variance-proto/proto/`
3. Start with `variance-p2p` crate (libp2p foundation)
4. Build up through identity → messaging → media
