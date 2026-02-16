use crate::error::*;
use dashmap::DashMap;
use std::sync::Arc;
use ulid::Ulid;
use variance_proto::media_proto::{CallState, CallStatus, CallType};

/// Call manager
///
/// Manages active call states and lifecycle.
pub struct CallManager {
    /// Local DID
    local_did: String,

    /// Active calls indexed by call ID
    calls: Arc<DashMap<String, Call>>,
}

/// Call state
#[derive(Debug, Clone)]
pub struct Call {
    /// Unique call ID
    pub id: String,

    /// Participant DIDs
    pub participants: Vec<String>,

    /// Call type (audio, video, screen share)
    pub call_type: CallType,

    /// Current status
    pub status: CallStatus,

    /// Start timestamp (milliseconds)
    pub started_at: i64,

    /// End timestamp (milliseconds)
    pub ended_at: Option<i64>,
}

impl CallManager {
    /// Create a new call manager
    pub fn new(local_did: String) -> Self {
        Self {
            local_did,
            calls: Arc::new(DashMap::new()),
        }
    }

    /// Create a new outgoing call
    pub fn create_call(&self, recipient_did: String, call_type: CallType) -> Call {
        let call_id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp_millis();

        let call = Call {
            id: call_id.clone(),
            participants: vec![self.local_did.clone(), recipient_did],
            call_type,
            status: CallStatus::Ringing,
            started_at: now,
            ended_at: None,
        };

        self.calls.insert(call_id, call.clone());
        call
    }

    /// Accept an incoming call
    pub fn accept_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Ringing {
            return Err(Error::InvalidState {
                message: format!("Cannot accept call in state {:?}", call_ref.status),
            });
        }

        call_ref.status = CallStatus::Connecting;
        Ok(call_ref.clone())
    }

    /// Reject an incoming call
    pub fn reject_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Ringing {
            return Err(Error::InvalidState {
                message: format!("Cannot reject call in state {:?}", call_ref.status),
            });
        }

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Ended;
        call_ref.ended_at = Some(now);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Mark call as active (connected)
    pub fn activate_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Connecting {
            return Err(Error::InvalidState {
                message: format!("Cannot activate call in state {:?}", call_ref.status),
            });
        }

        call_ref.status = CallStatus::Active;
        Ok(call_ref.clone())
    }

    /// End an active call
    pub fn end_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Ended;
        call_ref.ended_at = Some(now);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Mark call as failed
    pub fn fail_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Failed;
        call_ref.ended_at = Some(now);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Get call by ID
    pub fn get_call(&self, call_id: &str) -> Option<Call> {
        self.calls.get(call_id).map(|r| r.clone())
    }

    /// List all active calls
    pub fn list_active_calls(&self) -> Vec<Call> {
        self.calls
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Register an incoming call
    pub fn register_incoming_call(
        &self,
        call_id: String,
        caller_did: String,
        call_type: CallType,
    ) -> Call {
        let now = chrono::Utc::now().timestamp_millis();

        let call = Call {
            id: call_id.clone(),
            participants: vec![caller_did, self.local_did.clone()],
            call_type,
            status: CallStatus::Ringing,
            started_at: now,
            ended_at: None,
        };

        self.calls.insert(call_id, call.clone());
        call
    }

    /// Convert Call to protobuf format
    pub fn call_to_proto(&self, call: &Call) -> CallState {
        CallState {
            call_id: call.id.clone(),
            participants: call.participants.clone(),
            call_type: call.call_type.into(),
            status: call.status.into(),
            started_at: call.started_at,
            ended_at: call.ended_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_manager() {
        let manager = CallManager::new("did:variance:alice".to_string());

        assert_eq!(manager.local_did, "did:variance:alice");
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_create_call() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        assert!(!call.id.is_empty());
        assert_eq!(call.participants.len(), 2);
        assert_eq!(call.participants[0], "did:variance:alice");
        assert_eq!(call.participants[1], "did:variance:bob");
        assert_eq!(call.call_type, CallType::Audio);
        assert_eq!(call.status, CallStatus::Ringing);
        assert_eq!(manager.list_active_calls().len(), 1);
    }

    #[test]
    fn test_accept_call() {
        let manager = CallManager::new("did:variance:bob".to_string());

        let call = manager.register_incoming_call(
            "call123".to_string(),
            "did:variance:alice".to_string(),
            CallType::Video,
        );

        assert_eq!(call.status, CallStatus::Ringing);

        let accepted = manager.accept_call(&call.id).unwrap();
        assert_eq!(accepted.status, CallStatus::Connecting);
    }

    #[test]
    fn test_reject_call() {
        let manager = CallManager::new("did:variance:bob".to_string());

        let call = manager.register_incoming_call(
            "call123".to_string(),
            "did:variance:alice".to_string(),
            CallType::Audio,
        );

        let rejected = manager.reject_call(&call.id).unwrap();
        assert_eq!(rejected.status, CallStatus::Ended);
        assert!(rejected.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_activate_call() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        manager.accept_call(&call.id).unwrap();

        let active = manager.activate_call(&call.id).unwrap();
        assert_eq!(active.status, CallStatus::Active);
    }

    #[test]
    fn test_end_call() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        manager.accept_call(&call.id).unwrap();
        manager.activate_call(&call.id).unwrap();

        let ended = manager.end_call(&call.id).unwrap();
        assert_eq!(ended.status, CallStatus::Ended);
        assert!(ended.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_fail_call() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        let failed = manager.fail_call(&call.id).unwrap();
        assert_eq!(failed.status, CallStatus::Failed);
        assert!(failed.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_get_call() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        let retrieved = manager.get_call(&call.id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, call.id);

        let not_found = manager.get_call("nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_invalid_state_transitions() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        // Cannot activate a ringing call
        let result = manager.activate_call(&call.id);
        assert!(result.is_err());

        manager.accept_call(&call.id).unwrap();

        // Cannot accept a connecting call
        let result = manager.accept_call(&call.id);
        assert!(result.is_err());

        // Cannot reject a connecting call
        let result = manager.reject_call(&call.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_call_not_found() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let result = manager.accept_call("nonexistent");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::CallNotFound { .. }));
    }

    #[test]
    fn test_call_to_proto() {
        let manager = CallManager::new("did:variance:alice".to_string());

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Video);

        let proto = manager.call_to_proto(&call);

        assert_eq!(proto.call_id, call.id);
        assert_eq!(proto.participants, call.participants);
        assert_eq!(proto.call_type, CallType::Video as i32);
        assert_eq!(proto.status, CallStatus::Ringing as i32);
        assert_eq!(proto.started_at, call.started_at);
    }
}
