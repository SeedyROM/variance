use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Bad request: {message}"))]
    BadRequest { message: String },

    #[snafu(display("Not found: {message}"))]
    NotFound { message: String },

    #[snafu(display("Unauthorized: {message}"))]
    Unauthorized { message: String },

    #[snafu(display("Forbidden: {message}"))]
    Forbidden { message: String },

    /// Returned when a Double Ratchet session must be initialized before sending.
    /// Callers should provide `recipient_x25519_key` to bootstrap the session.
    #[snafu(display("Session required: {message}"))]
    SessionRequired { message: String },

    #[snafu(display("Internal error: {message}"))]
    App { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error() {
        let err = Error::App {
            message: "test error".to_string(),
        };
        assert_eq!(err.to_string(), "Internal error: test error");
    }

    #[test]
    fn test_not_found_error() {
        let err = Error::NotFound {
            message: "user not found".to_string(),
        };
        assert_eq!(err.to_string(), "Not found: user not found");
    }

    #[test]
    fn test_bad_request_error() {
        let err = Error::BadRequest {
            message: "invalid key format".to_string(),
        };
        assert_eq!(err.to_string(), "Bad request: invalid key format");
    }

    #[test]
    fn test_session_required_error() {
        let err = Error::SessionRequired {
            message: "no session with peer".to_string(),
        };
        assert_eq!(err.to_string(), "Session required: no session with peer");
    }

    #[test]
    fn test_result_type() {
        let ok_result: Result<i32> = Ok(42);
        assert!(matches!(ok_result, Ok(42)));

        let err_result: Result<i32> = Err(Error::App {
            message: "failure".to_string(),
        });
        assert!(err_result.is_err());
    }
}
