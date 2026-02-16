# Variance Changelog

## 2026-02-15 - Protocol Handlers & Event System

### ✅ Completed: Custom Protocol Business Logic Integration

Successfully wired up all three custom libp2p protocols with full business logic handlers and event channel system.

#### 1. Protocol Handlers Implementation

**Location:** `crates/variance-p2p/src/handlers/`

Created business logic handlers that connect libp2p protocol events to actual functionality:

- **`identity.rs`** - Identity resolution handler
  - Resolves DIDs and usernames via in-memory cache
  - Handles three query types: DID, Username, PeerId
  - Returns proper success/not-found/error responses
  - TODO: IPFS/IPNS integration for persistent storage

- **`offline.rs`** - Offline message relay handler
  - Wraps `OfflineRelayHandler` from `variance-messaging`
  - Uses local sled storage for message persistence
  - Supports fetch requests with pagination
  - 30-day TTL for offline messages

- **`signaling.rs`** - WebRTC signaling handler
  - Handles Offer, Answer, ICE Candidates, Control messages
  - Tracks active calls in memory
  - Delegates to `SignalingHandler` from `variance-media`
  - Signature verification (TODO: integrate with identity system)

#### 2. Event Channel System

**Location:** `crates/variance-p2p/src/events.rs`

Implemented broadcast-based event system for all protocol events:

```rust
pub struct EventChannels {
    pub identity: tokio::sync::broadcast::Sender<IdentityEvent>,
    pub offline_messages: tokio::sync::broadcast::Sender<OfflineMessageEvent>,
    pub signaling: tokio::sync::broadcast::Sender<SignalingEvent>,
}
```

**Features:**
- Multiple subscribers supported (broadcast channels)
- Isolated channels prevent cross-contamination
- Application layer can subscribe to protocol events
- Events emitted automatically when protocols receive messages

**Event Types:**
- `IdentityEvent`: RequestReceived, ResponseReceived, DidCached
- `OfflineMessageEvent`: FetchRequested, MessagesReceived, MessageStored
- `SignalingEvent`: OfferReceived, AnswerReceived, IceCandidateReceived, ControlReceived, CallEnded

#### 3. Node Integration

**Location:** `crates/variance-p2p/src/node.rs`

Updated Node to:
- Initialize protocol handlers on construction
- Wire handlers into event loop
- Emit events when protocol messages are received/sent
- Expose event channels via `node.events()`

**Example Usage:**
```rust
let node = Node::new(config)?;

// Subscribe to identity events
let mut rx = node.events().subscribe_identity();

// React to events
while let Ok(event) = rx.recv().await {
    match event {
        IdentityEvent::DidCached { did } => {
            println!("Cached DID: {}", did);
        }
        _ => {}
    }
}
```

#### 4. Testing

**Unit Tests:** `crates/variance-p2p/src/handlers/*.rs`
- Test handler creation and basic functionality
- Test request/response handling
- Test error cases

**Integration Tests:** `crates/variance-p2p/tests/integration_tests.rs`
- Test full event flow for all protocols
- Test event isolation (channels don't cross-contaminate)
- Test multi-subscriber behavior
- Test call lifecycle simulation

**Test Results:** ✅ 36/36 tests passing (28 unit + 8 integration)

#### 5. Architecture Changes

**Fixed Circular Dependencies:**
- Removed `variance-p2p` dependency from:
  - `variance-identity`
  - `variance-messaging`
  - `variance-media`
- Protocol implementations now live in `variance-p2p` (correct)
- Business logic crates depend on `variance-proto` only

**Dependency Flow (Correct):**
```
variance-cli
  └── variance-app
      ├── variance-p2p (protocols + handlers)
      │   ├── variance-identity (business logic)
      │   ├── variance-messaging (business logic)
      │   └── variance-media (business logic)
      └── variance-proto (schema definitions)
```

**Configuration Updates:**
- Added `storage_path` to `Config` with smart default
- Uses `dirs::data_local_dir()` for platform-appropriate storage
- Tests use `tempfile` for isolated storage

**Workspace Dependencies Added:**
- `tempfile = "3.14"` (dev dependency)
- `ulid = "1.1"` (for message IDs)
- `chrono` (already present, now used in p2p)
- `dirs = "5.0"` (for storage path defaults)

#### 6. What's Next

**Immediate TODOs:**
1. **IPFS/IPNS Integration** - Identity handler currently uses in-memory cache
   - Add IPFS client to `variance-identity`
   - Implement `store_did()` and `resolve_did()` with IPFS
   - Update identity handler to use IPFS backend

2. **Call Manager Integration** - Signaling handler needs full WebRTC stack
   - Implement `CallManager` in `variance-media`
   - Wire signaling handler to call manager
   - Add call state persistence

3. **Message Delivery** - Offline messages need delivery mechanism
   - Add event subscriber in `variance-app`
   - Deliver messages to Tauri frontend via WebSocket
   - Implement read receipts and acknowledgments

4. **Public API** - Expose protocol functionality to application layer
   - Add methods to Node for sending identity requests
   - Add methods for fetching offline messages
   - Add methods for initiating calls

**Longer-term:**
- DHT provider records for username discovery
- Relay node selection and failover
- Message encryption/decryption integration
- Performance monitoring and metrics

#### 7. Files Changed

**New Files:**
- `crates/variance-p2p/src/handlers/mod.rs`
- `crates/variance-p2p/src/handlers/identity.rs`
- `crates/variance-p2p/src/handlers/offline.rs`
- `crates/variance-p2p/src/handlers/signaling.rs`
- `crates/variance-p2p/src/events.rs`
- `crates/variance-p2p/tests/integration_tests.rs`
- `docs/CHANGELOG.md` (this file)

**Modified Files:**
- `crates/variance-p2p/src/lib.rs` - Added exports
- `crates/variance-p2p/src/node.rs` - Added handlers and events
- `crates/variance-p2p/src/config.rs` - Added storage_path
- `crates/variance-p2p/src/error.rs` - Added InvalidMessage variant
- `crates/variance-p2p/Cargo.toml` - Added dependencies
- `crates/variance-identity/Cargo.toml` - Removed p2p dependency
- `crates/variance-identity/src/error.rs` - Removed P2p error variant
- `crates/variance-messaging/Cargo.toml` - Removed p2p dependency
- `crates/variance-media/Cargo.toml` - Removed p2p dependency
- `Cargo.toml` - Added workspace dependencies

#### 8. Breaking Changes

None - this is net-new functionality. The protocols were previously wired up but had no business logic.

#### 9. Migration Guide

If you were manually handling protocol events in your application:

**Before:**
```rust
// Manual event handling in application layer
match swarm_event {
    SwarmEvent::Behaviour(Event::Identity(identity_event)) => {
        // Handle manually
    }
}
```

**After:**
```rust
// Subscribe to typed events
let mut rx = node.events().subscribe_identity();
tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
        match event {
            IdentityEvent::ResponseReceived { response, .. } => {
                // Handle with full context
            }
            _ => {}
        }
    }
});
```

---

## Previous Work

See git history for protocol definitions, protobuf schemas, and initial P2P setup.
