use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use super::super::command::{CommandError, CommandOutput, run_bounded};
use super::super::graphite::raw_branch_metadata_presence;
use super::GitAdapter;
use crate::model::{BranchId, GraphiteProvenance};

pub(super) type DeleteContractCache = Arc<Mutex<Option<Result<(), Arc<str>>>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRequest {
    pub branch: BranchId,
    pub expected_oid: Arc<str>,
    pub expected_provenance: GraphiteProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    Unchanged,
    Inconsistent,
}

pub(super) trait DeleteCommandRunner {
    fn run(
        &self,
        args: &[&OsStr],
        cwd: &Path,
        timeout: Duration,
        output_limit: usize,
    ) -> std::result::Result<CommandOutput, CommandError>;
}

pub(super) struct SystemDeleteCommandRunner;

pub(super) trait DeletePostStateReader {
    fn exact_local_ref(&self, adapter: &GitAdapter, branch: &BranchId) -> Result<Option<Arc<str>>>;
    fn metadata_present(&self, common_dir: &Path, branch: &BranchId) -> Result<bool>;
}

pub(super) struct SystemDeletePostStateReader;

impl DeletePostStateReader for SystemDeletePostStateReader {
    fn exact_local_ref(&self, adapter: &GitAdapter, branch: &BranchId) -> Result<Option<Arc<str>>> {
        adapter.exact_local_ref(branch)
    }

    fn metadata_present(&self, common_dir: &Path, branch: &BranchId) -> Result<bool> {
        raw_branch_metadata_presence(common_dir, branch)
    }
}

impl DeleteCommandRunner for SystemDeleteCommandRunner {
    fn run(
        &self,
        args: &[&OsStr],
        cwd: &Path,
        timeout: Duration,
        output_limit: usize,
    ) -> std::result::Result<CommandOutput, CommandError> {
        run_bounded(
            OsStr::new("gt"),
            args.iter().copied(),
            cwd,
            timeout,
            output_limit,
        )
    }
}
