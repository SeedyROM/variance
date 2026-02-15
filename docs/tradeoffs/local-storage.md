# Identity Storage Strategy

## Problem

The Variance architecture calls for IPFS/IPNS for identity storage:
- **IPFS**: Content-addressed, immutable storage for DID documents
- **IPNS**: Mutable pointers to latest DID version (like DNS)
- **DHT**: Provider records for peer discovery

However, requiring an IPFS daemon for **local development and testing** creates friction:
- External dependency (IPFS daemon must be running)
- Additional setup complexity
- Harder to test multi-node scenarios locally

## Solution: Storage Abstraction

We've implemented a **storage backend abstraction** that allows swapping between local and production storage:

```rust
#[async_trait]
pub trait IdentityStorage: Send + Sync {
    async fn store(&self, did: &Did) -> Result<String>;
    async fn fetch(&self, id: &str) -> Result<Option<Did>>;
    async fn publish(&self, name: &str, content_id: &str) -> Result<()>;
    async fn resolve(&self, name: &str) -> Result<Option<String>>;
}
```

### LocalStorage (Development/Testing)

Uses **sled** (embedded key-value store) to simulate IPFS/IPNS semantics:

- **Content addressing**: SHA-256 hash of DID document → "local:..." CID
- **Immutable storage**: Content stored by CID, never modified
- **Mutable pointers**: Name → CID mapping (like IPNS)
- **No external deps**: Pure Rust, embedded database

**Trade-offs:**
- ✅ No IPFS daemon needed
- ✅ Persistence across restarts
- ✅ Fast local testing
- ⚠️ No global namespace (each node has own DB)
- ⚠️ No content replication
- ⚠️ "CIDs" won't match real IPFS CIDs

These trade-offs are **acceptable for local testing** because:
1. Cross-node resolution uses the libp2p protocol anyway
2. DHT provider records still work for discovery
3. The storage interface is identical to production

### IpfsStorage (Production)

TODO: Implement when ready for production deployment.

Will integrate with real IPFS daemon:
- **Real CIDs**: Content-addressed via IPFS
- **Global namespace**: Content replicated across IPFS network
- **IPNS support**: Mutable name resolution
- **Pinning**: Keep important content available

## Configuration

Configured via `IdentityConfig`:

```toml
# Development
[identity.storage]
backend = "local"
db_path = "~/.variance/identity/storage"

# Production (future)
[identity.storage]
backend = "ipfs"
api_url = "http://localhost:5001"
gateway_url = "http://localhost:8080"
```

## Local Testing Workflow

**Running 2 nodes locally:**

```bash
# Terminal 1: Node A
variance start --config node_a.toml
# Uses LocalStorage at ~/.variance/node_a/storage.db

# Terminal 2: Node B
variance start --config node_b.toml
# Uses LocalStorage at ~/.variance/node_b/storage.db
```

**How resolution works:**

1. **Node A** creates DID → stores in LocalStorage → gets "fake CID"
2. **Node A** publishes DHT provider record: `"alice"` → Node A's PeerID
3. **Node B** searches DHT for `"alice"` → finds Node A's PeerID
4. **Node B** sends `IdentityRequest` to Node A via libp2p protocol
5. **Node A** responds with DID document from LocalStorage
6. **Node B** caches it in multi-layer cache

**Key insight:** The libp2p protocol handles cross-node resolution, so each node's LocalStorage only needs to serve its own DIDs.

## Migration Path to IPFS

When ready for production:

1. Implement `IpfsStorage` struct
2. Add IPFS client library dependency
3. Implement the `IdentityStorage` trait for `IpfsStorage`
4. Change config: `backend = "ipfs"`
5. No code changes needed elsewhere (abstraction handles it)

The rest of the system is **completely agnostic** to which backend is used.

## Testing

LocalStorage includes comprehensive tests:
- Content-addressed storage (same content → same CID)
- Mutable name resolution (IPNS-like)
- Updates (new version → new CID, name points to latest)
- Missing content/names (returns None)

Run tests:
```bash
cargo test -p variance-identity storage
```

## References

- `crates/variance-identity/src/storage.rs` - Storage trait and implementations
- `crates/variance-identity/src/config.rs` - Configuration structures
- `docs/ARCHITECTURE-CORRECTIONS.md` - Why we don't use DHT for storage
