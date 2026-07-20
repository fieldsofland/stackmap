use std::path::PathBuf;
use std::sync::Arc;

use crate::model::{BranchId, ConfiguredUpstream, RepositoryState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBranch {
    pub id: BranchId,
    pub oid: Arc<str>,
    pub committed_at: i64,
    pub worktree: Option<PathBuf>,
    pub configured_upstream: ConfiguredUpstream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitInventory {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub repository_id: Arc<str>,
    pub branches: Vec<GitBranch>,
    pub current: Option<BranchId>,
    pub dirty: bool,
    pub state: RepositoryState,
    pub source_token: u64,
}
