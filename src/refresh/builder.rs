use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::adapters::git::GitAdapter;
use crate::adapters::graphite::read_topology;
use crate::model::{Branch, BranchId, DiffState, RepositorySnapshot};

pub struct SnapshotBuilder {
    git: GitAdapter,
    generation: u64,
}

impl SnapshotBuilder {
    pub fn new(git: GitAdapter) -> Self {
        Self { git, generation: 0 }
    }

    pub fn build(&mut self) -> Result<Arc<RepositorySnapshot>> {
        for _ in 0..3 {
            let inventory = self.git.inventory()?;
            let local: HashSet<_> = inventory
                .branches
                .iter()
                .map(|branch| branch.id.clone())
                .collect();
            let graphite = read_topology(&inventory.common_dir, &local);
            let after = self.git.inventory()?;
            let after_local: HashSet<_> = after
                .branches
                .iter()
                .map(|branch| branch.id.clone())
                .collect();
            let graphite_after = read_topology(&after.common_dir, &after_local);
            if inventory.source_token != after.source_token || graphite != graphite_after {
                continue;
            }

            self.generation = self.generation.wrapping_add(1);
            let mut branches = Vec::with_capacity(inventory.branches.len());
            for source in &inventory.branches {
                let recorded_parent = graphite.parents.get(&source.id).cloned();
                let trunk = graphite.trunk_by_branch.get(&source.id).cloned();
                let visual_parent = recorded_parent
                    .as_ref()
                    .and_then(|parent| (Some(parent) != trunk.as_ref()).then(|| parent.clone()));
                let root = stack_root(&source.id, &graphite.parents, trunk.as_ref());
                let provenance = graphite.provenance(&source.id);
                branches.push(Branch {
                    id: source.id.clone(),
                    oid: source.oid.clone(),
                    parent: visual_parent,
                    diff_parent: recorded_parent,
                    stack_root: root,
                    trunk,
                    graphite: provenance,
                    committed_at: source.committed_at,
                    current: inventory.current.as_ref() == Some(&source.id),
                    dirty: inventory.current.as_ref() == Some(&source.id) && inventory.dirty,
                    worktree: source.worktree.clone(),
                    diff: DiffState::Loading,
                    pr: None,
                });
            }
            branches.sort_by(|left, right| left.id.cmp(&right.id));
            let mut graphite_children: Vec<_> = graphite.child_order.into_iter().collect();
            graphite_children.sort_by(|left, right| left.0.cmp(&right.0));
            let snapshot = RepositorySnapshot {
                generation: self.generation,
                root: inventory.root,
                git_dir: inventory.git_dir,
                common_dir: inventory.common_dir,
                repository_id: inventory.repository_id,
                default_trunk: graphite.default_trunk,
                configured_trunks: graphite.configured_trunks,
                trunks: graphite.trunks,
                graphite_children: graphite_children.into(),
                branch_index: RepositorySnapshot::index_branches(&branches),
                branches: Arc::from(branches),
                state: inventory.state,
                graphite_status: graphite.status,
                stale_error: None,
            };
            snapshot.validate()?;
            return Ok(Arc::new(snapshot));
        }
        bail!("repository changed during three snapshot attempts")
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
