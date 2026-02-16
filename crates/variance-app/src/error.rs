use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Application error: {message}"))]
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
        assert_eq!(err.to_string(), "Application error: test error");
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
