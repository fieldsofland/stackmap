use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BranchId(pub Arc<str>);

impl BranchId {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self(name.into())
    }
}

impl std::fmt::Display for BranchId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiffStat {
    pub insertions: u64,
    pub deletions: u64,
    pub files: u32,
    pub binary_files: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DiffState {
    #[default]
    Loading,
    Ready(DiffStat),
    Unavailable(Arc<str>),
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum ConfiguredUpstream {
    #[default]
    None,
    Equal {
        reference: Arc<str>,
    },
    Ahead {
        reference: Arc<str>,
        ahead: u64,
    },
    Behind {
        reference: Arc<str>,
        behind: u64,
    },
    Diverged {
        reference: Arc<str>,
        ahead: u64,
        behind: u64,
    },
    Gone {
        reference: Arc<str>,
    },
    Unavailable {
        reference: Option<Arc<str>>,
        reason: Arc<str>,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RemoteRefEvidence {
    #[default]
    NotRequested,
    Checking,
    Contained {
        reference: Arc<str>,
        source_token: u64,
        checked_at: SystemTime,
    },
    LocalOnly {
        source_token: u64,
        checked_at: SystemTime,
    },
    Unavailable {
        reason: Arc<str>,
        source_token: Option<u64>,
        checked_at: SystemTime,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub title: Arc<str>,
    pub url: Arc<str>,
    pub status: PullRequestStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullRequestStatus {
    Open,
    Approved,
    Closed,
    Merged,
}

impl PullRequestStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Approved => "Approved",
            Self::Closed => "Closed",
            Self::Merged => "Merged",
        }
    }

    pub fn column_text(self, number: u64) -> String {
        match self {
            Self::Open => format!("#{number}"),
            Self::Approved => format!("✓#{number}"),
            Self::Merged => "Mrgd".to_owned(),
            Self::Closed => "Clsd".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphiteProvenance {
    DefinitelyUntracked,
    Tracked,
    Degraded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Branch {
    pub id: BranchId,
    pub oid: Arc<str>,
    pub parent: Option<BranchId>,
    pub diff_parent: Option<BranchId>,
    pub stack_root: BranchId,
    pub trunk: Option<BranchId>,
    pub graphite: GraphiteProvenance,
    pub committed_at: i64,
    pub current: bool,
    pub dirty: bool,
    pub worktree: Option<PathBuf>,
    pub configured_upstream: ConfiguredUpstream,
    pub remote_ref: RemoteRefEvidence,
    pub diff: DiffState,
    pub pr: Option<PullRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepositoryState {
    Ready,
    Detached,
    Unborn,
    OperationInProgress(Arc<str>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySnapshot {
    pub generation: u64,
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub repository_id: Arc<str>,
    pub default_trunk: Option<BranchId>,
    pub configured_trunks: Arc<[BranchId]>,
    pub trunks: Arc<[BranchId]>,
    pub graphite_children: Arc<[(BranchId, Arc<[BranchId]>)]>,
    pub branches: Arc<[Branch]>,
    pub branch_index: Arc<HashMap<BranchId, usize>>,
    pub stack_diffs: Arc<HashMap<BranchId, DiffState>>,
    pub state: RepositoryState,
    pub graphite_status: Arc<str>,
    pub stale_error: Option<Arc<str>>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ValidationError {
    DuplicateBranch(BranchId),
    MissingParent { branch: BranchId, parent: BranchId },
    Cycle(BranchId),
    WrongRoot { branch: BranchId, root: BranchId },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ValidationError {}

impl RepositorySnapshot {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut indexes = HashMap::with_capacity(self.branches.len());
        for (index, branch) in self.branches.iter().enumerate() {
            if indexes.insert(branch.id.clone(), index).is_some() {
                return Err(ValidationError::DuplicateBranch(branch.id.clone()));
            }
        }

        for branch in self.branches.iter() {
            if let Some(parent) = &branch.parent
                && !indexes.contains_key(parent)
            {
                return Err(ValidationError::MissingParent {
                    branch: branch.id.clone(),
                    parent: parent.clone(),
                });
            }

            let mut seen = HashSet::with_capacity(8);
            let mut cursor = Some(&branch.id);
            let mut discovered_root = &branch.id;
            while let Some(id) = cursor {
                if !seen.insert(id.clone()) {
                    return Err(ValidationError::Cycle(id.clone()));
                }
                discovered_root = id;
                cursor = indexes
                    .get(id)
                    .and_then(|index| self.branches[*index].parent.as_ref());
            }
            if discovered_root != &branch.stack_root {
                return Err(ValidationError::WrongRoot {
                    branch: branch.id.clone(),
                    root: branch.stack_root.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn branch(&self, id: &BranchId) -> Option<&Branch> {
        self.branch_index
            .get(id)
            .and_then(|index| self.branches.get(*index))
    }

    pub fn index_branches(branches: &[Branch]) -> Arc<HashMap<BranchId, usize>> {
        Arc::new(
            branches
                .iter()
                .enumerate()
                .map(|(index, branch)| (branch.id.clone(), index))
                .collect(),
        )
    }
}
