use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::BranchId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderMode {
    Recent,
    Alphabetical,
    Graphite,
    Chronological,
}

impl Default for OrderMode {
    fn default() -> Self {
        Self::Recent
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ArchiveMode {
    #[default]
    Active,
    Archive,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Emphasis {
    #[default]
    Full,
    Dim,
    Hidden,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ProjectionScope {
    #[default]
    All,
    Stack(BranchId),
    Trunk(BranchId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionOptions {
    pub order: OrderMode,
    pub scope: ProjectionScope,
    pub separators: bool,
    pub filter: String,
    pub archive_mode: ArchiveMode,
    pub archived: HashSet<BranchId>,
    pub stack_names: HashMap<BranchId, Arc<str>>,
}

impl Default for ProjectionOptions {
    fn default() -> Self {
        Self {
            order: OrderMode::Recent,
            scope: ProjectionScope::All,
            separators: true,
            filter: String::new(),
            archive_mode: ArchiveMode::Active,
            archived: HashSet::new(),
            stack_names: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedSection {
    pub title: Arc<str>,
    pub trunk: Option<BranchId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedRow {
    pub branch: BranchId,
    pub depth: usize,
    pub stack_index: usize,
    pub stack_id: BranchId,
    pub lane: usize,
    pub context_only: bool,
    pub is_trunk: bool,
    pub emphasis: Emphasis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorRow {
    pub section: Option<BranchId>,
    pub stack_id: BranchId,
    pub parent: Option<BranchId>,
    pub from_lane: usize,
    pub to_lane: usize,
    pub emphasis: Emphasis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaceholderRow {
    pub section: Option<BranchId>,
    pub branch: BranchId,
    pub parent: Option<BranchId>,
    pub stack_id: BranchId,
    pub lane: usize,
    pub emphasis: Emphasis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackLabelRow {
    pub stack_id: BranchId,
    pub lane: usize,
    pub text: Arc<str>,
    pub emphasis: Emphasis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DividerRow {
    Spacer { section: Option<BranchId> },
    Connector(ConnectorRow),
    Placeholder(PlaceholderRow),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionEntry {
    Section(ProjectedSection),
    StackLabel(StackLabelRow),
    Branch(ProjectedRow),
    Divider(DividerRow),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SectionRange {
    pub section: Option<BranchId>,
    pub start: usize,
    pub end: usize,
    pub bottom: usize,
    pub trunk_row: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackAnchor {
    pub branch: BranchId,
    pub stack_id: BranchId,
    pub visual_row: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneSpan {
    pub stack_id: BranchId,
    pub lane: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyProjection {
    pub entries: Vec<ProjectionEntry>,
    pub selectable: Vec<BranchId>,
    pub selectable_visual_rows: Vec<usize>,
    pub branch_to_visual: HashMap<BranchId, usize>,
    pub branch_to_selectable: HashMap<BranchId, usize>,
    pub stack_heads: Vec<StackAnchor>,
    pub lane_spans: Vec<LaneSpan>,
    pub(super) lane_spans_by_lane: Vec<Vec<LaneSpan>>,
    pub lane_count: usize,
    pub section_ranges: Vec<SectionRange>,
    pub emphasis_by_branch: HashMap<BranchId, Emphasis>,
    pub rows: Vec<ProjectedRow>,
    pub stack_starts: Vec<usize>,
    pub(super) visible_stack_sizes: HashMap<BranchId, usize>,
}
