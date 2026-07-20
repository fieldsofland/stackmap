use std::collections::HashMap;

use super::super::BranchId;
use super::Emphasis;

#[derive(Clone, Debug)]
pub(super) struct Node {
    pub(super) id: BranchId,
    pub(super) parent: Option<BranchId>,
    pub(super) trunk: Option<BranchId>,
    pub(super) committed_at: i64,
}

#[derive(Clone, Debug)]
pub(super) struct StackGroup {
    pub(super) id: BranchId,
    pub(super) component_id: BranchId,
    pub(super) branches: Vec<BranchId>,
    pub(super) attach_parent: Option<BranchId>,
    pub(super) child_stacks_by_parent: HashMap<BranchId, Vec<BranchId>>,
    pub(super) activity: i64,
    pub(super) default_index: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ProjectionGroupState {
    pub(super) visible: bool,
    pub(super) structural_count: usize,
    pub(super) named_count: usize,
    pub(super) emphasis: Emphasis,
}

impl Default for ProjectionGroupState {
    fn default() -> Self {
        Self {
            visible: false,
            structural_count: 0,
            named_count: 0,
            emphasis: Emphasis::Hidden,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TopologyIndex {
    pub(super) nodes: HashMap<BranchId, Node>,
    pub(super) children: HashMap<BranchId, Vec<BranchId>>,
    pub(super) trunks: Vec<BranchId>,
    pub(super) root_stacks: HashMap<Option<BranchId>, Vec<BranchId>>,
    pub(super) groups: HashMap<BranchId, StackGroup>,
    pub(super) stack_by_branch: HashMap<BranchId, BranchId>,
}
