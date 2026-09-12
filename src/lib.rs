//! ahu — the moai-agent launcher.
//!
//! ahu launches repository-defined agents in isolated Git worktrees and
//! organises their interactive sessions in cmux. Its contract, in one place:
//!
//!   - a named agent's harness, model, and system prompt come from the
//!     repository and are used exactly as configured, or the launch fails;
//!   - one policy per project, with no personal profiles or per-machine
//!     fallbacks that change what gets selected;
//!   - the harness owns its own conventions for skills, memory, and settings,
//!     and ahu preserves them in place;
//!   - what ahu cannot see or cannot enforce is said out loud rather than
//!     papered over.

pub mod agent;
pub mod catalog;
pub mod cli;
pub mod cmux;
pub mod commands;
pub mod config;
pub mod drift;
pub mod explain;
pub mod git;
pub mod harness;
pub mod hooks;
pub mod hygiene;
pub mod inventory;
pub mod launch;
pub mod launcher;
pub mod onboard;
pub mod orchestration;
pub mod selection;
pub mod snapshot;
pub mod state;
pub mod task;
pub mod util;
