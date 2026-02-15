/// Custom libp2p protocols for Variance
///
/// This module provides utilities for creating custom request-response protocols
/// using libp2p's request_response framework.
use libp2p::request_response;

/// Protocol version constants
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Helper to create a request-response protocol configuration
pub fn create_protocol_config(_protocol_name: &str) -> request_response::ProtocolSupport {
    request_response::ProtocolSupport::Full
}

/// Helper to format a protocol name with version
pub fn protocol_name(base: &str) -> String {
    format!("/variance/{}/{}", base, PROTOCOL_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_name() {
        assert_eq!(protocol_name("identity"), "/variance/identity/1.0.0");
        assert_eq!(protocol_name("messaging"), "/variance/messaging/1.0.0");
    }
}
