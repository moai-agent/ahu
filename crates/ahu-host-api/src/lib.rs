//! Shared Rust host contract and versioned executable protocol. No dynamic ABI.
pub mod error;
pub mod protocol;
pub mod v1;
pub use error::{Error, ErrorKind, Result};
