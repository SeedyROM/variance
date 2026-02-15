use snafu::Snafu;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Media error: {message}"))]
    Media { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_error() {
        let err = Error::Media {
            message: "test error".to_string(),
        };
        assert_eq!(err.to_string(), "Media error: test error");
    }

    #[test]
    fn test_result_type() {
        let ok_result: Result<i32> = Ok(42);
        assert!(ok_result.is_ok());
        assert_eq!(ok_result.unwrap(), 42);

        let err_result: Result<i32> = Err(Error::Media {
            message: "failure".to_string(),
        });
        assert!(err_result.is_err());
    }
}
