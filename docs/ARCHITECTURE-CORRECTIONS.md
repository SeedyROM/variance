# Architecture Corrections: DHT, IPFS, and Protocol Design

## Critical Issues with Original Go Design

The original design documents make several architectural mistakes that would prevent the system from working correctly in production. This document outlines the issues and correct approaches.

---

## Problem 1: DHT is NOT a Database

### What the Go Docs Got Wrong

The original design treats Kademlia DHT like a key-value database:

```go
// WRONG: Storing username mappings in DHT
dht.PutValue(ctx, "username:alice:0001", []byte(did))

// WRONG: Storing offline messages in DHT
dht.PutValue(ctx, "inbox:bob-did:msg-123", encryptedMessage)

// WRONG: Storing DID documents in DHT
dht.PutValue(ctx, "did:peer:123...", didDocument)
```

### Why This Doesn't Work

1. **DHT is for Routing, Not Storage**
   - Kademlia DHT is designed for peer routing and content routing
   - It's a distributed index, not a distributed database
   - Values are replicated to k-closest nodes (typically k=20)
   - No guarantees about persistence

2. **Size Limitations**
   - DHT values are typically limited to 1-4 MB
   - Large DID documents or message histories won't fit
   - No pagination or chunking support

3. **TTL and Expiration**
   - DHT values expire (typical TTL: 24 hours to 7 days)
   - Nodes don't guarantee long-term storage
   - Values must be re-published regularly (DHT refresh)

4. **Mutable Data Problems**
   - DHT keys are content-addressed (hash-based)
   - Updating a value requires writing to a new key
   - No atomic updates or transactions
   - Race conditions on concurrent writes

5. **Performance Issues**
   - Every DHT query requires network round-trips (O(log n) hops)
   - Brute-forcing 9,999 discriminators = 9,999 network queries
   - No query batching or indexing

---

## Correct Approach 1: IPFS + IPNS for Identity

### IPFS (InterPlanetary File System)

**What it's for**: Storing immutable content-addressed data

```rust
// Store DID document in IPFS
let did_doc = DIDDocument { ... };
let cid = ipfs.add_json(&did_doc).await?;
// CID: bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi

// Retrieve DID document from IPFS
let did_doc: DIDDocument = ipfs.get_json(&cid).await?;
```

**Benefits**:
- Content-addressed (CID = hash of content)
- Deduplicated automatically
- Distributed storage across peers
- Works great for immutable data (published DID docs, media files)

**Limitations**:
- Immutable (can't update a CID)
- Need IPNS for mutable pointers

### IPNS (InterPlanetary Name System)

**What it's for**: Mutable pointers to IPFS content

```rust
// Publish mutable pointer to latest DID document
let ipns_key = keypair.public().to_peer_id();
ipfs.name_publish(&cid, &keypair).await?;
// IPNS name: /ipns/12D3KooWRBy97UB99e3J6hiPesre1MZeuNQvfan4gBziswrRJsNK

// Resolve IPNS name to latest CID
let cid = ipfs.name_resolve(&ipns_key).await?;
let did_doc: DIDDocument = ipfs.get_json(&cid).await?;
```

**Benefits**:
- Mutable (update by re-publishing with same key)
- Cryptographically signed (only key owner can update)
- Works perfectly for evolving identity documents

**Recommended Identity Flow**:

```
1. User generates DID and keypair
2. Create DID document → Store in IPFS → Get CID
3. Publish IPNS record: IPNS key → CID
4. Share IPNS name (derived from public key) as identity handle
5. To update DID (key rotation, new addresses):
   - Create new DID document → Store in IPFS → Get new CID
   - Re-publish IPNS: same key → new CID
   - Anyone resolving IPNS gets latest version
```

---

## Correct Approach 2: Custom libp2p Protocols

### What Custom Protocols Are For

Direct peer-to-peer request/response for identity resolution, without DHT overhead.

### Example: Identity Resolution Protocol

```rust
use libp2p::{request_response, StreamProtocol};

// Define protocol
const IDENTITY_PROTOCOL: StreamProtocol = StreamProtocol::new("/variance/identity/1.0.0");

#[derive(Debug, Clone)]
struct IdentityRequest {
    username: String,
    discriminator: u16,
}

#[derive(Debug, Clone)]
struct IdentityResponse {
    did: Option<String>,
    ipns_key: Option<String>,
    public_keys: HashMap<String, Vec<u8>>,
}

// Use request_response protocol
let identity_protocol = request_response::cbor::Behaviour::<
    IdentityRequest,
    IdentityResponse,
>::new(
    [(IDENTITY_PROTOCOL, ProtocolSupport::Full)],
    request_response::Config::default(),
);
```

### How It Works

```
1. Alice wants to find @bob#1234
2. Alice queries DHT for "who has bob#1234?" → Gets list of PeerIDs
3. Alice opens direct stream to one of those peers
4. Alice: "Give me identity for bob#1234"
5. Peer responds with DID, IPNS key, public keys
6. Alice caches result locally
7. Future lookups hit cache (no network)
```

### Benefits

- **Direct P2P**: No DHT overhead for every query
- **Efficient**: Single network round-trip
- **Cacheable**: Results can be cached locally
- **Flexible**: Can implement complex query patterns
- **Type-safe**: Using protobuf schemas

---

## Correct Approach 3: GossipSub for Updates

### What GossipSub Is For

Broadcasting updates to interested peers (pub/sub pattern).

### Example: Identity Update Broadcast

```rust
// Subscribe to identity updates topic
let topic = IdentTopic::new("/variance/identity-updates");
gossipsub.subscribe(&topic)?;

// When user updates their profile
let update = IdentityUpdate {
    did: my_did.clone(),
    ipns_key: my_ipns_key.clone(),
    updated_at: Utc::now().timestamp(),
    signature: sign(&update_data, &private_key),
};

// Broadcast to all subscribers (protobuf-encoded)
gossipsub.publish(topic.clone(), update.encode_to_vec())?;

// Subscribers receive update
match swarm.next().await {
    SwarmEvent::Behaviour(Event::Gossipsub(GossipsubEvent::Message {
        message, ..
    })) => {
        let update = IdentityUpdate::decode(&message.data[..])?;
        // Invalidate cache for this DID
        cache.remove(&update.did)?;
    }
}
```

### Benefits

- **Real-time**: Friends get updates immediately
- **Efficient**: One message → many subscribers
- **Scalable**: Epidemic broadcast (resilient to failures)
- **Selective**: Only subscribe to topics you care about

---

## Recommended Architecture

### Identity System

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: Local Cache (In-Memory + Disk)                     │
│ - 80%+ hit rate for friends and recent contacts             │
│ - TTL: 5 min (hot) / 1 hour (warm) / 24 hour (cold)         │
└───────────────────────────┬─────────────────────────────────┘
                            │ Cache miss
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 2: Custom libp2p Protocol (Direct P2P Query)          │
│ - Query known nodes via /variance/identity/1.0.0            │
│ - Single round-trip, typed request/response                 │
└───────────────────────────┬─────────────────────────────────┘
                            │ No peers available
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 3: DHT Provider Records (Who Has This Identity?)      │
│ - DHT stores: "alice#1234" → [PeerID1, PeerID2, ...]        │
│ - Use DHT for DISCOVERY, not STORAGE                        │
│ - Query peer directly after discovery                       │
└───────────────────────────┬─────────────────────────────────┘
                            │ Peer found
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 4: IPNS Resolution (Retrieve Latest Identity)         │
│ - Resolve IPNS key → CID                                    │
│ - Fetch DID document from IPFS                              │
│ - Cache result                                              │
└─────────────────────────────────────────────────────────────┘
```

### Message Storage (Offline Delivery)

```
DO NOT use DHT for message storage!

Option A: Relay Nodes (Recommended)
- User registers with relay node(s)
- Relay stores offline messages in local DB
- User fetches on reconnect
- Trust model: Relay can see metadata (not content, due to E2E encryption)

Option B: Friend Relay (Decentralized)
- Friends relay messages for each other
- Mutual aid model
- No single point of failure
- Privacy-preserving (friends already trusted)

Option C: IPFS Pinning Service
- Store message as IPFS object
- Pin to pinning service (Pinata, Web3.Storage)
- Publish CID via GossipSub or relay
- Receiver fetches from IPFS
```

---

## Protobuf Schema Design

All data structures shared between components should use Protocol Buffers.

### Why Protobuf?

1. **Language-Agnostic**: Works with Rust, Go, TypeScript, etc.
2. **Type-Safe**: Schema enforced at compile time
3. **Versioned**: Forward/backward compatibility
4. **Efficient**: Compact binary format
5. **Documented**: Schema is documentation

### Example: variance-proto Crate

```protobuf
syntax = "proto3";

package variance.identity.v1;

// DID Document
message DIDDocument {
  string id = 1;  // did:peer:...
  repeated VerificationMethod authentication = 2;
  repeated VerificationMethod key_agreement = 3;
  repeated Service service = 4;
  int64 created_at = 5;
  int64 updated_at = 6;
}

message VerificationMethod {
  string id = 1;
  string type = 2;
  string controller = 3;
  bytes public_key_multibase = 4;
}

message Service {
  string id = 1;
  string type = 2;
  string service_endpoint = 3;
}

// Identity Query Protocol
message IdentityRequest {
  oneof query {
    string username = 1;  // "alice#1234"
    string did = 2;       // "did:peer:..."
    string peer_id = 3;   // "12D3Koo..."
  }
}

message IdentityResponse {
  optional DIDDocument did_document = 1;
  optional string ipns_key = 2;
  repeated string multiaddrs = 3;
  int64 timestamp = 4;
}

// Gossipsub Messages
message IdentityUpdate {
  string did = 1;
  string ipns_key = 2;
  int64 updated_at = 3;
  bytes signature = 4;  // Ed25519 signature
}

message ChatMessage {
  string id = 1;  // ULID
  string sender_did = 2;
  string recipient_did = 3;
  bytes ciphertext = 4;
  bytes nonce = 5;
  bytes signature = 6;
  int64 timestamp = 7;
}
```

---

## Implementation Checklist

### Phase 0: Foundation ✅
- [x] Set up workspace with proto, p2p, identity, messaging, app crates
- [x] Define protobuf schemas in `variance-proto`
- [x] Generate Rust code from protobufs
- [x] Set up tracing and error handling

### Phase 1: Identity with IPFS/IPNS
- [x] Implement DID generation (Ed25519 keypair + BIP39 recovery)
- [x] Implement local identity cache (multi-layer: hot/warm/disk)
- [ ] 🚧 Integrate IPFS client (DIDs currently in-memory only)
- [ ] 🚧 Store DID documents in IPFS
- [ ] 🚧 Publish IPNS records for mutable identity

### Phase 2: Custom Identity Protocol ✅ (minus DHT provider records)
- [x] Define identity request/response protocol
- [x] Implement libp2p request_response behavior
- [x] Direct peer queries for identity resolution
- [ ] 🚧 DHT provider records for `@username#discriminator` discovery

### Phase 3: Messaging ✅
- [x] Direct messages: Olm Double Ratchet via vodozemac (PreKey + Normal)
- [x] Group messages: GossipSub with OpenMLS (RFC 9420) — per-message forward secrecy + post-compromise security
- [x] Offline messages: Relay node storage with 30-day TTL (NOT DHT)
- [x] Message persistence (sled KV, ULID-sorted, paginated)
- [x] Read receipts and delivery status
- [x] Typing indicators

### Phase 4: Application Layer ✅
- [x] Complete HTTP REST API (identity, messages, calls, signaling, receipts, typing)
- [x] WebSocket event delivery to Tauri frontend
- [x] Tauri desktop app (onboarding, conversations, messages)
- [x] CLI (identity generate/recover/show, config, start)

### Phase 5: Media
- [x] WebRTC signaling protocol (Offer/Answer/ICE/Control via libp2p)
- [x] Call state machine (Ringing → Connecting → Active → Ended)
- [ ] 🚧 WebRTC peer connection (actual media stream negotiation)
- [ ] 🚧 STUN/TURN server configuration

### Phase 6: Caching & Performance
- [x] Multi-layer cache (L1 hot 5min → L2 warm 1hr → L3 disk 24hr)
- [ ] 🚧 Network fallback layer (blocked on IPFS integration)
- [ ] 🚧 Cache invalidation via GossipSub identity updates
- [ ] Metrics and observability

---

## Key Takeaways

| What the Go Docs Said | Why It's Wrong | Correct Approach |
|----------------------|----------------|------------------|
| Store usernames in DHT | DHT is not a database | Use DHT for provider records, IPFS/IPNS for storage |
| Store offline messages in DHT | DHT has TTL, size limits | Use relay nodes with local DB |
| Query DHT for every lookup | Too slow, expensive | Layer caches, custom protocols, DHT for discovery only |
| Brute-force discriminators | O(9999) queries | Use indexing, provider records, or sequential assignment |
| JSON over libp2p streams | No schema, versioning issues | Use protobuf for all P2P communication |

---

## Resources

- [libp2p Specifications](https://github.com/libp2p/specs)
- [Kademlia DHT Paper](https://pdos.csail.mit.edu/~petar/papers/maymounkov-kademlia-lncs.pdf)
- [IPFS Concepts](https://docs.ipfs.tech/concepts/)
- [IPNS Specification](https://github.com/ipfs/specs/blob/main/ipns/IPNS.md)
- [Protocol Buffers Guide](https://protobuf.dev/programming-guides/proto3/)
- [rust-libp2p Examples](https://github.com/libp2p/rust-libp2p/tree/master/examples)
