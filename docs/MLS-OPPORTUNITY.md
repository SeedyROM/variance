# MLS Integration Opportunity

Evaluating whether to adopt the Messaging Layer Security protocol ([RFC 9420](https://datatracker.ietf.org/doc/html/rfc9420)) via [openmls](https://github.com/openmls/openmls) before the current crypto layer hardens further.

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

### Group Chats — AES-256-GCM + X25519 (hand-rolled)

Each group has a symmetric AES-256 key, versioned for forward secrecy on member removal. Key distribution uses X25519 ECDH:

1. Admin creates group → random 32-byte AES key (version 1).
2. Admin invites member → encrypts group key with member's X25519 public key (ECDH shared secret → HKDF → AES-256-GCM wrapping). Sent as `GroupInvitation.encrypted_group_key`.
3. Messages encrypted with `Aes256Gcm::encrypt(group_key, nonce, plaintext)`.
4. On member removal → `rotate_key()`: new version, re-encrypt for all remaining members via X25519.

**Limitations of current approach:**
- No forward secrecy *within* a key epoch (all messages between rotations use the same key).
- No post-compromise security (compromised key decrypts all messages until next rotation).
- X25519 key missing for pre-migration or self-created members → re-keying skips them.
- Key distribution is all-or-nothing: new member gets current key but not historical keys (by design), and the admin must be online to rotate.
- No standardized protocol — entirely custom, harder to audit.

**Files:**
- `variance-messaging/src/group.rs` — `GroupMessageHandler`: create, join, add/remove member, rotate key, encrypt/decrypt, X25519 key derivation
- `variance-messaging/src/storage.rs` — `store_group_key_encrypted`, `store_versioned_group_key`, `fetch_all_group_keys`, `store_group_metadata`
- `variance-proto/proto/messaging.proto` — `GroupMessage.key_version`, `GroupKey`, `GroupInvitation.encrypted_group_key`, `GroupMember.x25519_key`

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

## Decision: Replace Groups Only, or DMs Too?

### Option A: MLS for groups only (recommended)

Replace the hand-rolled AES-256-GCM + X25519 group crypto with MLS groups. Keep Olm for 1:1 DMs.

**Rationale:**
- Group crypto is where the biggest security gap exists (no per-message FS, no PCS).
- Olm is battle-tested (Matrix/Element), well-understood, and `vodozemac` is actively maintained.
- Olm for 1:1 is simpler and lighter than an MLS group-of-2.
- Lower migration risk — only group.rs changes significantly.

### Option B: MLS everywhere (DMs become group-of-2)

Replace both Olm and the group layer with MLS. Every conversation is an MLS group.

**Rationale:**
- Single encryption protocol to maintain and audit.
- Multi-device support for DMs and groups with the same mechanism.
- But: heavier per-message cost for 1:1, loses Olm simplicity, larger migration surface.

**Recommendation: Start with Option A.** The group layer is where the security gaps are real. We can revisit DMs → MLS later when multi-device becomes a priority.

## File-by-File Impact (Option A)

### High impact (major rewrite)

| File | Change |
|---|---|
| `variance-messaging/src/group.rs` (1691 lines) | **Replace entirely.** `GroupMessageHandler` becomes a thin wrapper around `openmls::MlsGroup`. Key creation, add/remove member, encrypt/decrypt all delegate to MLS. X25519 manual key exchange is eliminated. Versioned key maps are replaced by MLS epochs. |
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
| `variance-messaging/src/direct.rs` | **No change.** Olm stays for 1:1. |
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

## Migration Strategy

### Phase 1: Add openmls plumbing (no behavior change)

1. Add `openmls`, `openmls_rust_crypto`, `openmls_basic_credential` to `variance-messaging/Cargo.toml`.
2. Implement `StorageProvider` for sled (or use `openmls_sqlite_storage` if migrating DB).
3. Create `MlsGroupHandler` alongside existing `GroupMessageHandler`.
4. Derive `CredentialWithKey` from existing Ed25519 signing key at boot.
5. Generate initial `KeyPackage` and add to `IdentityFound` responses.

### Phase 2: Wire up MLS groups

1. Replace `GroupMessageHandler.create_group()` → `MlsGroup::new()`.
2. Replace `add_member()` → `MlsGroup::add_members()` (produces `Commit` + `Welcome`).
3. Replace `remove_member()` → `MlsGroup::remove_members()` (produces `Commit`).
4. Replace `send_message()` → `MlsGroup::create_message()`.
5. Replace `receive_message()` → `MlsGroup::process_message()`.
6. Update proto definitions.
7. Update event_router to handle MLS message types.

### Phase 3: Clean up

1. Remove `x25519-dalek` dependency.
2. Remove manual key rotation / versioning code.
3. Remove `GroupKey`, `GroupMember.x25519_key`, old `GroupInvitation` fields from proto.
4. Run full test suite; add MLS-specific integration tests.

### Storage migration

Existing group data (messages, metadata) in sled needs a migration path:
- **Old messages:** Keep as-is; they were encrypted with old AES keys that are still in sled. Mark with a `pre_mls: bool` flag.
- **Active groups:** On first use after upgrade, the admin re-creates the group as an MLS group and sends Welcome messages to all members. This is a one-time "group upgrade" flow.
- **Group keys:** Old versioned keys stay in sled for decrypting old messages. New MLS epochs are managed by openmls internally.

## Effort Estimate

| Phase | Estimated effort |
|---|---|
| Phase 1: plumbing | 2–3 days |
| Phase 2: wire up | 4–5 days |
| Phase 3: cleanup + tests | 2–3 days |
| **Total** | **~10 days** |

The bulk of the work is in `group.rs` (1691 lines of custom key management replaced by MLS delegation) and the proto changes (which ripple into event_router, api, and storage).

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

## Conclusion

The group encryption layer is the weakest part of our crypto stack — hand-rolled AES key management without per-message forward secrecy or post-compromise security. MLS directly addresses these gaps with an IETF-standardized, formally audited protocol.

Integration is feasible in ~10 days because:
- Our Ed25519 keys are already MLS-compatible.
- The impact is scoped to `group.rs`, proto definitions, and storage — DMs are untouched.
- openmls provides a high-level API (`MlsGroup`) that replaces our entire manual key management.

The main architectural question is handling commit ordering in a P2P environment without a central delivery service. This is solvable with the `fork-resolution` feature and our existing offline relay — but it's the area that needs the most design attention.
