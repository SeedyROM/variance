use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;
use variance_proto::messaging_proto::TypingIndicator;

/// Minimum interval between outbound typing-start P2P messages per recipient.
/// Repeated calls within this window are silently dropped to prevent
/// per-keystroke network traffic and potential DoS.
const OUTBOUND_COOLDOWN_MS: u64 = 3000;

/// Minimum sustained-composition time (ms) before the first group typing
/// indicator is emitted. Prevents brief/accidental keystrokes from
/// broadcasting activity to the entire group.
const COMPOSE_THRESHOLD_MS: u64 = 1000;

/// Typing indicator handler
///
/// Manages ephemeral typing state for direct and group conversations.
/// Typing indicators are not persisted and have automatic timeout.
pub struct TypingHandler {
    /// Local DID
    local_did: String,

    /// Active typing states: key is "recipient_did" or "group:{group_id}"
    /// Value is (sender_did, timestamp)
    typing_states: Arc<DashMap<String, Vec<(String, i64)>>>,

    /// Timeout for typing indicators in milliseconds (default: 5 seconds)
    timeout_ms: i64,

    /// Tracks the last time we sent a typing-start for each recipient.
    /// Prevents flooding the P2P network when the UI fires on every keystroke.
    last_outbound_start: Arc<DashMap<String, Instant>>,

    /// Tracks when the user first started composing in each group conversation.
    /// The first typing indicator is suppressed until [`COMPOSE_THRESHOLD_MS`]
    /// has elapsed, preventing brief/accidental keystrokes from broadcasting.
    compose_start: Arc<DashMap<String, Instant>>,
}

impl TypingHandler {
    /// Create a new typing handler
    pub fn new(local_did: String) -> Self {
        Self {
            local_did,
            typing_states: Arc::new(DashMap::new()),
            timeout_ms: 5000, // 5 seconds
            last_outbound_start: Arc::new(DashMap::new()),
            compose_start: Arc::new(DashMap::new()),
        }
    }

    /// Create a new typing handler with custom timeout
    pub fn with_timeout(local_did: String, timeout_ms: i64) -> Self {
        Self {
            local_did,
            typing_states: Arc::new(DashMap::new()),
            timeout_ms,
            last_outbound_start: Arc::new(DashMap::new()),
            compose_start: Arc::new(DashMap::new()),
        }
    }

    /// Send typing indicator for direct message
    pub fn send_typing_direct(&self, recipient_did: String, is_typing: bool) -> TypingIndicator {
        let timestamp = chrono::Utc::now().timestamp_millis();

        TypingIndicator {
            sender_did: self.local_did.clone(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    recipient_did,
                ),
            ),
            is_typing,
            timestamp,
        }
    }

    /// Send typing indicator for group
    pub fn send_typing_group(&self, group_id: String, is_typing: bool) -> TypingIndicator {
        let timestamp = chrono::Utc::now().timestamp_millis();

        TypingIndicator {
            sender_did: self.local_did.clone(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(group_id),
            ),
            is_typing,
            timestamp,
        }
    }

    /// Rate-limited typing-start for direct messages.
    ///
    /// Returns `Some(indicator)` only if enough time has elapsed since the last
    /// outbound typing-start for this recipient. Repeated calls within the
    /// cooldown window return `None`, preventing per-keystroke P2P traffic.
    pub fn try_start_typing_direct(&self, recipient_did: String) -> Option<TypingIndicator> {
        if self.is_within_cooldown(&recipient_did) {
            return None;
        }
        self.last_outbound_start
            .insert(recipient_did.clone(), Instant::now());
        Some(self.send_typing_direct(recipient_did, true))
    }

    /// Rate-limited typing-start for group messages.
    ///
    /// In addition to the standard outbound cooldown, group typing indicators
    /// enforce a sustained-composition threshold: the very first indicator for
    /// a group is suppressed until the user has been composing for at least
    /// [`COMPOSE_THRESHOLD_MS`]. This prevents brief or accidental keystrokes
    /// from broadcasting activity to the entire group (privacy mitigation).
    pub fn try_start_typing_group(&self, group_id: String) -> Option<TypingIndicator> {
        let key = format!("group:{}", group_id);

        // Enforce sustained-composition threshold on first indicator
        let now = Instant::now();
        let compose_elapsed = if let Some(start) = self.compose_start.get(&key) {
            start.elapsed().as_millis()
        } else {
            // First keystroke for this group — record and suppress
            self.compose_start.insert(key.clone(), now);
            return None;
        };

        if compose_elapsed < u128::from(COMPOSE_THRESHOLD_MS) {
            // Still within the compose threshold — suppress
            return None;
        }

        // Past threshold — apply normal outbound cooldown
        if self.is_within_cooldown(&key) {
            return None;
        }
        self.last_outbound_start.insert(key, Instant::now());
        Some(self.send_typing_group(group_id, true))
    }

    /// Clear the outbound cooldown for a recipient so the next typing-start
    /// will be sent immediately. Called when the user sends a message or
    /// explicitly stops typing.
    pub fn clear_cooldown(&self, recipient: &str) {
        self.last_outbound_start.remove(recipient);
    }

    /// Clear the compose-start timestamp for a group conversation.
    ///
    /// Call this when the user sends a message or the input is cleared. The
    /// next keystroke will restart the sustained-composition threshold.
    pub fn clear_compose_start(&self, group_key: &str) {
        self.compose_start.remove(group_key);
    }

    /// Returns `true` if we sent a typing-start for `key` within the cooldown window.
    fn is_within_cooldown(&self, key: &str) -> bool {
        if let Some(last) = self.last_outbound_start.get(key) {
            last.elapsed().as_millis() < u128::from(OUTBOUND_COOLDOWN_MS)
        } else {
            false
        }
    }

    /// Receive a typing indicator
    ///
    /// Updates the typing state and automatically expires old indicators.
    pub fn receive_indicator(&self, indicator: TypingIndicator) {
        // Determine key based on recipient type
        let key = match &indicator.recipient {
            Some(variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                did,
            )) => did.clone(),
            Some(variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                group_id,
            )) => format!("group:{}", group_id),
            None => return, // Invalid indicator
        };

        if indicator.is_typing {
            // Add or update typing state
            self.typing_states
                .entry(key.clone())
                .or_default()
                .retain(|(did, _)| did != &indicator.sender_did);

            self.typing_states
                .get_mut(&key)
                .unwrap()
                .push((indicator.sender_did.clone(), indicator.timestamp));
        } else {
            // Remove typing state
            if let Some(mut entry) = self.typing_states.get_mut(&key) {
                entry.retain(|(did, _)| did != &indicator.sender_did);
                if entry.is_empty() {
                    drop(entry);
                    self.typing_states.remove(&key);
                }
            }
        }

        // Clean up expired indicators
        self.cleanup_expired(&key);
    }

    /// Get who is currently typing in a conversation
    pub fn get_typing_users_direct(&self, peer_did: &str) -> Vec<String> {
        self.cleanup_expired(peer_did);

        self.typing_states
            .get(peer_did)
            .map(|entry| entry.iter().map(|(did, _)| did.clone()).collect())
            .unwrap_or_default()
    }

    /// Get who is currently typing in a group
    pub fn get_typing_users_group(&self, group_id: &str) -> Vec<String> {
        let key = format!("group:{}", group_id);
        self.cleanup_expired(&key);

        self.typing_states
            .get(&key)
            .map(|entry| entry.iter().map(|(did, _)| did.clone()).collect())
            .unwrap_or_default()
    }

    /// Clean up expired typing indicators for a specific key
    fn cleanup_expired(&self, key: &str) {
        let now = chrono::Utc::now().timestamp_millis();

        if let Some(mut entry) = self.typing_states.get_mut(key) {
            entry.retain(|(_, timestamp)| now - timestamp < self.timeout_ms);

            if entry.is_empty() {
                drop(entry);
                self.typing_states.remove(key);
            }
        }
    }

    /// Clean up all expired typing indicators
    ///
    /// Should be called periodically to prevent memory buildup.
    pub fn cleanup_all_expired(&self) {
        let now = chrono::Utc::now().timestamp_millis();

        let keys_to_check: Vec<String> = self
            .typing_states
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for key in keys_to_check {
            if let Some(mut entry) = self.typing_states.get_mut(&key) {
                entry.retain(|(_, timestamp)| now - timestamp < self.timeout_ms);

                if entry.is_empty() {
                    drop(entry);
                    self.typing_states.remove(&key);
                }
            }
        }
    }

    /// Get timeout in milliseconds
    pub fn timeout_ms(&self) -> i64 {
        self.timeout_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_handler() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        assert_eq!(handler.local_did, "did:variance:alice");
        assert_eq!(handler.timeout_ms, 5000);
    }

    #[test]
    fn test_custom_timeout() {
        let handler = TypingHandler::with_timeout("did:variance:alice".to_string(), 10000);

        assert_eq!(handler.timeout_ms(), 10000);
    }

    #[test]
    fn test_send_typing_direct() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        let indicator = handler.send_typing_direct("did:variance:bob".to_string(), true);

        assert_eq!(indicator.sender_did, "did:variance:alice");
        assert!(indicator.is_typing);
        match indicator.recipient {
            Some(variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                did,
            )) => {
                assert_eq!(did, "did:variance:bob");
            }
            _ => panic!("Expected RecipientDid"),
        }
    }

    #[test]
    fn test_send_typing_group() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        let indicator = handler.send_typing_group("group123".to_string(), true);

        assert_eq!(indicator.sender_did, "did:variance:alice");
        assert!(indicator.is_typing);
        match indicator.recipient {
            Some(variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                group_id,
            )) => {
                assert_eq!(group_id, "group123");
            }
            _ => panic!("Expected GroupId"),
        }
    }

    #[test]
    fn test_receive_indicator_direct() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        let indicator = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    "did:variance:alice".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator);

        let typing_users = handler.get_typing_users_direct("did:variance:alice");
        assert_eq!(typing_users.len(), 1);
        assert_eq!(typing_users[0], "did:variance:bob");
    }

    #[test]
    fn test_receive_indicator_group() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        let indicator = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                    "group123".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator);

        let typing_users = handler.get_typing_users_group("group123");
        assert_eq!(typing_users.len(), 1);
        assert_eq!(typing_users[0], "did:variance:bob");
    }

    #[test]
    fn test_typing_stop() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // Bob starts typing
        let indicator = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    "did:variance:alice".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator);

        let typing_users = handler.get_typing_users_direct("did:variance:alice");
        assert_eq!(typing_users.len(), 1);

        // Bob stops typing
        let indicator = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    "did:variance:alice".to_string(),
                ),
            ),
            is_typing: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator);

        let typing_users = handler.get_typing_users_direct("did:variance:alice");
        assert_eq!(typing_users.len(), 0);
    }

    #[tokio::test]
    async fn test_timeout_expiration() {
        let handler = TypingHandler::with_timeout("did:variance:alice".to_string(), 100); // 100ms timeout

        let indicator = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    "did:variance:alice".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator);

        // Should have typing user immediately
        let typing_users = handler.get_typing_users_direct("did:variance:alice");
        assert_eq!(typing_users.len(), 1);

        // Wait for timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Should be expired now
        let typing_users = handler.get_typing_users_direct("did:variance:alice");
        assert_eq!(typing_users.len(), 0);
    }

    #[test]
    fn test_multiple_typing_users() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // Bob starts typing
        let indicator1 = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                    "group123".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator1);

        // Charlie starts typing
        let indicator2 = TypingIndicator {
            sender_did: "did:variance:charlie".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                    "group123".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator2);

        let typing_users = handler.get_typing_users_group("group123");
        assert_eq!(typing_users.len(), 2);
        assert!(typing_users.contains(&"did:variance:bob".to_string()));
        assert!(typing_users.contains(&"did:variance:charlie".to_string()));
    }

    #[tokio::test]
    async fn test_cleanup_all_expired() {
        let handler = TypingHandler::with_timeout("did:variance:alice".to_string(), 100);

        // Add typing indicators for different conversations
        let indicator1 = TypingIndicator {
            sender_did: "did:variance:bob".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::RecipientDid(
                    "did:variance:alice".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        let indicator2 = TypingIndicator {
            sender_did: "did:variance:charlie".to_string(),
            recipient: Some(
                variance_proto::messaging_proto::typing_indicator::Recipient::GroupId(
                    "group123".to_string(),
                ),
            ),
            is_typing: true,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        handler.receive_indicator(indicator1);
        handler.receive_indicator(indicator2);

        assert_eq!(handler.typing_states.len(), 2);

        // Wait for timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Clean up all
        handler.cleanup_all_expired();

        assert_eq!(handler.typing_states.len(), 0);
    }

    #[test]
    fn test_try_start_typing_direct_cooldown() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // First call should produce an indicator
        let first = handler.try_start_typing_direct("did:variance:bob".to_string());
        assert!(first.is_some());
        assert!(first.unwrap().is_typing);

        // Immediate second call should be suppressed (within cooldown)
        let second = handler.try_start_typing_direct("did:variance:bob".to_string());
        assert!(second.is_none());

        // Different recipient should still work
        let other = handler.try_start_typing_direct("did:variance:charlie".to_string());
        assert!(other.is_some());
    }

    #[tokio::test]
    async fn test_try_start_typing_group_compose_threshold() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // First call should be suppressed (compose threshold not yet met)
        let first = handler.try_start_typing_group("group123".to_string());
        assert!(
            first.is_none(),
            "first keystroke should be suppressed by compose threshold"
        );

        // Immediate second call should still be suppressed (under 1s threshold)
        let second = handler.try_start_typing_group("group123".to_string());
        assert!(
            second.is_none(),
            "second keystroke under threshold should be suppressed"
        );

        // Wait past the compose threshold (1s)
        tokio::time::sleep(tokio::time::Duration::from_millis(1050)).await;

        // Now it should fire
        let after_threshold = handler.try_start_typing_group("group123".to_string());
        assert!(
            after_threshold.is_some(),
            "should fire after compose threshold"
        );
        assert!(after_threshold.unwrap().is_typing);

        // Immediately after should be suppressed by outbound cooldown
        let cooldown_suppressed = handler.try_start_typing_group("group123".to_string());
        assert!(
            cooldown_suppressed.is_none(),
            "should be suppressed by outbound cooldown"
        );
    }

    #[tokio::test]
    async fn test_try_start_typing_group_different_groups_independent() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // Start composing in group123
        handler.try_start_typing_group("group123".to_string());

        // Start composing in group456 — should also be suppressed independently
        let other = handler.try_start_typing_group("group456".to_string());
        assert!(
            other.is_none(),
            "different group should also start in threshold"
        );

        // Wait past threshold
        tokio::time::sleep(tokio::time::Duration::from_millis(1050)).await;

        // Both should now fire
        let g1 = handler.try_start_typing_group("group123".to_string());
        assert!(g1.is_some());
        let g2 = handler.try_start_typing_group("group456".to_string());
        assert!(g2.is_some());
    }

    #[tokio::test]
    async fn test_clear_compose_start_resets_threshold() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        // Start composing
        handler.try_start_typing_group("group123".to_string());

        // Wait past threshold
        tokio::time::sleep(tokio::time::Duration::from_millis(1050)).await;

        // Fire once
        let first = handler.try_start_typing_group("group123".to_string());
        assert!(first.is_some());

        // Clear compose start (simulates user sent message or cleared input)
        handler.clear_compose_start("group:group123");

        // Clear cooldown too so we can test the threshold again
        handler.clear_cooldown("group:group123");

        // Next call should be suppressed again (threshold restarted)
        let after_clear = handler.try_start_typing_group("group123".to_string());
        assert!(
            after_clear.is_none(),
            "should be suppressed after compose_start cleared"
        );
    }

    #[test]
    fn test_clear_cooldown_allows_resend() {
        let handler = TypingHandler::new("did:variance:alice".to_string());

        let first = handler.try_start_typing_direct("did:variance:bob".to_string());
        assert!(first.is_some());

        // Suppressed while in cooldown
        assert!(handler
            .try_start_typing_direct("did:variance:bob".to_string())
            .is_none());

        // Clear cooldown (simulates stop-typing or message-sent)
        handler.clear_cooldown("did:variance:bob");

        // Should work again immediately
        let after_clear = handler.try_start_typing_direct("did:variance:bob".to_string());
        assert!(after_clear.is_some());
    }
}
