//! Shared Rust host contract and versioned executable protocol. No dynamic ABI.
pub mod error;
pub mod protocol;
pub use error::{Error, ErrorKind, Result};
