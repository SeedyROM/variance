# Protocol Guide

How protobuf schemas work across the Variance codebase.

## Overview

All data exchanged between components uses Protocol Buffers:
- **Type-safe**: Schemas enforced at compile time
- **Versioned**: Forward/backward compatibility
- **Efficient**: Binary encoding
- **Language-agnostic**: Works with Rust, Go, TypeScript, etc.

## Schema Organization

```
crates/variance-proto/proto/
├── identity.proto    # DID documents, identity resolution
├── messaging.proto   # Chat messages, groups, receipts
└── media.proto       # WebRTC signaling
```

## Identity Protocol

### File: `identity.proto`

#### DID Document Storage

```protobuf
message DIDDocument {
  string id = 1;                                    // did:peer:123...
  repeated VerificationMethod authentication = 2;  // Signing keys
  repeated VerificationMethod key_agreement = 3;   // Encryption keys
  repeated Service service = 4;                     // libp2p endpoints
  int64 created_at = 5;
  int64 updated_at = 6;
  optional string display_name = 7;                 // Human-readable name
  optional string avatar_cid = 8;                   // IPFS CID
}
```

**Storage:**
- Serialized → Stored in IPFS → Get CID
- IPNS publishes mutable pointer to latest CID
- DHT stores provider records (who has this identity)

#### Identity Resolution Protocol

```protobuf
message IdentityRequest {
  oneof query {
    UsernameQuery username = 1;  // Search by @alice#1234
    string did = 2;              // Search by DID
    string peer_id = 3;          // Search by libp2p PeerID
  }
  optional string requester_did = 4;  // Who's asking
  int64 timestamp = 5;
}

message IdentityResponse {
  oneof result {
    IdentityFound found = 1;
    IdentityNotFound not_found = 2;
    IdentityError error = 3;
  }
}
```

**Usage:**
```rust
use variance_proto::identity_proto::*;

let request = IdentityRequest {
    query: Some(identity_request::Query::Username(UsernameQuery {
        username: "alice".into(),
        discriminator: Some(1234),
        subnet_id: Some("public".into()),
    })),
    requester_did: Some(my_did),
    timestamp: now(),
};

// Send via custom libp2p protocol
let response: IdentityResponse = protocol.request(&peer, request).await?;

match response.result {
    Some(identity_response::Result::Found(found)) => {
        let did_doc = found.did_document.unwrap();
        // Use DID document
    }
    _ => { /* Handle not found / error */ }
}
```

#### Identity Updates (GossipSub)

```protobuf
message IdentityUpdate {
  string did = 1;
  optional string ipns_key = 2;
  optional string new_ipfs_cid = 3;  // Updated DID doc CID
  int64 updated_at = 4;
  bytes signature = 5;               // Proof of ownership
  repeated string changed_fields = 6; // "display_name", "avatar_cid"
}
```

**Flow:**
1. User updates profile
2. New DID document → IPFS → Get new CID
3. IPNS re-publish with new CID
4. Broadcast `IdentityUpdate` via GossipSub
5. Friends invalidate cache for this DID

## Messaging Protocol

### File: `messaging.proto`

#### Direct Messages

```protobuf
message DirectMessage {
  string id = 1;                           // ULID (chronological sort)
  string sender_did = 2;
  string recipient_did = 3;
  bytes ciphertext = 4;                    // OlmMessage body (to_parts().1)
  uint32 olm_message_type = 5;            // 0 = PreKey, 1 = Normal (to_parts().0)
  bytes signature = 6;                     // Ed25519 signature
  int64 timestamp = 7;                     // Unix ms
  MessageType type = 8;
  optional string reply_to = 9;
  optional bytes sender_identity_key = 10; // Curve25519 key, PreKey messages only
}
```

**Encryption uses `vodozemac` (Olm Double Ratchet):**
```rust
// Encrypt (outbound session already established)
let plaintext = content.encode_to_vec();
let olm_message = session.encrypt(&plaintext);
let (msg_type, ciphertext) = olm_message.to_parts();
// msg_type: 0 = PreKey (first message), 1 = Normal (ratcheted)

// Sign the ciphertext
let signature = keypair.sign(&ciphertext).to_bytes().to_vec();

let dm = DirectMessage {
    id: Ulid::new().to_string(),
    sender_did: my_did.clone(),
    recipient_did: their_did.clone(),
    ciphertext,
    olm_message_type: msg_type as u32,
    signature,
    timestamp: now_ms(),
    r#type: MessageType::Text.into(),
    reply_to: None,
    sender_identity_key: if msg_type == 0 {
        Some(account.curve25519_key().to_vec())
    } else {
        None
    },
};

// Decrypt (inbound)
let olm_msg = OlmMessage::from_parts(dm.olm_message_type as usize, &dm.ciphertext)?;
// PreKey: creates new inbound session from account
// Normal: decrypt with existing session
let plaintext = session.decrypt(&olm_msg)?;
let content = MessageContent::decode(&plaintext[..])?;
```

#### Group Messages

```protobuf
message GroupMessage {
  string id = 1;
  string sender_did = 2;
  string group_id = 3;
  bytes ciphertext = 4;  // AES-256-GCM with group key
  bytes nonce = 5;
  bytes signature = 6;
  int64 timestamp = 7;
  MessageType type = 8;
}
```

**Group Key Distribution:**
```protobuf
message GroupInvitation {
  string group_id = 1;
  string group_name = 2;
  string inviter_did = 3;
  string invitee_did = 4;
  bytes encrypted_group_key = 5;  // Encrypted with invitee's pubkey
  int64 timestamp = 6;
  bytes signature = 7;
}
```

**Flow:**
1. Admin creates group → Generate AES-256 group key
2. For each member: Encrypt group key with their X25519 public key
3. Send `GroupInvitation` via DM
4. Member decrypts group key
5. Messages encrypted/decrypted with shared group key
6. Broadcast via GossipSub topic: `/variance/{subnet}/group/{group_id}`

#### Offline Messages (Relay)

```protobuf
message OfflineMessageEnvelope {
  string recipient_did = 1;
  oneof message {
    DirectMessage direct = 2;
    GroupMessage group = 3;
  }
  string relay_peer_id = 4;  // Which relay stored it
  int64 stored_at = 5;
  int64 expires_at = 6;      // TTL (30 days)
}
```

**Retrieval:**
```rust
let request = OfflineMessageRequest {
    did: my_did,
    since_timestamp: Some(last_online),
    limit: 100,
};

let response: OfflineMessageResponse = relay
    .query_offline_messages(request)
    .await?;

for envelope in response.messages {
    match envelope.message {
        Some(Message::Direct(dm)) => {
            // Decrypt and process
        }
        Some(Message::Group(gm)) => {
            // Decrypt with group key
        }
        None => {}
    }
}
```

## Media Protocol

### File: `media.proto`

#### WebRTC Signaling

```protobuf
message SignalingMessage {
  string call_id = 1;
  string sender_did = 2;
  string recipient_did = 3;
  oneof message {
    Offer offer = 4;
    Answer answer = 5;
    ICECandidate ice_candidate = 6;
    CallControl control = 7;
  }
  int64 timestamp = 8;
  bytes signature = 9;
}
```

**Call Flow:**
```rust
// 1. Alice initiates call
let offer = Offer {
    sdp: peer_connection.create_offer().await?.sdp,
    call_type: CallType::Video.into(),
};

let signal = SignalingMessage {
    call_id: Ulid::new().to_string(),
    sender_did: alice_did,
    recipient_did: bob_did,
    message: Some(signaling_message::Message::Offer(offer)),
    timestamp: now(),
    signature: sign(...),
};

// Send via libp2p stream (NOT gossipsub - direct only)
send_signaling(&bob_peer_id, signal).await?;

// 2. Bob receives offer
match receive_signaling().await? {
    Some(signaling_message::Message::Offer(offer)) => {
        peer_connection.set_remote_description(offer.sdp).await?;
        let answer = peer_connection.create_answer().await?;

        let response = SignalingMessage {
            call_id: signal.call_id,
            sender_did: bob_did,
            recipient_did: alice_did,
            message: Some(signaling_message::Message::Answer(Answer {
                sdp: answer.sdp,
            })),
            timestamp: now(),
            signature: sign(...),
        };

        send_signaling(&alice_peer_id, response).await?;
    }
}

// 3. Exchange ICE candidates
for candidate in ice_candidates {
    let ice_msg = SignalingMessage {
        message: Some(signaling_message::Message::IceCandidate(
            ICECandidate {
                candidate: candidate.candidate,
                sdp_mid: candidate.sdp_mid,
                sdp_m_line_index: candidate.sdp_m_line_index,
            }
        )),
        // ... other fields
    };
    send_signaling(&peer_id, ice_msg).await?;
}
```

## Code Generation

### Build Process

`crates/variance-proto/build.rs`:
```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::Config::new()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(
            &[
                "proto/identity.proto",
                "proto/messaging.proto",
                "proto/media.proto",
            ],
            &["proto/"],
        )?;
    Ok(())
}
```

**Generated code location:**
```
target/debug/build/variance-proto-<hash>/out/
├── variance.identity.v1.rs
├── variance.messaging.v1.rs
└── variance.media.v1.rs
```

### Using Generated Code

```rust
// In variance-proto/src/lib.rs
pub mod identity {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/variance.identity.v1.rs"));
    }
}

// In other crates
use variance_proto::identity_proto::DIDDocument;
use variance_proto::messaging_proto::DirectMessage;
use variance_proto::media_proto::SignalingMessage;
```

## Versioning Strategy

### Breaking Changes

When making breaking changes to a proto:
1. Create new package: `variance.identity.v2`
2. Keep v1 for backward compatibility
3. Support both versions during transition
4. Deprecate v1 after migration period

### Non-Breaking Changes

Safe additions:
- New optional fields
- New enum values (with UNSPECIFIED = 0)
- New message types in oneofs
- New services/RPCs

## Best Practices

### 1. Always Use `optional` for Nullable Fields

```protobuf
// Good
optional string display_name = 1;

// Bad (required in proto3)
string display_name = 1;  // Empty string vs null confusion
```

### 2. Include Timestamps

```protobuf
message MyMessage {
  // ... fields
  int64 created_at = 100;  // Use high field numbers for metadata
  int64 updated_at = 101;
}
```

### 3. Sign Important Messages

```protobuf
message SecureMessage {
  // ... payload
  bytes signature = 1000;  // Always last field
}
```

### 4. Use Enums with UNSPECIFIED

```protobuf
enum MessageType {
  MESSAGE_TYPE_UNSPECIFIED = 0;  // Default value
  MESSAGE_TYPE_TEXT = 1;
  MESSAGE_TYPE_IMAGE = 2;
}
```

## Debugging

### View Encoded Message

```rust
let msg = DirectMessage { /* ... */ };
let bytes = msg.encode_to_vec();
println!("Encoded: {} bytes", bytes.len());
println!("Hex: {}", hex::encode(&bytes));
```

### Decode from Bytes

```rust
use prost::Message;

let bytes: Vec<u8> = /* received data */;
let msg = DirectMessage::decode(&bytes[..])?;
println!("Decoded: {:?}", msg);
```
