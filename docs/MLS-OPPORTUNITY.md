# MLS Group Encryption

Group messaging uses the Messaging Layer Security protocol ([RFC 9420](https://datatracker.ietf.org/doc/html/rfc9420)) via [openmls](https://github.com/openmls/openmls). This document explains the design, what MLS replaces, and the implementation structure.

The hand-rolled AES-256-GCM + X25519 group crypto has been replaced with OpenMLS. The evaluation rationale is preserved below for context.

## Current State

### 1:1 DMs — Olm (vodozemac 0.9)

Each node creates a `vodozemac::olm::Account` at identity generation. The account holds a Curve25519 key pair and a pool of one-time pre-keys (OTKs). Session establishment follows the Olm X3DH-like handshake:

1. Alice resolves Bob's DID → gets `olm_identity_key` + `one_time_keys` from `IdentityFound`.
2. Alice calls `create_outbound_session(identity_key, otk)` → PreKey `OlmMessage`.
3. Bob receives PreKey, calls `create_inbound_session(alice_identity_key, pre_key_msg)`.
4. Subsequent messages use Double Ratchet (`session.encrypt` / `session.decrypt`).

**Persistence:** Account pickled to `identity.json`; sessions pickled to sled trees; plaintext encrypted at rest with AES-256-GCM (key derived via HKDF from signing key).

**Files:**
- `variance-app/src/identity_gen.rs` — `Account::new()`, pickle serialization
- `variance-app/src/state.rs` — `IdentityFile.olm_account_pickle`, `Account::from_pickle()`
- `variance-app/src/node.rs` — OTK generation, mark published, persist pickle
- `variance-messaging/src/direct.rs` — `DirectMessageHandler`: session init, encrypt, decrypt, self-messages, session persistence
- `variance-messaging/src/storage.rs` — `MessageStorage` trait: `store_session_pickle`, `fetch_session_pickle`, `load_all_session_pickles`, `store_plaintext`, `fetch_plaintext`
- `variance-proto/proto/identity.proto` — `IdentityFound.olm_identity_key`, `one_time_keys`
- `variance-proto/proto/messaging.proto` — `DirectMessage.olm_message_type`, `sender_identity_key`

### Group Chats — OpenMLS (RFC 9420)

Groups use OpenMLS backed by GossipSub for message delivery. Each group is an `MlsGroup` with a ratchet tree that provides per-message forward secrecy and post-compromise security.

1. Admin creates group → `MlsGroup::new()` with Ed25519 credential.
2. Admin invites member → `MlsGroup::add_members()` produces a `Commit` + `Welcome`. Welcome sent as `GroupInvitation.mls_welcome`.
3. Messages encrypted via `MlsGroup::create_message()` → `MlsCiphertext` serialized into `GroupMessage.mls_ciphertext`.
4. On member removal → `MlsGroup::remove_members()` produces a `Commit`; epoch advances, ratchet tree updated.

**Replaced approach (AES-256-GCM + X25519):**
The old design used a symmetric AES-256 key versioned per epoch, distributed via X25519 ECDH. Limitations: no per-message forward secrecy within an epoch, no post-compromise security, entirely custom key management.

**Files:**
- `variance-messaging/src/mls.rs` — `MlsGroupHandler`: create, join, add/remove member, encrypt/decrypt, commit processing
- `variance-messaging/src/storage.rs` — `store_mls_group_state`, `fetch_mls_group_state`, `store_key_package`, `fetch_key_packages`
- `variance-proto/proto/messaging.proto` — `GroupMessage.mls_ciphertext`, `GroupInvitation.mls_welcome`, `KeyPackageMessage`
- `variance-proto/proto/identity.proto` — `IdentityFound.mls_key_package`

### Shared Primitives

Both DM and group use:
- `ed25519-dalek` for message signing / verification
- `hkdf` + `sha2` for key derivation
- `aes-gcm` for at-rest encryption of plaintext cache and group key blobs
- `rand` for nonce / key generation

## What MLS Provides

MLS (RFC 9420) is an IETF standard for asynchronous group key agreement. `openmls` (v0.8.1, MIT, ~35K SLoC) implements it in Rust with pluggable crypto backends.

### Core concepts

| MLS concept | Maps to Variance... |
|---|---|
| **Group** | A `Group` (conversation) |
| **KeyPackage** | Published per-user; replaces OTKs + X25519 pubkey |
| **Welcome** | Replaces `GroupInvitation.encrypted_group_key` |
| **Commit** | Group state transition (add/remove/update member) |
| **Application message** | Encrypted message in group epoch |
| **MlsGroup** | Replaces `GroupMessageHandler` state per-group |

### Security gains

- **Per-message forward secrecy** via ratchet tree — not just per-epoch.
- **Post-compromise security** — a compromised member can self-heal by issuing an Update commit.
- **Formal security proofs** — IETF-audited vs. hand-rolled AES-GCM key management.
- **Multi-device** — each device is a separate leaf in the ratchet tree (natural extension).
- **1:1 as group-of-2** — MLS works for any group size ≥ 2, unifying DM and group encryption under one protocol.

### Ciphersuite alignment

OpenMLS supports `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` which uses:
- X25519 for DH key exchange (same as our group crypto)
- AES-128-GCM for encryption (we use AES-256-GCM — slightly different, but same family)
- SHA-256 for hashing (same as our HKDF usage)
- Ed25519 for signatures (same as our signing keys)

This is a near-perfect fit — our existing `ed25519-dalek` keys can serve as MLS credentials.

## Decision: MLS for Groups, Olm for DMs

Group crypto uses MLS (Option A). 1:1 DMs keep Olm (`vodozemac`).

**Rationale:**
- Group crypto had the biggest security gaps (no per-message FS, no PCS).
- Olm is battle-tested (Matrix/Element), well-understood, and `vodozemac` is actively maintained.
- Olm for 1:1 is simpler and lighter than an MLS group-of-2.
- Impact scoped to `mls.rs`, proto definitions, and storage — DMs untouched.

DMs → MLS can be revisited later if multi-device support becomes a priority (each device is a separate leaf in the ratchet tree).

## File-by-File Impact

### High impact (major rewrite)

| File | Change |
|---|---|
| `variance-messaging/src/mls.rs` (was `group.rs`) | **Replaced.** `GroupMessageHandler` is now a thin wrapper around `openmls::MlsGroup`. Key creation, add/remove member, encrypt/decrypt all delegate to MLS. X25519 manual key exchange eliminated. Versioned key maps replaced by MLS epochs. |
| `variance-proto/proto/messaging.proto` | **`GroupMessage`**: drop `nonce`, `key_version`; add `mls_ciphertext: bytes` (openmls `MlsCiphertext` serialized). **`GroupInvitation`**: drop `encrypted_group_key`, `key_version`; add `mls_welcome: bytes`. **`GroupKey`**: remove (MLS manages keys internally). **`GroupMember.x25519_key`**: remove (MLS uses `KeyPackage` instead). Add `KeyPackageMessage` for publishing/distributing key packages. |
| `variance-messaging/src/storage.rs` | **Remove:** `store_group_key_encrypted`, `fetch_group_key_encrypted`, `store_versioned_group_key`, `fetch_all_group_keys`. **Add:** `store_mls_group_state` / `fetch_mls_group_state` (serialized `MlsGroup`), `store_key_package` / `fetch_key_packages`. OpenMLS can use its bundled `openmls_sqlite_storage` or we implement `openmls_traits::storage::StorageProvider` over sled. |

### Medium impact (significant edits)

| File | Change |
|---|---|
| `variance-messaging/Cargo.toml` | **Add:** `openmls = "0.8"`, `openmls_rust_crypto = "0.5"` (or `openmls_libcrux_crypto`), `openmls_basic_credential = "0.5"`. **Remove:** `x25519-dalek`. `aes-gcm` and `hkdf` stay for at-rest encryption + DMs. |
| `variance-app/src/state.rs` | `AppState.group_messaging` type unchanged (still `Arc<GroupMessageHandler>`), but the inner handler's constructor needs an MLS crypto provider and credential. Add MLS `CredentialWithKey` derived from existing Ed25519 signing key. |
| `variance-app/src/node.rs` | Group-related boot code changes: generate initial `KeyPackage` instead of X25519 key derivation. Publish `KeyPackage` alongside OTKs in `IdentityFound`. |
| `variance-app/src/event_router.rs` | Group message handler: deserialize `MlsCiphertext` instead of raw AES-GCM, call `mls_group.process_message()`. Commit processing for add/remove/update. |
| `variance-proto/proto/identity.proto` | `IdentityFound`: add `bytes mls_key_package = 7` for the responding peer's current key package. Keep `olm_identity_key` and `one_time_keys` (DM Olm stays). |
| `variance-p2p/src/handlers/identity.rs` | Identity response builder: include MLS key package bytes alongside Olm keys. |

### Low impact (minor tweaks)

| File | Change |
|---|---|
| `variance-app/src/api.rs` | Group API endpoints: adjust request/response types if GroupInvitation shape changes. |
| `variance-app/src/identity_gen.rs` | **No change** (Olm account generation stays for DMs). Optionally generate initial MLS key packages here too. |
| `variance-messaging/src/direct.rs` | Unchanged. Olm stays for 1:1. |
| `variance-messaging/src/offline.rs` | **No change** to envelope format; MLS ciphertext is just bytes in the existing `GroupMessage` field. |
| `variance-messaging/src/error.rs` | Add MLS-specific error variants (`MlsGroupError`, `MlsWelcomeError`, etc.). |
| `variance-messaging/src/lib.rs` | **No change.** |

### No impact

| File | Reason |
|---|---|
| `variance-identity/` | No crypto changes (DID resolution, caching, IPFS storage unaffected). |
| `variance-media/` | WebRTC signaling uses its own DTLS-SRTP; unrelated. |
| `variance-relay/` | Relay handles opaque `OfflineMessageEnvelope` bytes; doesn't decrypt. |
| `variance-cli/` | CLI command handlers just call into `variance-app`. |
| `app/` (frontend) | TypeScript client sends/receives JSON over HTTP — crypto is transparent. |

## Migration Notes

The migration from hand-rolled AES-256-GCM replaced `group.rs` with `mls.rs` in three phases:

1. **Plumbing:** Added `openmls`, `openmls_rust_crypto`, `openmls_basic_credential`; implemented `StorageProvider` over sled; derived `CredentialWithKey` from existing Ed25519 signing key; added `KeyPackage` to `IdentityFound` responses.
2. **Wire-up:** Replaced `GroupMessageHandler` create/add/remove/send/receive with `MlsGroup` equivalents; updated proto definitions; updated `event_router` to handle MLS message types (Commit, Welcome, Application).
3. **Cleanup:** Removed `x25519-dalek`; removed manual key rotation / versioned key maps; removed `GroupKey`, `GroupMember.x25519_key`, old `GroupInvitation` fields from proto.

### Storage migration for existing groups

- **Old messages (pre-MLS):** Kept as-is in sled; still decryptable with old AES keys stored alongside them.
- **Active groups:** On first use after upgrade, admin re-creates the group as an MLS group and sends Welcome messages to all members (one-time "group upgrade" flow).
- **Group keys:** Old versioned keys remain in sled for decrypting pre-MLS history. New MLS epochs are managed by openmls internally.

## Dependency Cost

```
openmls          = "0.8"     # ~35K SLoC, core MLS logic
openmls_rust_crypto = "0.5"  # Crypto backend (wraps ring/RustCrypto)
openmls_basic_credential = "0.5"  # Basic credential type
```

- Total added deps: ~50K SLoC (openmls + crypto provider).
- Compile time impact: moderate (openmls uses `rayon` for tree operations, `tls_codec`, `serde`).
- `openmls_rust_crypto` uses `RustCrypto` primitives — same family as our existing `aes-gcm`, `hkdf`, `sha2`, `ed25519-dalek`.
- Actively maintained by Phoenix R&D + Cryspen. Last release: Feb 2026.

## Risks

1. **openmls maturity:** v0.8.1 is still pre-1.0. API may change. But 887 stars, 52 contributors, Wire uses it, so it's production-tested.
2. **Delivery service assumption:** MLS assumes a delivery service that orders commits. In our P2P model, GossipSub doesn't guarantee ordering. We need to handle concurrent commits / conflicts (openmls has `fork-resolution` feature for this).
3. **Key package distribution:** MLS needs peers to publish `KeyPackage`s. We already distribute `olm_identity_key` + `one_time_keys` via `IdentityFound` — adding a key package is straightforward.
4. **Offline members:** MLS commits require all members' key packages. If a member is offline during an add/remove, we need to buffer the commit and deliver it when they come online — this fits our existing offline relay infrastructure.
5. **Group size performance:** MLS ratchet tree operations are O(log n). For small groups (<100) this is irrelevant. For very large channels, it matters — but we don't support those yet.

## Key Design Considerations

### Commit ordering in P2P

MLS assumes a delivery service that orders commits. GossipSub doesn't guarantee ordering. Concurrent commits / conflicts are handled using the openmls `fork-resolution` feature. The offline relay infrastructure handles buffering commits for offline members.

### Ciphersuite

`MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` — X25519 DH, AES-128-GCM, SHA-256, Ed25519 signatures. Our existing `ed25519-dalek` keys serve as MLS credentials directly.
