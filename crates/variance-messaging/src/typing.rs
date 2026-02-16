use dashmap::DashMap;
use std::sync::Arc;
use variance_proto::messaging_proto::TypingIndicator;

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
}

impl TypingHandler {
    /// Create a new typing handler
    pub fn new(local_did: String) -> Self {
        Self {
            local_did,
            typing_states: Arc::new(DashMap::new()),
            timeout_ms: 5000, // 5 seconds
        }
    }

    /// Create a new typing handler with custom timeout
    pub fn with_timeout(local_did: String, timeout_ms: i64) -> Self {
        Self {
            local_did,
            typing_states: Arc::new(DashMap::new()),
            timeout_ms,
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
}
