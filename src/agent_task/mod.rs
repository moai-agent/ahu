//! Core-owned agent task coordination, independent of terminal Host V1.
//!
//! Only a trusted core controller admits tasks or invokes execution adapters.
//! Consumers receive a scoped `v1::TaskService`, never a store or controller.
//! This is an in-process contract, not a network or plugin storage protocol.
pub mod inspection;
mod service;
mod store;
pub mod v1;
pub use service::{Controller, Session};

pub(crate) use service::read_page_ids;
