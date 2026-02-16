use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Call not found: {call_id}"))]
    CallNotFound { call_id: String },

    #[snafu(display("Invalid call state: {message}"))]
    InvalidState { message: String },

    #[snafu(display("Invalid signature for call: {call_id}"))]
    InvalidSignature { call_id: String },

    #[snafu(display("Signaling error: {message}"))]
    Signaling { message: String },

    #[snafu(display("Protocol error: {source}"))]
    Protocol { source: prost::DecodeError },

    #[snafu(display("Media error: {message}"))]
    Media { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call_not_found_error() {
        let err = Error::CallNotFound {
            call_id: "call123".to_string(),
        };
        assert_eq!(err.to_string(), "Call not found: call123");
    }

    #[test]
    fn test_invalid_state_error() {
        let err = Error::InvalidState {
            message: "cannot accept ended call".to_string(),
        };
        assert_eq!(err.to_string(), "Invalid call state: cannot accept ended call");
    }

    #[test]
    fn test_invalid_signature_error() {
        let err = Error::InvalidSignature {
            call_id: "call123".to_string(),
        };
        assert_eq!(err.to_string(), "Invalid signature for call: call123");
    }

    #[test]
    fn test_result_type() {
        let ok_result: Result<i32> = Ok(42);
        assert!(matches!(ok_result, Ok(42)));

        let err_result: Result<i32> = Err(Error::Media {
            message: "failure".to_string(),
        });
        assert!(err_result.is_err());
    }
}
