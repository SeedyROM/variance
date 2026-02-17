//! Business logic handlers for custom protocols
//!
//! Connects protocol events to actual functionality from other crates.
//!
//! ## Completed Features (Phases 1-4)
//!
//! ### Offline Message Handler ✓
//! - [x] Proper error responses with structured error types
//! - [x] Error variant added to OfflineMessageResponse protobuf
//! - [ ] Message acknowledgment protocol (future work)
//! - [ ] TTL-based message expiration monitoring (future work)
//!
//! ### Signaling Handler ✓
//! - [x] Message signature verification using Ed25519
//! - [x] Identity system integration for public key lookup
//! - [x] Full call manager with WebRTC PeerConnection
//! - [x] SDP offer/answer processing
//! - [x] ICE candidate handling with NAT traversal
//! - [x] Call state management (ringing, active, ended)
//! - [x] STUN/TURN server configuration
//!
//! ## TODO: Future Work
//!
//! ### Identity Handler
//! - [ ] IPFS/IPNS integration for persistent DID storage (backend implemented in variance-identity)
//! - [ ] DHT provider record lookup for username discovery
//! - [ ] Multi-peer username resolution with verification
//! - [ ] Disk-based cache layer with TTL expiration
//!
//! ### Cross-Cutting Concerns
//! - [ ] Metrics and observability for handler operations
//! - [ ] Rate limiting and abuse prevention
//! - [ ] Request timeout handling
//! - [ ] Graceful degradation when dependencies unavailable

pub mod identity;
pub mod offline;
pub mod signaling;
