//! The crate error type, its exit categories, and the error-chain source.

use std::fmt;

/// Stable exit categories for command failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorKind {
    Usage = 2,
    UnknownAgent = 3,
    Prerequisite = 4,
    RunFailure = 5,
}

#[derive(Debug)]
/// A failure category and an actionable message for the person running ahu.
pub struct Error {
    kind: ErrorKind,
    message: String,
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Runtime validation, persistence, and subprocess failures default to run failure.
    /// User input and unavailable prerequisites are classified at their boundaries.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::RunFailure,
            cause: None,
        }
    }
}

impl Error {
    pub fn with_kind(mut self, kind: ErrorKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.cause.as_deref() {
            Some(cause) => Some(cause),
            None => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self {
            message: value.to_string(),
            kind: ErrorKind::RunFailure,
            cause: Some(Box::new(value)),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self {
            message: format!("invalid JSON: {value}"),
            kind: ErrorKind::RunFailure,
            cause: Some(Box::new(value)),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Build an [`Error`] with `format!` syntax.
#[macro_export]
macro_rules! bail {
    (kind: $kind:expr, $($arg:tt)*) => {
        return Err($crate::util::Error::new(format!($($arg)*)).with_kind($kind))
    };
    ($($arg:tt)*) => {
        return Err($crate::util::Error::new(format!($($arg)*)))
    };
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn io_conversions_keep_the_message_and_expose_the_original_error_as_source() {
        let error: Error = std::io::Error::other("boom").into();
        assert_eq!(error.to_string(), "boom");
        assert_eq!(error.kind(), ErrorKind::RunFailure);
        let source = error.source().expect("io source");
        assert!(source.downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn json_conversions_keep_the_message_and_expose_the_original_error_as_source() {
        let expected = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let error: Error = serde_json::from_str::<serde_json::Value>("{")
            .unwrap_err()
            .into();
        assert_eq!(error.to_string(), format!("invalid JSON: {expected}"));
        assert_eq!(error.kind(), ErrorKind::RunFailure);
        let source = error.source().expect("json source");
        assert!(source.downcast_ref::<serde_json::Error>().is_some());
    }

    #[test]
    fn new_errors_have_no_source() {
        let error = Error::new("plain failure");
        assert_eq!(error.to_string(), "plain failure");
        assert_eq!(error.kind(), ErrorKind::RunFailure);
        assert!(error.source().is_none());
    }
}
