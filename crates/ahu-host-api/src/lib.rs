//! Shared Rust contract for compiled ahu interactive plugins. No dynamic ABI.
pub mod error;
pub mod v1;
pub use error::{Error, ErrorKind, Result};
