# Quick Reference

## Workspace Structure

```
variance/
├── crates/
│   ├── variance-proto/          # Protobuf schemas (foundation)
│   │   ├── proto/
│   │   │   ├── identity.proto   # DID, identity resolution
│   │   │   ├── messaging.proto  # Chat messages, groups
│   │   │   └── media.proto      # WebRTC signaling
│   │   └── build.rs             # Codegen via prost-build
│   │
│   ├── variance-p2p/            # libp2p core (no app logic)
│   │   ├── behaviour.rs         # NetworkBehaviour composite
│   │   ├── node.rs              # Swarm management
│   │   └── protocols.rs         # Custom protocol handlers
│   │
│   ├── variance-identity/       # DID & identity
│   │   ├── did.rs               # DID generation, IPNS
│   │   ├── cache.rs             # Multi-layer caching
│   │   ├── protocol.rs          # Identity request/response
│   │   └── username.rs          # Registration, lookup
│   │
│   ├── variance-messaging/      # Chat system
│   │   ├── direct.rs            # 1-on-1 (Double Ratchet)
│   │   ├── group.rs             # GossipSub groups
│   │   ├── offline.rs           # Relay integration
│   │   └── storage.rs           # Local persistence (sled)
│   │
│   ├── variance-media/          # WebRTC
│   │   └── signaling.rs         # SDP/ICE exchange via libp2p
│   │
│   ├── variance-app/            # Application logic
│   │   ├── api.rs               # HTTP routes (axum)
│   │   ├── state.rs             # App state management
│   │   └── config.rs            # TOML config loading
│   │
│   └── variance-cli/            # Binary
│       └── main.rs              # CLI entry point
│
├── docs/
│   ├── ARCHITECTURE-CORRECTIONS.md  # Read this first!
│   ├── QUICK-REFERENCE.md           # This file
│   └── PROTOCOL-GUIDE.md            # Protobuf usage
│
└── Cargo.toml                   # Workspace manifest
```

## Crate Dependency Graph

```
variance-cli
    └── variance-app
            ├── variance-messaging
            │   └── variance-identity
            │       └── variance-p2p
            │           └── variance-proto
            ├── variance-media
            │   └── variance-p2p
            └── variance-identity
```

## Key Design Patterns

### 1. Protobuf for All P2P Communication

```rust
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};

// Send request
let request = IdentityRequest {
    query: Some(Query::Username(UsernameQuery {
        username: "alice".into(),
        discriminator: Some(1234),
        subnet_id: Some("public".into()),
    })),
    requester_did: Some(my_did.clone()),
    timestamp: now(),
};

// Serialize to bytes
let bytes = request.encode_to_vec();

// Send via libp2p stream
stream.write_all(&bytes).await?;
```

### 2. IPFS/IPNS for Identity

```rust
// Create DID document
let did_doc = DIDDocument {
    id: "did:peer:123...".into(),
    authentication: vec![...],
    key_agreement: vec![...],
    created_at: now(),
    updated_at: now(),
};

// Store in IPFS (immutable)
let cid = ipfs.add_json(&did_doc).await?;

// Publish IPNS (mutable pointer)
let ipns_key = keypair.public().to_peer_id();
ipfs.name_publish(&cid, &keypair).await?;

// Later: update identity
let new_cid = ipfs.add_json(&updated_did_doc).await?;
ipfs.name_publish(&new_cid, &keypair).await?;
```

### 3. Multi-Layer Caching

```rust
// L1: Hot cache (active conversations)
if let Some(identity) = l1_cache.get(&key) {
    return Ok(identity);
}

// L2: Warm cache (recent lookups)
if let Some(identity) = l2_cache.get(&key) {
    l1_cache.insert(key.clone(), identity.clone());
    return Ok(identity);
}

// L3: Disk cache (persistent)
if let Some(identity) = disk_cache.get(&key).await? {
    l2_cache.insert(key.clone(), identity.clone());
    return Ok(identity);
}

// L4: Network (DHT + custom protocol)
let identity = resolve_from_network(&key).await?;
disk_cache.insert(key.clone(), &identity).await?;
l2_cache.insert(key.clone(), identity.clone());
Ok(identity)
```

### 4. Custom libp2p Protocol

```rust
use libp2p::request_response::{self, ProtocolSupport};
use variance_proto::identity_proto::*;

// Define protocol
const IDENTITY_PROTOCOL: &str = "/variance/identity/1.0.0";

// Create request_response behaviour
let protocol = request_response::cbor::Behaviour::<
    IdentityRequest,
    IdentityResponse,
>::new(
    [(IDENTITY_PROTOCOL.into(), ProtocolSupport::Full)],
    Default::default(),
);

// Send request
let request_id = swarm
    .behaviour_mut()
    .identity_protocol
    .send_request(&peer_id, request);

// Handle response
match swarm.next().await {
    SwarmEvent::Behaviour(Event::IdentityProtocol(
        request_response::Event::Message {
            message: request_response::Message::Response {
                response, ..
            },
            ..
        }
    )) => {
        // Process response
    }
}
```

## Data Flow Examples

### Identity Lookup Flow

```
User searches "@alice#1234"
    ↓
[L1 Cache] → Hit? Return immediately
    ↓ Miss
[L2 Cache] → Hit? Promote to L1, return
    ↓ Miss
[L3 Disk Cache] → Hit? Promote to L2, return
    ↓ Miss
[DHT Provider Query] → "Who has alice#1234?"
    ↓
[Custom Protocol Request] → Ask peer directly
    ↓
[IPNS Resolution] → Get latest CID
    ↓
[IPFS Fetch] → Get DID document
    ↓
[Cache All Layers] → Store for future
    ↓
Return to user
```

### Direct Message Flow

```
Alice sends message to Bob
    ↓
[Double Ratchet Encrypt] → Forward secrecy
    ↓
[Resolve Bob's PeerID] → Via identity cache
    ↓
Bob online? → Yes: Direct libp2p stream
    ↓
Bob offline? → Store in relay node
    ↓
[Relay Storage] → Local DB, 30-day TTL
    ↓
Bob comes online
    ↓
[Query Relay] → "Any messages for me?"
    ↓
[Fetch & Decrypt] → Double Ratchet decrypt
    ↓
[Send ACK] → Delivery receipt
```

## Configuration Example

```toml
# config.toml
[p2p]
listen_addrs = [
    "/ip4/0.0.0.0/tcp/0",
    "/ip4/0.0.0.0/udp/0/quic-v1",
]

[p2p.bootstrap]
peers = [
    "/dns4/bootstrap.variance.network/tcp/4001/p2p/12D3Koo...",
]

[p2p.dht]
mode = "server"  # or "client"
protocol_prefix = "/variance/public"

[identity]
subnet_id = "public"
cache_size = 10000
cache_ttl_secs = 3600

[messaging]
enable_relay = true
relay_servers = [
    "/dns4/relay.variance.network/tcp/4001/p2p/12D3Koo...",
]

[api]
listen_addr = "127.0.0.1:3000"
cors_origins = ["tauri://localhost"]
```

## Common Tasks

### Generate Identity

```bash
cargo run --bin variance -- gen-identity --output identity.json
```

### Start Node

```bash
RUST_LOG=debug cargo run --bin variance -- start --config config.toml
```

### Test Specific Module

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_identity_cache() {
        let cache = IdentityCache::new(100);
        // ... test
    }
}
```

## Metrics to Track

```rust
// Cache performance
identity_cache_hit_rate: 0.85  // Target: >80%
identity_cache_size: 8234

// DHT performance
dht_lookup_duration_p95: 150ms  // Target: <200ms
dht_provider_records: 1523

// Message latency
message_delivery_p50: 45ms
message_delivery_p95: 180ms
```

## Error Handling with snafu

```rust
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to resolve identity for {username}: {source}"))]
    IdentityResolution {
        username: String,
        source: Box<dyn std::error::Error>,
    },

    #[snafu(display("Cache error: {message}"))]
    Cache { message: String },
}

// Usage
fn resolve(username: &str) -> Result<Identity> {
    cache
        .get(username)
        .context(IdentityResolutionSnafu { username })?
}
```
