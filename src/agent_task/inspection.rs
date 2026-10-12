//! Core inspection bridge. Legacy metadata stays in core; consumers receive V1.
//! Opening a view neither registers tasks nor reconciles/executes any effects.
use super::{Controller, v1::*};
use crate::{git::Repo, task::TaskListing, util};
use std::{collections::BTreeSet, path::PathBuf};

/// Trusted core factory, not a plugin capability. A view freezes validated
/// observations; reopen to refresh, reauthorizing the exact scope at that time.
pub struct Inspection {
    controller: Controller,
    listing: TaskListing,
}

impl Inspection {
    /// Load only exact demanded indexed IDs. The caller must authorize them first.
    pub(crate) fn open_exact(
        repo: &Repo,
        ids: &BTreeSet<String>,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> util::Result<Self> {
        let mut listing = TaskListing::default();
        for id in ids {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(error(Error::Denied));
            }
            listing
                .records
                .push(crate::commands::load_exact_indexed_task(repo, id)?);
        }
        Self::from_listing(repo, listing)
    }

    pub(crate) fn repository_identity(&self) -> &str {
        self.controller.repository_identity()
    }

    pub fn open(repo: &Repo) -> util::Result<Self> {
        Self::from_listing(repo, crate::task::list(repo)?)
    }

    fn from_listing(repo: &Repo, listing: TaskListing) -> util::Result<Self> {
        if listing
            .records
            .iter()
            .any(|(_, record)| record.repo_identity != repo.identity())
        {
            return Err(error(Error::Denied));
        }
        let records = listing
            .records
            .iter()
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>();
        let controller = Controller::inspection(repo, &records).map_err(error)?;
        Ok(Self {
            controller,
            listing,
        })
    }

    /// Core chooses plugin identity and exact grants; never populate these from
    /// untrusted tool arguments. Only the returned TaskService crosses the boundary.
    pub fn plugin_read_session(
        &self,
        plugin: &str,
        exact_tasks: BTreeSet<String>,
    ) -> Result<impl TaskService + use<>> {
        self.controller.plugin_read_session(plugin, exact_tasks)
    }

    fn into_listing(self) -> util::Result<TaskListing> {
        // Both local CLI and approved stdio MCP reads have repository read scope.
        // MCP job ownership is separate and remains enforced by mcp_tasks.
        let session = self.controller.session(
            Principal::Operator("local-inspection".into()),
            self.listing
                .records
                .iter()
                .map(|(_, r)| r.task_id.clone())
                .collect(),
            [Operation::Read].into(),
        );
        for (_, record) in &self.listing.records {
            session.get(VERSION, &record.task_id).map_err(error)?;
        }
        Ok(self.listing)
    }
}

/// Compatibility data is released only after shared TaskService authorization.
/// Response shaping stays with the existing CLI/MCP presentation layer.
pub fn list(repo: &Repo) -> util::Result<TaskListing> {
    Inspection::open(repo)?.into_listing()
}

pub fn get(repo: &Repo, selector: &str) -> util::Result<(PathBuf, crate::task::TaskRecord)> {
    let record = crate::commands::resolve_inspection(repo, selector)?;
    let listing = TaskListing {
        records: vec![record],
        ..Default::default()
    };
    Ok(Inspection::from_listing(repo, listing)?
        .into_listing()?
        .records
        .remove(0))
}

fn error(error: Error) -> util::Error {
    util::Error::new(format!("task inspection: {error}"))
}
