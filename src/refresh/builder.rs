use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::adapters::git::GitAdapter;
use crate::adapters::graphite::{GraphiteTopology, read_topology};
use crate::model::topology::TopologyIndex;
use crate::model::{
    Branch, BranchId, DiffState, GraphiteHealth, GraphiteProvenance, RepositorySnapshot,
};

pub struct SnapshotBuilder {
    git: GitAdapter,
    generation: u64,
}

pub struct SnapshotCandidate {
    pub snapshot: Arc<RepositorySnapshot>,
    source_token: u64,
    graphite: GraphiteTopology,
}

impl SnapshotBuilder {
    pub fn new(git: GitAdapter) -> Self {
        Self { git, generation: 0 }
    }

    pub fn build(&mut self) -> Result<Arc<RepositorySnapshot>> {
        for _ in 0..3 {
            let candidate = self.build_candidate()?;
            if self.verify_candidate(&candidate)? {
                return Ok(candidate.snapshot);
            }
        }
        bail!("repository changed during three snapshot attempts")
    }

    pub fn build_candidate(&mut self) -> Result<SnapshotCandidate> {
        let inventory = self.git.inventory()?;
        let local: HashSet<_> = inventory
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&inventory.common_dir, &local);
        let source_token = inventory.source_token;
        self.generation = self.generation.wrapping_add(1);
        let oid_by_branch: HashMap<_, _> = inventory
            .branches
            .iter()
            .map(|branch| (branch.id.clone(), branch.oid.clone()))
            .collect();
        let mut branches = Vec::with_capacity(inventory.branches.len());
        for source in &inventory.branches {
            let recorded_parent = graphite.parents.get(&source.id).cloned();
            let trunk = graphite.trunk_by_branch.get(&source.id).cloned();
            let visual_parent = recorded_parent
                .as_ref()
                .and_then(|parent| (Some(parent) != trunk.as_ref()).then(|| parent.clone()));
            let root = stack_root(&source.id, &graphite.parents, trunk.as_ref());
            let provenance = graphite.provenance(&source.id);
            let graphite_health = match (&provenance, recorded_parent.as_ref()) {
                (GraphiteProvenance::Tracked, Some(parent)) => match oid_by_branch.get(parent) {
                    Some(_) => GraphiteHealth::NotRequested,
                    None => GraphiteHealth::Unavailable(Arc::from(
                        "recorded Graphite parent is not a local branch",
                    )),
                },
                (GraphiteProvenance::Tracked, None) => GraphiteHealth::Healthy,
                (GraphiteProvenance::Degraded, _) => {
                    GraphiteHealth::Unavailable(Arc::from("Graphite topology is degraded"))
                }
                (GraphiteProvenance::DefinitelyUntracked, _) => GraphiteHealth::NotTracked,
            };
            branches.push(Branch {
                id: source.id.clone(),
                oid: source.oid.clone(),
                parent: visual_parent,
                diff_parent: recorded_parent,
                stack_root: root,
                trunk,
                graphite: provenance,
                graphite_health,
                committed_at: source.committed_at,
                current: inventory.current.as_ref() == Some(&source.id),
                dirty: inventory.current.as_ref() == Some(&source.id) && inventory.dirty,
                worktree: source.worktree.clone(),
                configured_upstream: source.configured_upstream.clone(),
                remote_ref: crate::model::RemoteRefEvidence::NotRequested,
                diff: DiffState::Loading,
                pr: None,
                pr_lookup: crate::model::PullRequestLookup::NotRequested,
            });
        }
        branches.sort_by(|left, right| left.id.cmp(&right.id));
        let mut graphite_children: Vec<_> = graphite
            .child_order
            .iter()
            .map(|(parent, children)| (parent.clone(), children.clone()))
            .collect();
        graphite_children.sort_by(|left, right| left.0.cmp(&right.0));
        let mut snapshot = RepositorySnapshot {
            generation: self.generation,
            root: inventory.root,
            git_dir: inventory.git_dir,
            common_dir: inventory.common_dir,
            repository_id: inventory.repository_id,
            default_trunk: graphite.default_trunk.clone(),
            configured_trunks: graphite.configured_trunks.clone(),
            trunks: graphite.trunks.clone(),
            graphite_children: graphite_children.into(),
            branch_index: RepositorySnapshot::index_branches(&branches),
            branches: Arc::from(branches),
            stack_diffs: Arc::new(HashMap::new()),
            state: inventory.state,
            graphite_status: graphite.status.clone(),
            stale_error: None,
        };
        snapshot.validate()?;
        snapshot.stack_diffs = Arc::new(
            TopologyIndex::build(&snapshot)
                .stack_diff_endpoints()
                .into_iter()
                .map(|endpoints| (endpoints.stack_id, DiffState::Loading))
                .collect(),
        );
        Ok(SnapshotCandidate {
            snapshot: Arc::new(snapshot),
            source_token,
            graphite,
        })
    }

    pub fn verify_candidate(&self, candidate: &SnapshotCandidate) -> Result<bool> {
        let after = self.git.inventory()?;
        let after_local: HashSet<_> = after
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite_after = read_topology(&after.common_dir, &after_local);
        Ok(candidate.source_token == after.source_token && candidate.graphite == graphite_after)
    }
}

fn stack_root(
    branch: &BranchId,
    parents: &HashMap<BranchId, BranchId>,
    trunk: Option<&BranchId>,
) -> BranchId {
    if Some(branch) == trunk {
        return branch.clone();
    }
    let mut cursor = branch;
    let mut root = branch;
    let mut seen = HashSet::new();
    while let Some(parent) = parents.get(cursor) {
        if !seen.insert(cursor.clone()) || Some(parent) == trunk {
            break;
        }
        root = parent;
        cursor = parent;
    }
    root.clone()
}
