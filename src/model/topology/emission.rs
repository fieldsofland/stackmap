use super::super::BranchId;

#[derive(Clone, Copy)]
pub(super) enum EmitPhase {
    StartBranch,
    Children,
    FinishBranch,
}

pub(super) struct EmitFrame {
    pub(super) stack: BranchId,
    pub(super) lane: usize,
    pub(super) branch_index: usize,
    pub(super) phase: EmitPhase,
    pub(super) current_branch: Option<BranchId>,
    pub(super) children: Vec<BranchId>,
    pub(super) next_child: usize,
    pub(super) pending_child: Option<BranchId>,
    pub(super) first_row: Option<usize>,
    pub(super) last_row: Option<usize>,
    pub(super) head: Option<(BranchId, usize)>,
    pub(super) all_structural: bool,
    pub(super) all_named: bool,
}
