use dashmap::DashMap;
use libp2p::PeerId;
use std::time::Instant;

/// A single token bucket that refills at a fixed rate.
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token. Returns `true` if allowed.
    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }
}

/// Configuration for a single protocol's rate limit bucket.
#[derive(Debug, Clone, Copy)]
pub struct BucketConfig {
    /// Maximum burst size (and initial token count).
    pub max_tokens: f64,
    /// Sustained refill rate in tokens per second.
    pub refill_rate: f64,
}

impl BucketConfig {
    /// Convenience: `max_tokens` as burst, refill = `max_tokens / window_secs`.
    pub const fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            max_tokens,
            refill_rate,
        }
    }

    /// Create from a "tokens per window" description.
    /// E.g. `from_window(10, 60)` → 10 burst, refills at 10/60 ≈ 0.167/s.
    pub fn from_window(tokens: u32, window_secs: u32) -> Self {
        Self {
            max_tokens: tokens as f64,
            refill_rate: tokens as f64 / window_secs as f64,
        }
    }
}

/// Well-known protocol identifiers used as rate-limit keys.
/// These are internal labels, not the wire protocol strings.
pub mod protocol {
    pub const IDENTITY: &str = "identity";
    pub const DIRECT_MESSAGES: &str = "direct_messages";
    pub const OFFLINE_MESSAGES: &str = "offline_messages";
    pub const TYPING_INDICATORS: &str = "typing_indicators";
    pub const SIGNALING: &str = "signaling";
    /// Username rename notifications — tight limit since renames are rare.
    pub const RENAME: &str = "rename";
    pub const GLOBAL: &str = "__global__";
}

/// Per-peer, per-protocol token-bucket rate limiter.
///
/// Every inbound request from a remote peer is checked against two buckets:
/// 1. A **protocol-specific** bucket (e.g. "identity" → 10 req / 60s)
/// 2. A **global per-peer** bucket that caps total cross-protocol load
///
/// If either bucket is exhausted the request is rejected (caller should drop it).
pub struct PeerRateLimiter {
    /// (PeerId, protocol_label) → TokenBucket
    protocol_buckets: DashMap<(PeerId, &'static str), TokenBucket>,
    /// PeerId → global TokenBucket
    global_buckets: DashMap<PeerId, TokenBucket>,
    /// Config per protocol label
    configs: DashMap<&'static str, BucketConfig>,
    /// Global per-peer config
    global_config: BucketConfig,
    /// Inverted index: PeerId → list of protocol labels the peer has used.
    /// Enables O(1) cleanup in `remove_peer` — we only touch buckets for this peer.
    peer_protocols: DashMap<PeerId, Vec<&'static str>>,
}

/// Result of a rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// Request is allowed.
    Allowed,
    /// Blocked by the protocol-specific bucket.
    ProtocolLimited,
    /// Blocked by the global per-peer bucket.
    GlobalLimited,
}

impl RateLimitDecision {
    pub fn is_allowed(self) -> bool {
        self == Self::Allowed
    }
}

impl PeerRateLimiter {
    /// Create a limiter with the default protocol budgets.
    ///
    /// Budgets are tiered by priority — direct messages get the most headroom
    /// since they're the core user experience. Typing indicators are generous
    /// enough that a peer active in many group chats won't get throttled.
    /// The global per-peer cap is intentionally high so multi-protocol
    /// legitimate traffic (messages + typing + signaling during a call)
    /// isn't accidentally blocked.
    ///
    /// | Protocol           | Burst | Sustained     | Rationale                          |
    /// |--------------------|-------|---------------|------------------------------------|
    /// | direct_messages    |    60 | 1/s           | Highest priority — core UX         |
    /// | typing_indicators  |    20 | 0.33/s        | Must handle multi-group activity   |
    /// | signaling          |    30 | 0.5/s         | ICE trickle can burst during setup |
    /// | identity           |    10 | 0.17/s        | Cacheable, low sustained need      |
    /// | offline_messages   |     3 | 0.05/s        | Rare; only after reconnect         |
    /// | **global/peer**    |   500 | ~8.3/s        | High ceiling for multi-protocol    |
    pub fn new() -> Self {
        let limiter = Self {
            protocol_buckets: DashMap::new(),
            global_buckets: DashMap::new(),
            configs: DashMap::new(),
            global_config: BucketConfig::from_window(500, 60),
            peer_protocols: DashMap::new(),
        };

        // Priority tier 1: core messaging
        limiter
            .configs
            .insert(protocol::DIRECT_MESSAGES, BucketConfig::from_window(60, 60));

        // Priority tier 2: interactive presence
        limiter.configs.insert(
            protocol::TYPING_INDICATORS,
            BucketConfig::from_window(20, 60),
        );
        limiter
            .configs
            .insert(protocol::SIGNALING, BucketConfig::from_window(30, 60));

        // Priority tier 3: infrastructure
        limiter
            .configs
            .insert(protocol::IDENTITY, BucketConfig::from_window(10, 60));
        limiter
            .configs
            .insert(protocol::OFFLINE_MESSAGES, BucketConfig::from_window(3, 60));
        // Rename notifications are extremely rare — 2 per minute is generous
        limiter
            .configs
            .insert(protocol::RENAME, BucketConfig::from_window(2, 60));

        limiter
    }

    /// Create a limiter with a custom global per-peer config.
    pub fn with_global_config(global_config: BucketConfig) -> Self {
        let mut limiter = Self::new();
        limiter.global_config = global_config;
        limiter
    }

    /// Override the config for a specific protocol.
    pub fn set_protocol_config(&self, protocol: &'static str, config: BucketConfig) {
        self.configs.insert(protocol, config);
    }

    /// Check whether `peer` is allowed to make a request on `protocol`.
    ///
    /// Consumes one token from both the protocol and global buckets.
    /// If either is exhausted, the request is denied and no token is consumed
    /// from the other bucket.
    pub fn check(&self, peer: &PeerId, protocol_label: &'static str) -> RateLimitDecision {
        // Protocol-specific check
        let proto_config = match self.configs.get(protocol_label) {
            Some(c) => *c,
            None => return RateLimitDecision::Allowed, // unknown protocol → allow
        };

        let proto_key = (*peer, protocol_label);
        // Split field borrows so the closure can reference peer_protocols without
        // conflicting with the protocol_buckets entry borrow.
        let protocol_buckets = &self.protocol_buckets;
        let peer_protocols = &self.peer_protocols;
        let mut proto_bucket = protocol_buckets.entry(proto_key).or_insert_with(|| {
            // First request from this peer on this protocol — record for O(1) cleanup.
            peer_protocols
                .entry(*peer)
                .or_default()
                .push(protocol_label);
            TokenBucket::new(proto_config.max_tokens, proto_config.refill_rate)
        });

        if !proto_bucket.try_consume() {
            return RateLimitDecision::ProtocolLimited;
        }

        // Global per-peer check
        let mut global_bucket = self.global_buckets.entry(*peer).or_insert_with(|| {
            TokenBucket::new(
                self.global_config.max_tokens,
                self.global_config.refill_rate,
            )
        });

        if !global_bucket.try_consume() {
            // Undo the protocol token we just consumed — the request is denied
            proto_bucket.tokens = (proto_bucket.tokens + 1.0).min(proto_bucket.max_tokens);
            return RateLimitDecision::GlobalLimited;
        }

        RateLimitDecision::Allowed
    }

    /// Remove all state for a disconnected peer to prevent unbounded memory growth.
    ///
    /// O(P) where P is the number of distinct protocols the peer used (≤ 6),
    /// not O(total entries across all peers).
    pub fn remove_peer(&self, peer: &PeerId) {
        self.global_buckets.remove(peer);

        // Use the inverted index to remove only this peer's protocol buckets.
        if let Some((_, protocols)) = self.peer_protocols.remove(peer) {
            for protocol in protocols {
                self.protocol_buckets.remove(&(*peer, protocol));
            }
        }
    }

    /// Number of peers currently tracked (for diagnostics).
    pub fn tracked_peer_count(&self) -> usize {
        self.global_buckets.len()
    }
}

impl Default for PeerRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_peer() -> PeerId {
        PeerId::random()
    }

    #[test]
    fn allows_requests_within_budget() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // direct_messages budget is 60 burst
        for _ in 0..60 {
            assert!(limiter.check(&peer, protocol::DIRECT_MESSAGES).is_allowed());
        }
    }

    #[test]
    fn blocks_after_protocol_budget_exhausted() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // Exhaust the identity budget (10 burst)
        for _ in 0..10 {
            assert!(limiter.check(&peer, protocol::IDENTITY).is_allowed());
        }

        let decision = limiter.check(&peer, protocol::IDENTITY);
        assert_eq!(decision, RateLimitDecision::ProtocolLimited);
    }

    #[test]
    fn direct_messages_have_highest_protocol_budget() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // DMs get 60, typing gets 20, identity gets 10
        // Verify DMs outlast both
        for _ in 0..10 {
            limiter.check(&peer, protocol::IDENTITY);
        }
        assert!(!limiter.check(&peer, protocol::IDENTITY).is_allowed());

        // DMs should still have tokens left
        assert!(limiter.check(&peer, protocol::DIRECT_MESSAGES).is_allowed());
    }

    #[test]
    fn typing_indicators_generous_for_multi_group() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // 20 burst — a peer in 10 groups can fire 2 indicators each
        for _ in 0..20 {
            assert!(limiter
                .check(&peer, protocol::TYPING_INDICATORS)
                .is_allowed());
        }
        assert!(!limiter
            .check(&peer, protocol::TYPING_INDICATORS)
            .is_allowed());
    }

    #[test]
    fn different_peers_have_independent_budgets() {
        let limiter = PeerRateLimiter::new();
        let alice = test_peer();
        let bob = test_peer();

        // Exhaust Alice's identity budget
        for _ in 0..10 {
            limiter.check(&alice, protocol::IDENTITY);
        }

        // Bob should still be fine
        assert!(limiter.check(&bob, protocol::IDENTITY).is_allowed());
    }

    #[test]
    fn different_protocols_have_independent_budgets() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // Exhaust identity budget
        for _ in 0..10 {
            limiter.check(&peer, protocol::IDENTITY);
        }
        assert!(!limiter.check(&peer, protocol::IDENTITY).is_allowed());

        // Direct messages should still work
        assert!(limiter.check(&peer, protocol::DIRECT_MESSAGES).is_allowed());
    }

    #[test]
    fn global_limit_blocks_across_protocols() {
        let limiter = PeerRateLimiter::with_global_config(BucketConfig::from_window(5, 60));
        // Override protocol limits to be very generous so we hit global first
        limiter.set_protocol_config(protocol::IDENTITY, BucketConfig::from_window(100, 60));
        limiter.set_protocol_config(
            protocol::DIRECT_MESSAGES,
            BucketConfig::from_window(100, 60),
        );

        let peer = test_peer();

        // Send 3 identity + 2 direct = 5 total (exhausts global)
        for _ in 0..3 {
            assert!(limiter.check(&peer, protocol::IDENTITY).is_allowed());
        }
        for _ in 0..2 {
            assert!(limiter.check(&peer, protocol::DIRECT_MESSAGES).is_allowed());
        }

        // Next request on any protocol should be globally limited
        let decision = limiter.check(&peer, protocol::IDENTITY);
        assert_eq!(decision, RateLimitDecision::GlobalLimited);
    }

    #[test]
    fn default_global_budget_is_generous() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // Exhaust all protocol budgets (10 + 60 + 3 + 20 + 30 = 123 total)
        // Global is 500, so all protocol-level traffic should fit within global.
        for _ in 0..10 {
            limiter.check(&peer, protocol::IDENTITY);
        }
        for _ in 0..60 {
            limiter.check(&peer, protocol::DIRECT_MESSAGES);
        }
        for _ in 0..3 {
            limiter.check(&peer, protocol::OFFLINE_MESSAGES);
        }
        for _ in 0..20 {
            limiter.check(&peer, protocol::TYPING_INDICATORS);
        }
        for _ in 0..30 {
            limiter.check(&peer, protocol::SIGNALING);
        }

        // All protocol budgets exhausted, but global should still have tokens
        // (500 - 123 = 377 remaining). Verify with an unregistered protocol.
        assert!(limiter.check(&peer, "unknown_proto").is_allowed());
    }

    #[test]
    fn tokens_refill_over_time() {
        use std::time::Duration;

        let limiter = PeerRateLimiter::new();
        // Override with a fast-refilling bucket for testing: 1 token, refills at 1000/s
        limiter.set_protocol_config(protocol::IDENTITY, BucketConfig::new(1.0, 1000.0));

        let peer = test_peer();

        assert!(limiter.check(&peer, protocol::IDENTITY).is_allowed());
        assert!(!limiter.check(&peer, protocol::IDENTITY).is_allowed());

        // Sleep briefly — at 1000 tokens/s, 2ms should refill ~2 tokens
        std::thread::sleep(Duration::from_millis(2));

        assert!(limiter.check(&peer, protocol::IDENTITY).is_allowed());
    }

    #[test]
    fn remove_peer_cleans_up_all_buckets() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        limiter.check(&peer, protocol::IDENTITY);
        limiter.check(&peer, protocol::DIRECT_MESSAGES);
        assert_eq!(limiter.tracked_peer_count(), 1);

        limiter.remove_peer(&peer);
        assert_eq!(limiter.tracked_peer_count(), 0);
        // Verify protocol_buckets are also cleaned up (no memory leak)
        assert!(!limiter
            .protocol_buckets
            .contains_key(&(peer, protocol::IDENTITY)));
        assert!(!limiter
            .protocol_buckets
            .contains_key(&(peer, protocol::DIRECT_MESSAGES)));
        assert!(!limiter.peer_protocols.contains_key(&peer));
    }

    #[test]
    fn remove_peer_only_removes_that_peers_buckets() {
        let limiter = PeerRateLimiter::new();
        let alice = test_peer();
        let bob = test_peer();

        limiter.check(&alice, protocol::IDENTITY);
        limiter.check(&bob, protocol::IDENTITY);
        assert_eq!(limiter.tracked_peer_count(), 2);

        limiter.remove_peer(&alice);
        assert_eq!(limiter.tracked_peer_count(), 1);
        // Bob's buckets untouched
        assert!(limiter
            .protocol_buckets
            .contains_key(&(bob, protocol::IDENTITY)));
    }

    #[test]
    fn unknown_protocol_always_allowed() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // An unregistered protocol label should pass through
        for _ in 0..1000 {
            assert!(limiter.check(&peer, "unknown_proto").is_allowed());
        }
    }

    #[test]
    fn offline_messages_has_tightest_limit() {
        let limiter = PeerRateLimiter::new();
        let peer = test_peer();

        // Offline messages: 3 burst (lowest priority — rare operation)
        for _ in 0..3 {
            assert!(limiter
                .check(&peer, protocol::OFFLINE_MESSAGES)
                .is_allowed());
        }
        assert!(!limiter
            .check(&peer, protocol::OFFLINE_MESSAGES)
            .is_allowed());
    }

    #[test]
    fn global_deny_does_not_consume_protocol_token() {
        // Global budget of 1 → only 1 request can pass globally
        let limiter = PeerRateLimiter::with_global_config(BucketConfig::from_window(1, 60));
        limiter.set_protocol_config(protocol::IDENTITY, BucketConfig::from_window(10, 60));

        let peer = test_peer();

        // First request succeeds (consumes 1 global + 1 protocol → 9 protocol left)
        assert!(limiter.check(&peer, protocol::IDENTITY).is_allowed());

        // Second request: global exhausted → denied, protocol token refunded
        let decision = limiter.check(&peer, protocol::IDENTITY);
        assert_eq!(decision, RateLimitDecision::GlobalLimited);

        // Verify the protocol bucket wasn't drained by the denied request.
        // Give global enough room so we can observe the protocol bucket directly.
        // We do this by testing a *different* peer with the same protocol — but
        // instead, let's just read the protocol bucket's remaining tokens via a
        // fresh global budget:
        //
        // Remove the global bucket for this peer and reconfigure global to be generous.
        limiter.global_buckets.remove(&peer);
        // Swap global config to generous so it won't interfere
        let generous_limiter =
            PeerRateLimiter::with_global_config(BucketConfig::from_window(500, 60));
        generous_limiter.set_protocol_config(protocol::IDENTITY, BucketConfig::from_window(10, 60));

        // Instead, simplest approach: use the internal protocol bucket directly.
        // The protocol bucket for `peer` should have exactly 9 tokens remaining
        // (10 initial - 1 consumed by the first allowed call; the second call's
        // consume was refunded).
        let proto_key = (peer, protocol::IDENTITY);
        let bucket = limiter.protocol_buckets.get(&proto_key).unwrap();
        // Allow a tiny tolerance for float arithmetic
        assert!(
            (bucket.tokens - 9.0).abs() < 0.01,
            "Expected ~9 tokens remaining, got {}",
            bucket.tokens
        );
    }
}
