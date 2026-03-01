# Variance Documentation

## Quick Start

**Architecture Overview:**
- DHT for peer discovery (provider records)
- IPFS/IPNS for identity documents (implemented, untested)
- DHT provider records for username discovery (implemented, untested)
- Custom libp2p protocols for queries
- Protobuf for all P2P communication

**Recent Progress (2026-02-20):**
- ✅ Real-time message delivery — WebSocket inbound tick drives component `refetch()`; no DID string matching
- ✅ Cursor-based pagination — `?before=<timestamp_ms>`, newest-first sled scan, scroll-to-top `IntersectionObserver`
- ✅ TOCTOU race fix — `session_init_lock: Mutex<()>` in `DirectMessageHandler` prevents concurrent session overwrites
- ✅ Conversation switching — `key={activePeerDid}` + `refetchOnMount: "always"` eliminates stale message cache
- ✅ Large enum boxing — `Box<IdentityRequest>`, `Box<IdentityResponse>`, `Box<DirectMessage>` in P2P event variants

**Previously (2026-02-17):**
- ✅ vodozemac 0.9 (replaces double-ratchet-2) — Olm Double Ratchet for DMs
- ✅ OpenMLS (RFC 9420) — replaces hand-rolled AES-256-GCM group crypto; per-message forward secrecy + post-compromise security
- ✅ Complete messaging: receipts, typing indicators, sled storage
- ✅ Full HTTP REST API — all endpoints implemented
- ✅ WebSocket event delivery for Tauri frontend
- ✅ Tauri desktop app — onboarding, conversations, messages UI
- ✅ 232 tests passing across all crates
- 🚧 IPFS/IPNS + DHT integration testing (implemented, need live daemon)
- 🚧 WebRTC peer connection (signaling done, media stream pending)

## Documentation Index

1. **[QUICK-REFERENCE.md](./QUICK-REFERENCE.md)**
   - Workspace structure
   - Crate responsibilities
   - Key design patterns
   - Event channel system

2. **[PROTOCOL-GUIDE.md](./PROTOCOL-GUIDE.md)**
   - Protobuf schemas explained
   - How data flows between components
   - IPC between Tauri app and P2P backend

## Architecture

### Identity Resolution

```rust
// 1. Store DID document in IPFS
let cid = ipfs.add(&did_doc).await?;

// 2. Publish IPNS pointer (mutable)
ipfs.name_publish(&cid, &keypair).await?;

// 3. DHT stores "who has alice#0001?" (provider records)
dht.provide(ipns_key).await?;

// 4. Query via custom protocol (cached)
let response = identity_protocol.request(peer, IdentityRequest {
    username: "alice#0001"
}).await?;
```

### Message Storage

```rust
// Relay nodes with local storage
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

1. Explore protobuf schemas in `crates/variance-proto/proto/`
2. Start with `variance-p2p` crate (libp2p foundation)
3. Build up through identity → messaging → media
