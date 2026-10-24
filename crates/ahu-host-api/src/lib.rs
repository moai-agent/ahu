//! Shared Rust host contract and versioned executable protocol. No dynamic ABI.
pub mod error;
pub mod protocol;
pub mod v2;
pub use error::{Error, ErrorKind, Result};
