//! Error compatibility exports shared with interactive plugins.
pub use ahu_host_api::{Error, ErrorKind, Result};

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
