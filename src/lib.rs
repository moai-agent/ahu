//! ahu — the moai-agent launcher.
//!
//! ahu launches repository-defined agents in isolated Git worktrees and
//! organises their interactive sessions in cmux. Its contract, in one place:
//!
//!   - a named agent's harness, model, and instructions come from the
//!     repository and are used exactly as configured, or the launch fails. The
//!     harness and model are pinned by real flags; the instructions are
//!     delivered as prompt text, because no harness offers ahu an instruction
//!     channel it can verify, and ahu reports that as a gap rather than as
//!     enforcement;
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
pub mod knowledge;
pub mod launch;
pub mod launcher;
pub mod onboard;
pub mod orchestration;
pub mod selection;
pub mod snapshot;
pub mod state;
pub mod style;
pub mod task;
pub mod util;

pub mod headless;

pub mod broker;

pub mod native;
