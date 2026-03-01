# Architecture

Variance is a peer-to-peer chat application — a decentralized Discord alternative. There is no central server; every node is both client and server.

## System Overview

```
┌──────────────────────────────────────────────────────────────────┐
│  Tauri Desktop App (variance-desktop)                            │
│  ┌─────────────────────┐    ┌──────────────────────────────────┐ │
│  │  React / TypeScript │◄──►|  Tauri Commands (FFI)            │ │
│  │  (app/src/)         │    │  (app/src-tauri/)                │ │
│  └─────────────────────┘    └─────────┬────────────────────────┘ │
│                                       │ in-process               │
│                           ┌───────────▼───────────────────────┐  │
│                           │  variance-app (HTTP API + State)  │  │
│                           │  ┌────────┐ ┌──────────────────┐  │  │
│                           │  │ Axum   │ │ EventRouter      │  │  │
│                           │  │ Router │ │ (P2P → WebSocket)│  │  │
│                           │  └───┬────┘ └────────▲─────────┘  │  │
│                           └──────┼───────────────┼────────────┘  │
│                    NodeCommand   │               │ EventChannels │
│                    (mpsc)        ▼               │ (broadcast)   │
│                           ┌──────────────────────┴────────────┐  │
│                           │  variance-p2p (libp2p Swarm)      │  │
│                           │  ┌──────────────────────────────┐ │  │
│                           │  │ Kademlia · GossipSub · mDNS  │ │  │
│                           │  │ Identity · Messaging · Media │ │  │
│                           │  └──────────────────────────────┘ │  │
│                           └────────────────────────────────── ┘  │
└──────────────────────────────────────────────────────────────────┘
                    │                              ▲
                    │         libp2p TCP/QUIC      │
                    ▼                              │
            ┌───────────────┐              ┌───────────────┐
            │  Other Peers  │              │  Relay Nodes  │
            └───────────────┘              │  (variance-   │
                                           │   relay)      │
                                           └───────────────┘
```

## Runtime Model

The **Tauri desktop app** is the primary runtime. It embeds `variance-app` in-process — there is no sidecar or child process. The React frontend communicates with the Rust backend through Tauri commands (FFI). The standalone `variance-cli` binary exists for headless operation and debugging only.

The P2P node's `Swarm` runs in a dedicated Tokio task and is **not** `Send`/`Sync`. All communication with the swarm goes through typed channels:

- **`NodeCommand`** (tokio `mpsc`): app → swarm. Fire-and-confirm commands like `SendDirectMessage`, `PublishGroupMessage`, `BroadcastUsernameChange`. Each command includes a `oneshot` response channel.
- **`EventChannels`** (tokio `broadcast`): swarm → app. Domain-typed events: `DirectMessageEvent`, `GroupMessageEvent`, `IdentityEvent`, `SignalingEvent`, `RenameEvent`, `TypingEvent`, `OfflineMessageEvent`.
- **`EventRouter`**: subscribes to all `EventChannels` at startup and translates P2P events into `WsMessage` variants broadcast to connected WebSocket clients. This is the single point where inbound P2P activity becomes a frontend-visible state change.

## Crate Dependency Graph

```
variance-cli ─────► variance-app
                        ├──► variance-p2p
                        │       ├──► variance-identity
                        │       ├──► variance-messaging
                        │       ├──► variance-media
                        │       └──► variance-proto
                        ├──► variance-identity   (direct, for API handlers)
                        ├──► variance-messaging   (direct, for API handlers)
                        └──► variance-media       (direct, for API handlers)
```

`variance-proto` is the foundation — all crates depend on it for protobuf types. `variance-p2p` depends on the domain crates to wire protocol handlers into the swarm. `variance-app` also depends on domain crates directly because HTTP handlers need business logic access (e.g., Olm encrypt, MLS group operations).

## Crate Responsibilities

### variance-proto

Protobuf schema definitions (`identity.proto`, `messaging.proto`, `media.proto`) compiled via `prost-build` in `build.rs`. All P2P wire formats and storage formats derive from these schemas.

### variance-p2p

The libp2p network layer. Owns the `Swarm`, composed `NetworkBehaviour`, protocol codecs, and event/command channels.

**Key modules:**
- `behaviour.rs` — composite `NetworkBehaviour` (Kademlia, GossipSub, mDNS, relay, identify, ping, custom request-response protocols)
- `node/mod.rs` — `Node` struct: swarm construction, command loop, `did_to_peer` in-memory mapping, `PeerStore` (sled-backed persistent DID→PeerId)
- `node/event_handlers.rs` — swarm event dispatch: identity responses, DM delivery/ACK, GossipSub messages, connection lifecycle
- `commands.rs` — `NodeCommand` enum + `NodeHandle` (clonable sender wrapper with typed async methods)
- `events.rs` — `EventChannels` with per-domain broadcast channels
- `handlers/` — business logic handlers for identity resolution, offline relaying, signaling
- `protocols/` — libp2p codec implementations for custom request-response protocols
- `peer_store.rs` — sled-backed persistent DID↔PeerId mapping (survives restarts)

### variance-identity

DID identity management. Generates Ed25519 keypairs, creates W3C DID documents, manages IPFS/IPNS storage, and handles username registration with 4-digit discriminators.

**Key modules:**
- `did.rs` — DID generation, document construction, IPNS publishing
- `username.rs` — `UsernameRegistry` with local registration, cache mapping, display name formatting (`name#0042`)
- `cache.rs` — multi-layer identity cache (memory → disk → network)
- `protocol.rs` — helper functions for creating identity request/response protos

### variance-messaging

All chat functionality: direct messages (1-on-1) and group messages.

**Key modules:**
- `direct.rs` — `DirectMessageHandler`: vodozemac Olm (Double Ratchet) session management, encrypt/decrypt, pending message queue
- `mls.rs` — `MlsGroupHandler`: OpenMLS (RFC 9420) group creation, member add/remove, encrypt/decrypt, key package management, at-rest plaintext caching with AES-256-GCM
- `storage.rs` — `LocalMessageStorage`: sled-backed persistence for DMs, group messages, plaintext cache, MLS state, group metadata, conversation tracking, read timestamps
- `offline.rs` — offline message relay protocol integration
- `receipts.rs` — delivery and read receipt tracking
- `typing.rs` — typing indicator state management

### variance-media

WebRTC call signaling. Manages call state machines and SDP/ICE exchange via libp2p custom protocols. Actual media streams use browser WebRTC — this crate handles only the signaling plane.

### variance-app

Application orchestration layer. Owns global state, HTTP API, WebSocket bridge, and event routing.

**Key modules:**
- `api/` — Axum HTTP handlers, split by domain:
  - `mod.rs` — router construction, error → HTTP status mapping
  - `types.rs` — shared request/response structs
  - `helpers.rs` — `ensure_olm_session` (auto-resolve keys via P2P), `send_dm_to_peer` (transmit + queue-if-offline)
  - `identity.rs` — DID identity and username handlers
  - `conversations.rs` — direct messages, reactions
  - `groups.rs` — MLS group CRUD, group messages, group reactions, `send_group_content` helper
  - `calls.rs` — call lifecycle and signaling
  - `social.rs` — receipts, typing indicators, presence
- `state.rs` — `AppState`: holds all shared state (DID, node handle, DM handler, MLS handler, storage, registries, managers)
- `event_router.rs` — `EventRouter`: bridges P2P events → WebSocket messages; auto-joins MLS groups from Welcome DMs
- `websocket.rs` — `WebSocketManager`, `WsMessage` enum, client subscription filtering
- `node.rs` — node startup lifecycle
- `config.rs` — TOML configuration loading

### variance-relay

Standalone relay server binary. Stores offline messages (30-day TTL) for peers that are currently unreachable. Operates as a lightweight libp2p node with relay protocol support.

### variance-cli

Headless CLI binary for debugging and testing. Not the primary runtime.

## Encryption Architecture

### Direct Messages — Olm Double Ratchet (vodozemac)

```
Sender                                              Recipient
  │                                                      │
  ├─ resolve_identity_by_did() ───────────────────────── │
  │  ◄── IdentityFound { olm_identity_key, one_time_keys } │
  │                                                      │
  ├─ create_outbound_session(ik, otk) ──────────────────►│
  │              PreKey message                          │
  │                                                      ├─ create_inbound_session()
  │                                                      │
  ├─ encrypt(plaintext) ────────────────────────────────►│─ decrypt(ciphertext)
  │              Normal message (ratcheted)              │
  │◄──────────────────────────────────────── encrypt()───┤
  │                                                      │
```

- Session init via PreKey messages (single-use one-time keys)
- Subsequent messages use ratcheted Normal messages
- Each message encrypted with a unique key (forward secrecy)
- At rest: `nonce || AES-256-GCM(plaintext)` in sled, keyed by ULID

### Group Messages — MLS (OpenMLS, RFC 9420)

```
Inviter                          GossipSub              Invitee
  │                                  │                      │
  ├─ add_member(KeyPackage) ─────────│                      │
  │  produces Commit + Welcome       │                      │
  │                                  │                      │
  ├─ publish(Commit) ──────────────► │ ─── to all members   │
  │                                  │                      │
  ├─ send_dm(Welcome, encrypted) ───────────────────────── ►│
  │     metadata: type=mls_welcome   │   (Olm-encrypted)    │
  │                                  │                      ├─ join_group_from_welcome()
  │                                  │                      ├─ subscribe_to_topic()
  │                                  │ ◄────────────────────┤
  │                                  │                      │
  ├─ encrypt_message(plaintext) ───► │ ─── to all members ──►│─ process_message()
  │                                  │     (MLS ciphertext)  │
```

- Commits broadcast via GossipSub to existing members
- Welcome messages delivered as Olm-encrypted DMs (auto-join on receipt)
- Per-message forward secrecy and post-compromise security
- Plaintext cached locally with AES-256-GCM (MLS forward secrecy prevents re-decryption)

## Data Flow: Sending a Direct Message

1. **Frontend** → `POST /messages/direct` with `{ recipient_did, text, reply_to? }`
2. **`conversations::send_direct_message`** →
   - `ensure_olm_session()` — if no session, resolves peer identity via P2P, establishes Olm session using identity key + one-time key
   - `DirectMessageHandler::send_message()` — encrypts with Double Ratchet, stores locally in sled
3. **`send_dm_to_peer()`** →
   - `NodeHandle::send_direct_message()` (NodeCommand) → swarm sends via direct P2P
   - If peer offline: queues in `pending_messages` sled tree for later delivery
4. **`emit_dm_sent_event()`** → `EventChannels::send_direct_message(MessageSent)`
5. **WebSocket** broadcast → `WsMessage::DirectMessageSent { ... }`

## Data Flow: Receiving a Direct Message

1. **P2P layer** receives message → `DirectMessageEvent::MessageReceived`
2. **EventRouter** DM listener →
   - `DirectMessageHandler::receive_message()` — decrypts via Double Ratchet
   - Checks `content.metadata["type"]`:
     - `"mls_welcome"` → auto-joins MLS group, broadcasts `WsMessage::MlsGroupJoined`
     - `"reaction"` → normal DM (stored as message with reaction metadata)
     - otherwise → broadcasts `WsMessage::DirectMessageReceived`
   - If PreKey message: refreshes advertised one-time keys
3. **Frontend** receives WebSocket event → invalidates React Query cache → UI updates

## HTTP API Route Map

| Method | Path | Handler |
|--------|------|---------|
| GET | `/health` | Health check |
| GET | `/identity` | Local DID + keys |
| GET | `/identity/resolve/{did}` | Resolve peer identity |
| POST | `/identity/username` | Register username |
| GET | `/identity/username/resolve/{username}` | Look up username → DID |
| GET | `/conversations` | List all conversations |
| POST | `/conversations` | Start conversation (with first message) |
| DELETE | `/conversations/{peer_did}` | Delete conversation |
| POST | `/messages/direct` | Send direct message |
| GET | `/messages/direct/{did}` | Fetch DM history (cursor-paginated) |
| POST | `/messages/direct/{id}/reactions` | Add DM reaction |
| DELETE | `/messages/direct/{id}/reactions/{emoji}` | Remove DM reaction |
| POST | `/messages/group` | Send group message |
| GET | `/messages/group/{id}` | Fetch group message history |
| POST | `/messages/group/{id}/reactions` | Add group reaction |
| DELETE | `/messages/group/{id}/reactions/{emoji}` | Remove group reaction |
| GET | `/mls/groups` | List MLS groups |
| POST | `/mls/groups` | Create MLS group |
| POST | `/mls/groups/{id}/invite` | Invite to group (sends Welcome via DM) |
| POST | `/mls/groups/{id}/leave` | Leave group |
| DELETE | `/mls/groups/{id}/members/{did}` | Remove member |
| POST | `/mls/welcome/accept` | Manually accept Welcome (fallback) |
| POST | `/calls/create` | Create call |
| GET | `/calls/active` | List active calls |
| POST | `/calls/{id}/accept` | Accept call |
| POST | `/calls/{id}/reject` | Reject call |
| POST | `/calls/{id}/end` | End call |
| POST | `/signaling/offer` | Send SDP offer |
| POST | `/signaling/answer` | Send SDP answer |
| POST | `/signaling/ice` | Send ICE candidate |
| POST | `/signaling/control` | Send call control |
| POST | `/receipts/delivered` | Send delivery receipt |
| POST | `/receipts/read` | Send read receipt |
| GET | `/receipts/{message_id}` | Get receipts for message |
| POST | `/typing/start` | Start typing indicator |
| POST | `/typing/stop` | Stop typing indicator |
| GET | `/typing/{recipient}` | Get typing users |
| GET | `/presence` | Get online peers |
| GET | `/ws` | WebSocket upgrade |

## Storage

All local persistence uses **sled** (embedded key-value store). Data is organized into sled trees:

| Tree | Contents | Key Format |
|------|----------|------------|
| `direct_messages` | Olm-encrypted DM ciphertexts | `{sorted_dids}:{ulid}` |
| `group_messages` | MLS group message records | `{group_id}:{ulid}` |
| `group_plaintext` | AES-256-GCM encrypted plaintext cache | `{message_id}` |
| `group_metadata` | Group name, member list, creation time | `{group_id}` |
| `mls_state` | Serialized OpenMLS provider state | `{local_did}` |
| `pending_messages` | Queued DMs for offline peers | `{recipient_did}:{ulid}` |
| `last_read_at` | Per-conversation read timestamp | `{local_did}:{peer_did}` |
| `peer_store` | Persistent DID → PeerId mapping | `{did}` |

Message IDs use **ULID** (Universally Unique Lexicographically Sortable Identifier) — time-ordered and suitable as sled keys for efficient range scans.

## Frontend Architecture

React + TypeScript + Vite, bundled by Tauri.

- **State management**: Zustand stores (`appStore`, `identityStore`, `messagingStore`, `settingsStore`)
- **Data fetching**: TanStack Query (React Query) with the Axum HTTP API
- **Real-time updates**: WebSocket connection to `/ws`, dispatched through `useWebSocket` hook
- **Styling**: Tailwind CSS with dark/light theme support
- **Components**: `onboarding/` (identity gen/recovery), `conversations/` (list, modals), `messages/` (chat view, bubbles, input), `ui/` (shared primitives)

## Adding a New P2P Feature

1. Define protobuf messages in `crates/variance-proto/proto/`
2. Add `NodeCommand` variant in `crates/variance-p2p/src/commands.rs`
3. Add event variant in `crates/variance-p2p/src/events.rs`
4. Handle the command in `node/mod.rs` and/or event in `node/event_handlers.rs`
5. Subscribe to the event in `EventRouter` and forward as `WsMessage`
6. Add HTTP handler in the appropriate `api/` sub-module
7. Wire the route in `api/mod.rs`
8. Add frontend types in `app/src/api/types.ts` and API client in `client.ts`
9. Handle the WebSocket event in `useWebSocket.ts`
