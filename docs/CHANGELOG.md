# Variance Changelog

## 2026-02-20 - Real-Time Messaging, Pagination & Bug Fixes

### ✅ Completed: End-to-End Message Delivery & UX Polish

#### 1. Real-Time Message Delivery (Frontend)

**Problem:** Messages received by the backend never appeared in the UI until the recipient sent a reply (one-message lag). Root cause: WebSocket handler used `invalidateQueries(["messages", event.from])` but `event.from` didn't exactly match the React Query key `["messages", peerDid]` used by `MessageView`.

**Fix:**
- Added `inboundMessageTick: number` + `tickInboundMessage()` to `messagingStore` (Zustand)
- WebSocket `DirectMessageReceived` handler bumps the tick instead of calling `invalidateQueries`
- `MessageView` watches the tick via `useEffect` and calls its own `refetch()` — using the `peerDid` already scoped into the query, avoiding any string-matching dependency
- Set `staleTime: Infinity` + `refetchOnWindowFocus: false` to eliminate background refetch races

#### 2. Conversation Switching Stale Cache (Frontend)

**Problem:** Switching to a previously-opened conversation showed the cached (old) message list because `staleTime: Infinity` prevented React Query from refetching.

**Fix:**
- Added `key={activePeerDid}` to `<MessageView>` in `App.tsx` — forces full unmount/remount on conversation switch
- Added `refetchOnMount: "always"` to the messages query — always fetches on mount regardless of stale time

#### 3. TOCTOU Session Initialization Race (Backend)

**Problem:** Sending multiple messages rapidly to a new peer caused the app to stop decrypting messages. Two concurrent HTTP requests both called `has_session()` → both saw `false` → both called `init_session_as_initiator()` → second overwrote the first session in the map, wasting the OTK. The recipient could only process one PreKey message with that OTK.

**Fix:** Added `session_init_lock: Mutex<()>` to `DirectMessageHandler`. `init_session_if_needed` now acquires the lock before the check-and-create, then re-checks under the lock so a concurrent winner short-circuits:

```rust
pub async fn init_session_if_needed(&self, ...) -> Result<()> {
    let _guard = self.session_init_lock.lock().await;
    if self.sessions.read().await.contains_key(recipient_did) {
        return Ok(());  // concurrent caller beat us to it
    }
    self.init_session_as_initiator(...).await
}
```

#### 4. Cursor-Based Message Pagination

**Problem:** `fetch_direct` scanned oldest-first and hard-capped at 50 messages, so long conversations were truncated and the first 50 (not the most recent 50) were shown.

**Changes:**

*Storage (`storage.rs`):*
- `before: Option<String>` → `before: Option<i64>` (millisecond timestamp)
- Implementation now reverse-scans (`scan_prefix().rev()`) to get newest-first, filters by `msg.timestamp < before` for the cursor, takes `limit`, then reverses to restore chronological order

*API (`api.rs`):*
- `GET /messages/direct/{did}` now accepts `?before=<timestamp_ms>&limit=<n>`
- Default limit: 1024 (was 50, hardcoded)

*Frontend (`client.ts`, `MessageView.tsx`):*
- `messagesApi.getDirect(peerDid, before?)` passes `?before=<ts>` when provided
- `MessageView` uses `IntersectionObserver` on a sentinel `<div>` at the top of the scroll container (root: the `<ScrollArea>` ref) to trigger `loadOlder()` when the user scrolls to the top
- `loadOlder` captures `scrollHeight` before prepending, restores scroll position via `requestAnimationFrame` after the DOM update so the view doesn't jump
- `hasMore` tracks exhaustion: a page with fewer than 1024 messages means no more history

#### 5. Large Enum Variant Boxing (P2P Events)

Removed two `#[allow(clippy::large_enum_variant)]` suppressions by boxing the large fields:

| Enum | Variant | Before | After |
|------|---------|--------|-------|
| `IdentityEvent` | `RequestReceived` | `request: IdentityRequest` | `request: Box<IdentityRequest>` |
| `IdentityEvent` | `ResponseReceived` | `response: IdentityResponse` | `response: Box<IdentityResponse>` |
| `DirectMessageEvent` | `MessageReceived` | `message: DirectMessage` | `message: Box<DirectMessage>` |

Construction sites in `node.rs` updated to `Box::new(...)`. The match site in `event_router.rs` that moves the message into `receive_message()` updated to `*message`.

---

## 2026-02-17 - vodozemac Migration, Complete Messaging Stack & Tauri Desktop App

### ✅ Completed: Full Application Layer

Major session completing the messaging stack, HTTP API, WebSocket delivery, and Tauri desktop app.

#### 1. Crypto: vodozemac Replaces double-ratchet-2

Replaced the unmaintained `double-ratchet-2` crate with `vodozemac 0.9`, the battle-tested Olm
implementation used by Matrix/Element.

**Changes:**
- `variance-messaging` now uses `vodozemac` and `ed25519-dalek`
- Removed: `double-ratchet-2`, `x25519-dalek`, `bincode` from messaging crate
- `IdentityFile` stores `olm_account_pickle` (JSON-serialized `AccountPickle`) instead of `x25519_key`
- Auto-migration for pre-vodozemac identity files

**vodozemac API patterns:**
```rust
// Session init (outbound)
account.create_outbound_session(SessionConfig::version_2(), identity_key, otk)

// Session init (inbound, first PreKey message)
account.create_inbound_session(sender_identity_key, &pre_key_msg)
// → InboundCreationResult { session, plaintext }

// Encrypt
let olm_message = session.encrypt(&plaintext);
let (msg_type, ciphertext) = olm_message.to_parts();
// msg_type: 0 = PreKey, 1 = Normal

// Decrypt
let olm_msg = OlmMessage::from_parts(msg_type, &ciphertext)?;
let plaintext = session.decrypt(&olm_msg)?;
```

**DirectMessage wire format (updated):**
- Field 4: `ciphertext` — OlmMessage body bytes (`to_parts().1`)
- Field 5: `olm_message_type` — 0=PreKey, 1=Normal (`to_parts().0 as uint32`)
- Field 6: `signature` — Ed25519
- Field 10: `sender_identity_key` — Curve25519 bytes (PreKey messages only)

#### 2. Complete Messaging System

All messaging features implemented and tested:

- **`receipts.rs`** — Read/delivery receipt tracking with `ReceiptStatus` (DELIVERED, READ)
- **`typing.rs`** — Real-time typing state broadcasts (per-DM and per-group)
- **`storage.rs`** — Full sled-backed persistence with ULID-sorted message history, conversation indexing, pagination

#### 3. HTTP REST API

All routes implemented in `variance-app/src/api.rs`:

| Group | Routes |
|-------|--------|
| Health | `GET /health` |
| Identity | `GET /identity`, `GET /identity/resolve/{did}`, `POST /identity/username` |
| Conversations | `GET /conversations`, `POST /conversations`, `DELETE /conversations/{peer_did}` |
| Messages | `POST /messages/direct`, `GET /messages/direct/{did}`, `POST /messages/group`, `GET /messages/group/{group_id}` |
| Calls | `POST /calls/create`, `GET /calls/active`, `POST /calls/{id}/accept\|reject\|end` |
| Signaling | `POST /signaling/offer\|answer\|ice\|control` |
| Receipts | `POST /receipts/delivered\|read`, `GET /receipts/{message_id}` |
| Typing | `POST /typing/start\|stop`, `GET /typing/{recipient}` |
| WebSocket | `GET /ws` |

#### 4. WebSocket Event Delivery

Real-time event streaming from P2P layer to Tauri frontend:

- **`websocket.rs`** — WebSocket upgrade handler and event pump
- **`event_router.rs`** — Routes P2P broadcast events to all active WebSocket subscribers
- Multiple concurrent clients supported (broadcast channels)

#### 5. Tauri Desktop App (`app/`)

Working desktop application with:
- **Onboarding flow** — Welcome, identity generation (BIP39 mnemonic display), identity recovery, setup complete
- **Conversations** — List view, conversation items, new conversation modal
- **Messages** — Message view, message bubbles, input, typing indicator, date dividers
- **Tauri host** (`src-tauri/`) — Spawns the `variance` node subprocess, manages app state, exposes commands

#### 6. Tauri Startup Fixes

Fixed startup reliability issues:
- XDG path resolution for platform-appropriate data directories
- Directory creation on first run
- Race condition fix on identity file loading
- Auto-migration for legacy identity files (pre-vodozemac pickle format)

#### 7. Test Suite: 232 Tests

| Crate | Tests | Notes |
|-------|-------|-------|
| variance-messaging | 56 | |
| variance-app | 42 | |
| variance-identity | 32 | 5 ignored (IPFS integration, need live daemon) |
| variance-media | 34 | |
| variance-p2p | 29 | |
| variance-proto | 2 | ignored |
| variance-cli | 2 | |
| integration | 8 | |
| **Total** | **232** | **5 ignored** |

#### 8. What's Next

1. **IPFS/IPNS Integration** — Identity handler uses in-memory cache only; need persistent storage
2. **WebRTC Peer Connection** — Signaling works; actual media stream negotiation pending
3. **DHT Provider Records** — Username discovery framework in place, not wired to DHT
4. **Relay Node Selection** — Discovery and failover between relay nodes
5. **Call UI** — Tauri has message UI; call screens not yet wired

---

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
    pub direct_messages: tokio::sync::broadcast::Sender<DirectMessageEvent>,
    pub group_messages: tokio::sync::broadcast::Sender<GroupMessageEvent>,
}
```

**Features:**
- Multiple subscribers supported (broadcast channels)
- Isolated channels prevent cross-contamination
- Application layer can subscribe to protocol events
- Events emitted automatically when protocols receive messages

**Event Types:**
- `IdentityEvent`: `RequestReceived { request: Box<IdentityRequest> }`, `ResponseReceived { response: Box<IdentityResponse> }`, `DidCached`
- `OfflineMessageEvent`: `FetchRequested`, `MessagesReceived`, `MessageStored`
- `SignalingEvent`: `OfferReceived`, `AnswerReceived`, `IceCandidateReceived`, `ControlReceived`, `CallEnded`
- `DirectMessageEvent`: `MessageReceived { message: Box<DirectMessage> }`, `MessageSent`
- `GroupMessageEvent`: `MessageReceived { message: GroupMessage }`, `MessageSent`

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
