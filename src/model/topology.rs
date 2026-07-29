use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{Branch, BranchId, RepositorySnapshot};

mod emission;
mod index;
mod projection;

use emission::{EmitFrame, EmitPhase};

type ManualSectionProjection = HashMap<BranchId, (usize, Option<(BranchId, Arc<str>)>)>;
const VISUAL_SECTION_COLORS: [&str; 8] = [
    "#7aa2f7", "#bb9af7", "#7dcfff", "#ff9e64", "#9ece6a", "#f7768e", "#2ac3de", "#c0caf5",
];
pub use index::TopologyIndex;
use index::{Node, ProjectionGroupState, StackGroup};
pub use projection::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackDiffEndpoints {
    pub stack_id: BranchId,
    pub bottom: BranchId,
    pub head: BranchId,
}

impl TopologyIndex {
    pub fn build(snapshot: &RepositorySnapshot) -> Self {
        Self::from_parts(
            &snapshot.branches,
            &snapshot.trunks,
            &snapshot.graphite_children,
        )
    }

    fn from_parts(
        branches: &[Branch],
        trunks: &[BranchId],
        graphite_children: &[(BranchId, Arc<[BranchId]>)],
    ) -> Self {
        let nodes: HashMap<_, _> = branches
            .iter()
            .map(|branch| {
                (
                    branch.id.clone(),
                    Node {
                        id: branch.id.clone(),
                        parent: branch.parent.clone(),
                        trunk: branch.trunk.clone(),
                        committed_at: branch.committed_at,
                    },
                )
            })
            .collect();
        let configured_order: HashMap<_, _> = graphite_children.iter().cloned().collect();
        let mut children: HashMap<BranchId, Vec<BranchId>> = HashMap::new();
        for node in nodes.values() {
            if let Some(parent) = &node.parent
                && nodes.contains_key(parent)
            {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(node.id.clone());
            }
        }
        for (parent, values) in &mut children {
            order_children(values, configured_order.get(parent));
        }

        let trunk_set: HashSet<_> = trunks.iter().cloned().collect();
        let mut roots: HashMap<Option<BranchId>, Vec<BranchId>> = HashMap::new();
        for node in nodes.values() {
            if trunk_set.contains(&node.id) {
                continue;
            }
            if node.parent.is_none()
                || node
                    .parent
                    .as_ref()
                    .is_some_and(|parent| !nodes.contains_key(parent))
            {
                roots
                    .entry(node.trunk.clone())
                    .or_default()
                    .push(node.id.clone());
            }
        }
        for (trunk, values) in &mut roots {
            if let Some(trunk) = trunk {
                order_children(values, configured_order.get(trunk));
            } else {
                values.sort();
            }
        }

        let mut index = Self {
            nodes,
            children,
            trunks: trunks.to_vec(),
            ..Self::default()
        };
        let section_roots: Vec<_> = index
            .trunks
            .iter()
            .cloned()
            .map(Some)
            .chain(std::iter::once(None))
            .collect();
        let mut next_default_index = 0;
        for section in section_roots {
            let section_branch_roots = roots.get(&section).cloned().unwrap_or_default();
            for root in section_branch_roots {
                let stack_id = root.clone();
                index
                    .root_stacks
                    .entry(section.clone())
                    .or_default()
                    .push(stack_id.clone());
                index.assign_stack(&root, stack_id, None, &mut next_default_index);
            }
        }
        index
    }

    fn assign_stack(
        &mut self,
        branch: &BranchId,
        stack_id: BranchId,
        attach_parent: Option<BranchId>,
        next_default_index: &mut usize,
    ) {
        let mut pending = vec![(branch.clone(), stack_id, attach_parent)];
        while let Some((branch, stack_id, attach_parent)) = pending.pop() {
            if self.stack_by_branch.contains_key(&branch) {
                continue;
            }
            if !self.groups.contains_key(&stack_id) {
                let group_attach_parent = attach_parent.clone();
                let parent_stack = attach_parent
                    .as_ref()
                    .and_then(|parent| self.stack_by_branch.get(parent))
                    .cloned();
                let component_id = parent_stack
                    .as_ref()
                    .and_then(|parent| self.groups.get(parent))
                    .map(|parent| parent.component_id.clone())
                    .unwrap_or_else(|| stack_id.clone());
                self.groups.insert(
                    stack_id.clone(),
                    StackGroup {
                        id: stack_id.clone(),
                        component_id,
                        branches: Vec::new(),
                        attach_parent: group_attach_parent.clone(),
                        child_stacks_by_parent: HashMap::new(),
                        activity: i64::MIN,
                        default_index: *next_default_index,
                    },
                );
                *next_default_index += 1;
                if let Some(parent_stack) = parent_stack
                    && let Some(parent) = self.groups.get_mut(&parent_stack)
                {
                    if let Some(attach_parent) = group_attach_parent {
                        parent
                            .child_stacks_by_parent
                            .entry(attach_parent)
                            .or_default()
                            .push(stack_id.clone());
                    }
                }
            }
            self.stack_by_branch
                .insert(branch.clone(), stack_id.clone());
            if let Some(node) = self.nodes.get(&branch)
                && let Some(group) = self.groups.get_mut(&stack_id)
            {
                group.branches.push(branch.clone());
                group.activity = group.activity.max(node.committed_at);
            }

            let descendants = self.children.get(&branch).cloned().unwrap_or_default();
            for child in descendants.iter().skip(1).rev() {
                pending.push((child.clone(), child.clone(), Some(branch.clone())));
            }
            if let Some(primary) = descendants.first() {
                pending.push((primary.clone(), stack_id, None));
            }
        }
    }

    pub fn stack_for(&self, branch: &BranchId) -> Option<&BranchId> {
        self.stack_by_branch.get(branch)
    }

    pub fn stack_diff_endpoints(&self) -> Vec<StackDiffEndpoints> {
        self.groups
            .values()
            .filter_map(|group| {
                Some(StackDiffEndpoints {
                    stack_id: group.id.clone(),
                    bottom: group.branches.first()?.clone(),
                    head: group.branches.last()?.clone(),
                })
            })
            .collect()
    }

    pub fn trunk_for(&self, branch: &BranchId) -> Option<&BranchId> {
        self.nodes.get(branch)?.trunk.as_ref()
    }

    pub fn is_trunk(&self, branch: &BranchId) -> bool {
        self.trunks.contains(branch)
    }

    pub fn project(&self, options: &ProjectionOptions) -> TopologyProjection {
        // Manual section depth is derived from the complete stack, before any visibility
        // projection, so filtering and archive views cannot make labels jump sideways.
        let mut manual_sections = HashMap::new();
        for group in self.groups.values() {
            let mut depth = 0usize;
            let mut active: Option<(BranchId, Arc<str>)> = None;
            let mut lower_color = options.stack_colors.get(&group.id).cloned();
            for branch in &group.branches {
                if let Some(section) = options.visual_sections.get(branch) {
                    depth += 1;
                    let color = if lower_color.as_deref() == Some(section.color.as_ref()) {
                        VISUAL_SECTION_COLORS
                            .iter()
                            .find(|candidate| Some(**candidate) != lower_color.as_deref())
                            .copied()
                            .map(Arc::from)
                            .unwrap_or_else(|| section.color.clone())
                    } else {
                        section.color.clone()
                    };
                    lower_color = Some(color.clone());
                    active = Some((branch.clone(), color));
                }
                manual_sections.insert(branch.clone(), (depth, active.clone()));
            }
        }
        let emphasis_by_branch = self.emphasis_for_scope(&options.scope);
        let needle = options.filter.to_lowercase();
        let filter_active = !needle.is_empty();
        let all_scope = matches!(options.scope, ProjectionScope::All);
        let archive_filter_is_empty = options.archived.is_empty();
        let mut named_visible = HashSet::with_capacity(self.nodes.len());
        for node in self.nodes.values() {
            let in_scope = all_scope || emphasis_by_branch.get(&node.id) != Some(&Emphasis::Hidden);
            let archive_selected = if archive_filter_is_empty {
                matches!(options.archive_mode, ArchiveMode::Active)
            } else {
                match options.archive_mode {
                    ArchiveMode::Active => !options.archived.contains(&node.id),
                    ArchiveMode::Archive => options.archived.contains(&node.id),
                }
            };
            if in_scope
                && archive_selected
                && (!filter_active || node.id.0.to_lowercase().contains(&needle))
            {
                named_visible.insert(node.id.clone());
            }
        }
        let exact_matches = filter_active.then(|| named_visible.clone());

        // Named rows determine selection. Structural rows add only the minimum ancestry needed
        // to preserve real edges when an archived internal branch is omitted from that policy.
        let needs_closure = filter_active
            || !archive_filter_is_empty
            || matches!(options.archive_mode, ArchiveMode::Archive);
        let mut structural_storage = needs_closure.then(|| named_visible.clone());
        if let Some(structural_visible) = structural_storage.as_mut() {
            let mut frontier = Vec::with_capacity(named_visible.len());
            frontier.extend(named_visible.iter().cloned());
            while let Some(branch) = frontier.pop() {
                let Some(parent) = self
                    .nodes
                    .get(&branch)
                    .and_then(|node| node.parent.as_ref())
                else {
                    continue;
                };
                if emphasis_by_branch.get(parent) == Some(&Emphasis::Hidden) {
                    continue;
                }
                let parent_selected = match options.archive_mode {
                    ArchiveMode::Active => !options.archived.contains(parent),
                    ArchiveMode::Archive => options.archived.contains(parent),
                };
                if parent_selected {
                    named_visible.insert(parent.clone());
                }
                if structural_visible.insert(parent.clone()) {
                    frontier.push(parent.clone());
                }
            }

            let trunk_anchors: HashSet<_> = named_visible
                .iter()
                .filter_map(|branch| self.nodes.get(branch)?.trunk.as_ref())
                .filter(|trunk| emphasis_by_branch.get(*trunk) != Some(&Emphasis::Hidden))
                .cloned()
                .collect();
            for trunk in trunk_anchors {
                structural_visible.insert(trunk.clone());
                named_visible.insert(trunk);
            }
        }
        let structural_visible = structural_storage.as_ref().unwrap_or(&named_visible);
        let group_states = self.projection_group_states(
            structural_visible,
            &named_visible,
            &emphasis_by_branch,
            structural_storage.is_none(),
            all_scope,
        );

        let mut projection = TopologyProjection {
            entries: Vec::with_capacity(
                structural_visible.len() + self.groups.len() * 3 + self.trunks.len() + 1,
            ),
            selectable: Vec::with_capacity(named_visible.len()),
            selectable_visual_rows: Vec::with_capacity(named_visible.len()),
            branch_to_visual: HashMap::with_capacity(named_visible.len()),
            branch_to_selectable: HashMap::with_capacity(named_visible.len()),
            stack_heads: Vec::with_capacity(self.groups.len()),
            lane_spans: Vec::with_capacity(self.groups.len() + self.trunks.len()),
            section_ranges: Vec::with_capacity(self.trunks.len() + 1),
            rows: Vec::with_capacity(named_visible.len()),
            stack_starts: Vec::with_capacity(self.groups.len()),
            visible_stack_sizes: HashMap::with_capacity(self.groups.len()),
            emphasis_by_branch,
            ..TopologyProjection::default()
        };
        for group in self.groups.values() {
            let count = group_states[group.default_index].named_count;
            if count > 0 {
                projection
                    .visible_stack_sizes
                    .insert(group.id.clone(), count);
            }
        }
        for trunk in self
            .trunks
            .iter()
            .cloned()
            .map(Some)
            .chain(std::iter::once(None))
        {
            let mut roots = self.root_stacks.get(&trunk).cloned().unwrap_or_default();
            roots.retain(|root| self.group_state(root, &group_states).visible);
            self.sort_groups(&mut roots, options.order, &group_states);
            let trunk_visible = trunk
                .as_ref()
                .is_some_and(|id| structural_visible.contains(id));
            if roots.is_empty() && !trunk_visible {
                continue;
            }
            let title: Arc<str> = trunk
                .as_ref()
                .map(|id| id.0.clone())
                .unwrap_or_else(|| Arc::from("Untrunked"));
            let section_start = projection.entries.len();
            projection
                .entries
                .push(ProjectionEntry::Section(ProjectedSection {
                    title,
                    trunk: trunk.clone(),
                }));
            let mut first_trunk_join = None;
            for (root_index, root) in roots.iter().enumerate() {
                if root_index > 0 && trunk.is_none() && options.separators {
                    projection
                        .entries
                        .push(ProjectionEntry::Divider(DividerRow::Spacer {
                            section: trunk.clone(),
                        }));
                }
                let Some(span_index) = self.emit_hierarchical_group(
                    root,
                    1,
                    trunk.as_ref(),
                    options,
                    structural_visible,
                    &named_visible,
                    exact_matches.as_ref(),
                    &group_states,
                    &manual_sections,
                    &mut projection,
                ) else {
                    continue;
                };
                if let Some(trunk_id) = &trunk {
                    if options.separators {
                        projection
                            .entries
                            .push(ProjectionEntry::Divider(DividerRow::Spacer {
                                section: trunk.clone(),
                            }));
                    }
                    let connector_row = projection.entries.len();
                    projection
                        .entries
                        .push(ProjectionEntry::Divider(DividerRow::Connector(
                            ConnectorRow {
                                section: trunk.clone(),
                                stack_id: root.clone(),
                                parent: Some(trunk_id.clone()),
                                from_lane: 1,
                                to_lane: 0,
                                emphasis: self.group_state(root, &group_states).emphasis,
                            },
                        )));
                    projection.lane_count = projection.lane_count.max(2);
                    projection.lane_spans[span_index].end = connector_row;
                    first_trunk_join.get_or_insert(connector_row);
                }
            }
            let mut trunk_row = None;
            if let Some(trunk_id) = trunk.as_ref().filter(|id| named_visible.contains(*id)) {
                let stack_index = projection.stack_heads.len();
                trunk_row = Some(projection.entries.len());
                let emphasis = if all_scope {
                    Emphasis::Full
                } else {
                    projection
                        .emphasis_by_branch
                        .get(trunk_id)
                        .copied()
                        .unwrap_or_default()
                };
                push_branch(
                    &mut projection,
                    ProjectedRow {
                        branch: trunk_id.clone(),
                        depth: 0,
                        stack_index,
                        stack_id: trunk_id.clone(),
                        lane: 0,
                        context_only: exact_matches
                            .as_ref()
                            .is_some_and(|matches| !matches.contains(trunk_id)),
                        is_trunk: true,
                        emphasis,
                        manual_depth: 0,
                        visual_section: None,
                        visual_color: None,
                    },
                );
            }
            if let Some(trunk_row) = trunk_row {
                let start = first_trunk_join.unwrap_or(trunk_row);
                projection.lane_spans.push(LaneSpan {
                    stack_id: trunk.as_ref().expect("trunk row has trunk").clone(),
                    lane: 0,
                    start,
                    end: trunk_row,
                });
            }
            let section_end = projection.entries.len().saturating_sub(1);
            let bottom = trunk_row
                .or_else(|| {
                    projection.selectable_visual_rows[projection
                        .selectable_visual_rows
                        .partition_point(|row| *row < section_start)..]
                        .iter()
                        .copied()
                        .take_while(|row| *row <= section_end)
                        .last()
                })
                .unwrap_or(section_end);
            projection.section_ranges.push(SectionRange {
                section: trunk.clone(),
                start: section_start,
                end: section_end,
                bottom,
                trunk_row,
            });
        }
        projection
            .stack_heads
            .sort_by_key(|anchor| anchor.visual_row);
        projection.stack_starts = projection
            .stack_heads
            .iter()
            .filter_map(|head| projection.branch_to_selectable.get(&head.branch).copied())
            .collect();
        projection
            .lane_spans
            .sort_by_key(|span| (span.lane, span.start));
        for span in projection.lane_spans.iter().cloned() {
            if projection.lane_spans_by_lane.len() <= span.lane {
                projection
                    .lane_spans_by_lane
                    .resize_with(span.lane + 1, Vec::new);
            }
            projection.lane_spans_by_lane[span.lane].push(span);
        }
        for (visual_row, entry) in projection.entries.iter().enumerate() {
            let target = match entry {
                ProjectionEntry::Branch(row) if !row.context_only => {
                    Some(SelectionTarget::Branch(row.branch.clone()))
                }
                ProjectionEntry::StackLabel(label) => {
                    Some(SelectionTarget::StackLabel(label.stack_id.clone()))
                }
                ProjectionEntry::VisualSectionLabel(label) => {
                    Some(SelectionTarget::VisualSectionLabel(label.anchor.clone()))
                }
                _ => None,
            };
            if let Some(target) = target {
                projection.navigation.push(target);
                projection.navigation_visual_rows.push(visual_row);
            }
        }
        projection
    }

    fn emphasis_for_scope(&self, scope: &ProjectionScope) -> HashMap<BranchId, Emphasis> {
        match scope {
            ProjectionScope::All => self
                .nodes
                .keys()
                .cloned()
                .map(|branch| (branch, Emphasis::Full))
                .collect(),
            ProjectionScope::Trunk(trunk) => self
                .nodes
                .values()
                .map(|node| {
                    let emphasis = if node.trunk.as_ref() == Some(trunk) || &node.id == trunk {
                        Emphasis::Full
                    } else {
                        Emphasis::Hidden
                    };
                    (node.id.clone(), emphasis)
                })
                .collect(),
            ProjectionScope::Stack(anchor) => {
                let Some(stack) = self.stack_by_branch.get(anchor) else {
                    return self.emphasis_for_scope(&ProjectionScope::All);
                };
                let Some(group) = self.groups.get(stack) else {
                    return self.emphasis_for_scope(&ProjectionScope::All);
                };
                let mut full = HashSet::new();
                full.extend(group.branches.iter().cloned());
                let mut cursor = group.attach_parent.as_ref();
                while let Some(branch) = cursor {
                    if !full.insert(branch.clone()) {
                        break;
                    }
                    cursor = self.nodes.get(branch).and_then(|node| node.parent.as_ref());
                }
                let section = self.nodes.get(anchor).and_then(|node| node.trunk.clone());
                let component = group.component_id.clone();
                if let Some(trunk) = &section {
                    full.insert(trunk.clone());
                }
                self.nodes
                    .values()
                    .map(|node| {
                        let emphasis = if full.contains(&node.id) {
                            Emphasis::Full
                        } else if section
                            .as_ref()
                            .is_some_and(|section| node.trunk.as_ref() == Some(section))
                            || section.is_none()
                                && node.trunk.is_none()
                                && self
                                    .stack_by_branch
                                    .get(&node.id)
                                    .and_then(|stack| self.groups.get(stack))
                                    .is_some_and(|group| group.component_id == component)
                        {
                            Emphasis::Dim
                        } else {
                            Emphasis::Hidden
                        };
                        (node.id.clone(), emphasis)
                    })
                    .collect()
            }
        }
    }

    fn projection_group_states(
        &self,
        structural_visible: &HashSet<BranchId>,
        named_visible: &HashSet<BranchId>,
        emphasis_by_branch: &HashMap<BranchId, Emphasis>,
        structural_is_named: bool,
        uniform_full_emphasis: bool,
    ) -> Vec<ProjectionGroupState> {
        let mut states = vec![ProjectionGroupState::default(); self.groups.len()];
        for group in self.groups.values() {
            let state = &mut states[group.default_index];
            for branch in &group.branches {
                let named = named_visible.contains(branch);
                let structural = if structural_is_named {
                    named
                } else {
                    structural_visible.contains(branch)
                };
                state.visible |= structural;
                state.structural_count += usize::from(structural);
                state.named_count += usize::from(named);
                let emphasis = if uniform_full_emphasis {
                    Emphasis::Full
                } else {
                    emphasis_by_branch
                        .get(branch)
                        .copied()
                        .unwrap_or(Emphasis::Hidden)
                };
                if emphasis_rank(emphasis) < emphasis_rank(state.emphasis) {
                    state.emphasis = emphasis;
                }
            }
        }
        states
    }

    fn group_state(
        &self,
        stack: &BranchId,
        states: &[ProjectionGroupState],
    ) -> ProjectionGroupState {
        self.groups
            .get(stack)
            .and_then(|group| states.get(group.default_index))
            .copied()
            .unwrap_or_default()
    }

    fn sort_groups(
        &self,
        groups: &mut [BranchId],
        order: OrderMode,
        states: &[ProjectionGroupState],
    ) {
        groups.sort_by(|left, right| {
            let left = &self.groups[left];
            let right = &self.groups[right];
            let left_complete = states[left.default_index].named_count >= 2;
            let right_complete = states[right.default_index].named_count >= 2;
            left_complete
                .cmp(&right_complete)
                .then_with(|| match order {
                    OrderMode::Recent => left
                        .activity
                        .cmp(&right.activity)
                        .then_with(|| left.default_index.cmp(&right.default_index)),
                    OrderMode::Chronological => left
                        .activity
                        .cmp(&right.activity)
                        .then_with(|| left.default_index.cmp(&right.default_index)),
                    OrderMode::Alphabetical => left.id.cmp(&right.id),
                    OrderMode::Graphite => left.default_index.cmp(&right.default_index),
                })
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_hierarchical_group(
        &self,
        stack: &BranchId,
        lane: usize,
        section: Option<&BranchId>,
        options: &ProjectionOptions,
        structural_visible: &HashSet<BranchId>,
        named_visible: &HashSet<BranchId>,
        exact_matches: Option<&HashSet<BranchId>>,
        group_states: &[ProjectionGroupState],
        manual_sections: &ManualSectionProjection,
        projection: &mut TopologyProjection,
    ) -> Option<usize> {
        let root = self.groups.get(stack)?;
        let root_state = group_states[root.default_index];
        if !root_state.visible {
            return None;
        }

        let mut started = HashSet::new();
        let mut emitted_visual_section_labels = HashSet::new();
        started.insert(stack.clone());
        let mut frames = vec![EmitFrame {
            stack: stack.clone(),
            lane,
            branch_index: 0,
            phase: EmitPhase::StartBranch,
            current_branch: None,
            children: Vec::new(),
            next_child: 0,
            pending_child: None,
            first_row: None,
            last_row: None,
            head: None,
            all_structural: root_state.structural_count == root.branches.len(),
            all_named: root_state.named_count == root.branches.len(),
        }];
        let mut returned: Option<(BranchId, Option<usize>)> = None;

        loop {
            if let Some((child, child_span)) = returned.take() {
                let Some(parent_frame) = frames.last_mut() else {
                    return child_span;
                };
                if parent_frame.pending_child.take().as_ref() != Some(&child) {
                    continue;
                }
                let Some(child_span) = child_span else {
                    continue;
                };
                if options.separators {
                    projection
                        .entries
                        .push(ProjectionEntry::Divider(DividerRow::Spacer {
                            section: section.cloned(),
                        }));
                }
                let connector_row = projection.entries.len();
                projection
                    .entries
                    .push(ProjectionEntry::Divider(DividerRow::Connector(
                        ConnectorRow {
                            section: section.cloned(),
                            stack_id: child.clone(),
                            parent: parent_frame.current_branch.clone(),
                            from_lane: parent_frame.lane + 1,
                            to_lane: parent_frame.lane,
                            emphasis: self.group_state(&child, group_states).emphasis,
                        },
                    )));
                projection.lane_count = projection.lane_count.max(parent_frame.lane + 2);
                projection.lane_spans[child_span].end = connector_row;
                parent_frame.first_row.get_or_insert(connector_row);
                parent_frame.last_row = Some(connector_row);
                continue;
            }

            let phase = frames.last().map(|frame| frame.phase)?;
            match phase {
                EmitPhase::StartBranch => {
                    let (stack, branch_index) = {
                        let frame = frames.last().expect("emission frame");
                        (frame.stack.clone(), frame.branch_index)
                    };
                    let group = self.groups.get(&stack)?;
                    if branch_index == group.branches.len() {
                        let frame = frames.pop().expect("completed emission frame");
                        let span = frame.first_row.zip(frame.last_row).map(|(start, end)| {
                            let span_index = projection.lane_spans.len();
                            projection.lane_spans.push(LaneSpan {
                                stack_id: frame.stack.clone(),
                                lane: frame.lane,
                                start,
                                end,
                            });
                            projection.lane_count = projection.lane_count.max(frame.lane + 1);
                            if let Some((branch, visual_row)) = frame.head {
                                projection.stack_heads.push(StackAnchor {
                                    branch,
                                    stack_id: frame.stack.clone(),
                                    visual_row,
                                });
                            }
                            span_index
                        });
                        returned = Some((frame.stack, span));
                        continue;
                    }

                    let branch = group.branches[group.branches.len() - 1 - branch_index].clone();
                    let mut children: Vec<_> = group
                        .child_stacks_by_parent
                        .get(&branch)
                        .into_iter()
                        .flatten()
                        .filter(|child| self.group_state(child, group_states).visible)
                        .cloned()
                        .collect();
                    self.sort_groups(&mut children, options.order, group_states);
                    let frame = frames.last_mut().expect("emission frame");
                    frame.current_branch = Some(branch);
                    frame.children = children;
                    frame.next_child = 0;
                    frame.phase = EmitPhase::Children;
                }
                EmitPhase::Children => {
                    let next = {
                        let frame = frames.last_mut().expect("emission frame");
                        let child = frame.children.get(frame.next_child).cloned();
                        frame.next_child += usize::from(child.is_some());
                        child.map(|child| (child, frame.lane + 1))
                    };
                    let Some((child, child_lane)) = next else {
                        frames.last_mut().expect("emission frame").phase = EmitPhase::FinishBranch;
                        continue;
                    };
                    if !started.insert(child.clone()) {
                        continue;
                    }
                    let Some(child_group) = self.groups.get(&child) else {
                        continue;
                    };
                    let child_state = group_states[child_group.default_index];
                    if !child_state.visible {
                        continue;
                    }
                    frames
                        .last_mut()
                        .expect("parent emission frame")
                        .pending_child = Some(child.clone());
                    frames.push(EmitFrame {
                        stack: child,
                        lane: child_lane,
                        branch_index: 0,
                        phase: EmitPhase::StartBranch,
                        current_branch: None,
                        children: Vec::new(),
                        next_child: 0,
                        pending_child: None,
                        first_row: None,
                        last_row: None,
                        head: None,
                        all_structural: child_state.structural_count == child_group.branches.len(),
                        all_named: child_state.named_count == child_group.branches.len(),
                    });
                }
                EmitPhase::FinishBranch => {
                    let (branch, lane, stack_id, stack_index, all_structural, all_named) = {
                        let frame = frames.last().expect("emission frame");
                        let group = &self.groups[&frame.stack];
                        (
                            frame
                                .current_branch
                                .clone()
                                .expect("branch prepared before emission"),
                            frame.lane,
                            group.id.clone(),
                            group.default_index,
                            frame.all_structural,
                            frame.all_named,
                        )
                    };
                    if all_structural || structural_visible.contains(&branch) {
                        let emphasis = if matches!(options.scope, ProjectionScope::All) {
                            Emphasis::Full
                        } else {
                            projection
                                .emphasis_by_branch
                                .get(&branch)
                                .copied()
                                .unwrap_or_default()
                        };
                        if all_named || named_visible.contains(&branch) {
                            let (manual_depth, visual_section) =
                                manual_sections.get(&branch).cloned().unwrap_or((0, None));
                            if frames.last().is_some_and(|frame| frame.head.is_none())
                                && let Some(text) = options.stack_names.get(&stack_id).cloned()
                            {
                                let label_row = projection.entries.len();
                                let frame = frames.last_mut().expect("emission frame");
                                frame.first_row.get_or_insert(label_row);
                                frame.last_row = Some(label_row);
                                projection.entries.push(ProjectionEntry::StackLabel(
                                    StackLabelRow {
                                        stack_id: stack_id.clone(),
                                        lane,
                                        text,
                                        branch_count: self.groups[&stack_id].branches.len(),
                                        emphasis,
                                    },
                                ));
                                let spacer_row = projection.entries.len();
                                frame.last_row = Some(spacer_row);
                                projection.entries.push(ProjectionEntry::Divider(
                                    DividerRow::Spacer {
                                        section: section.cloned(),
                                    },
                                ));
                            }
                            if let Some((anchor, color)) = &visual_section
                                && let Some(section) = options.visual_sections.get(anchor)
                                && section.name.is_some()
                                && emitted_visual_section_labels.insert(anchor.clone())
                            {
                                let text = section.name.clone().expect("checked section name");
                                let label_row = projection.entries.len();
                                let frame = frames.last_mut().expect("emission frame");
                                frame.first_row.get_or_insert(label_row);
                                frame.last_row = Some(label_row);
                                projection.entries.push(ProjectionEntry::VisualSectionLabel(
                                    VisualSectionLabelRow {
                                        anchor: anchor.clone(),
                                        stack_id: stack_id.clone(),
                                        lane,
                                        manual_depth,
                                        text,
                                        color: color.clone(),
                                        emphasis,
                                    },
                                ));
                            }
                            let visual_row = projection.entries.len();
                            let frame = frames.last_mut().expect("emission frame");
                            frame.first_row.get_or_insert(visual_row);
                            frame.last_row = Some(visual_row);
                            frame
                                .head
                                .get_or_insert_with(|| (branch.clone(), visual_row));
                            push_branch(
                                projection,
                                ProjectedRow {
                                    branch: branch.clone(),
                                    depth: lane,
                                    stack_index,
                                    stack_id,
                                    lane,
                                    context_only: exact_matches
                                        .is_some_and(|matches| !matches.contains(&branch)),
                                    is_trunk: false,
                                    emphasis,
                                    manual_depth,
                                    visual_section: visual_section
                                        .as_ref()
                                        .map(|(anchor, _)| anchor.clone()),
                                    visual_color: visual_section
                                        .as_ref()
                                        .map(|(_, color)| color.clone()),
                                },
                            );
                            if let Some(section) = options.visual_sections.get(&branch) {
                                let effective_color = visual_section
                                    .as_ref()
                                    .filter(|(anchor, _)| anchor == &branch)
                                    .map(|(_, color)| color.clone())
                                    .unwrap_or_else(|| section.color.clone());
                                projection
                                    .entries
                                    .push(ProjectionEntry::VisualSectionDivider(
                                        VisualSectionDividerRow {
                                            anchor: branch.clone(),
                                            stack_id: frames
                                                .last()
                                                .expect("emission frame")
                                                .stack
                                                .clone(),
                                            lane,
                                            manual_depth,
                                            color: effective_color,
                                            emphasis,
                                        },
                                    ));
                            }
                        } else {
                            let visual_row = projection.entries.len();
                            let frame = frames.last_mut().expect("emission frame");
                            frame.first_row.get_or_insert(visual_row);
                            frame.last_row = Some(visual_row);
                            projection.entries.push(ProjectionEntry::Divider(
                                DividerRow::Placeholder(PlaceholderRow {
                                    section: section.cloned(),
                                    branch: branch.clone(),
                                    parent: self
                                        .nodes
                                        .get(&branch)
                                        .and_then(|node| node.parent.clone()),
                                    stack_id,
                                    lane,
                                    emphasis,
                                }),
                            ));
                            projection.lane_count = projection.lane_count.max(lane + 1);
                        }
                    }

                    let frame = frames.last_mut().expect("emission frame");
                    frame.branch_index += 1;
                    frame.phase = EmitPhase::StartBranch;
                    frame.current_branch = None;
                    frame.children.clear();
                }
            }
        }
    }
}

impl TopologyProjection {
    pub fn build(branches: &[Branch], filter: &str) -> Self {
        TopologyIndex::from_parts(branches, &[], &[]).project(&ProjectionOptions {
            filter: filter.to_owned(),
            separators: false,
            ..ProjectionOptions::default()
        })
    }

    pub fn row_for(&self, branch: &BranchId) -> Option<&ProjectedRow> {
        let visual = self.branch_to_visual.get(branch)?;
        match self.entries.get(*visual)? {
            ProjectionEntry::Branch(row) => Some(row),
            _ => None,
        }
    }

    pub fn emphasis_for(&self, branch: &BranchId) -> Emphasis {
        self.emphasis_by_branch
            .get(branch)
            .copied()
            .unwrap_or(Emphasis::Hidden)
    }

    pub fn is_true_stack(&self, branch: &BranchId) -> bool {
        self.row_for(branch)
            .and_then(|row| self.visible_stack_sizes.get(&row.stack_id))
            .is_some_and(|count| *count >= 2)
    }

    pub fn active_lane_span(&self, lane: usize, visual_row: usize) -> Option<&LaneSpan> {
        let spans = self.lane_spans_by_lane.get(lane)?;
        let position = spans.partition_point(|span| span.start <= visual_row);
        position
            .checked_sub(1)
            .and_then(|index| spans.get(index))
            .filter(|span| span.end >= visual_row)
    }
}

fn push_branch(projection: &mut TopologyProjection, row: ProjectedRow) {
    let visual = projection.entries.len();
    let selectable = projection.selectable.len();
    projection
        .branch_to_visual
        .insert(row.branch.clone(), visual);
    projection
        .branch_to_selectable
        .insert(row.branch.clone(), selectable);
    projection.selectable.push(row.branch.clone());
    projection.selectable_visual_rows.push(visual);
    projection.lane_count = projection.lane_count.max(row.lane + 1);
    projection.rows.push(row.clone());
    projection.entries.push(ProjectionEntry::Branch(row));
}

fn emphasis_rank(emphasis: Emphasis) -> u8 {
    match emphasis {
        Emphasis::Full => 0,
        Emphasis::Dim => 1,
        Emphasis::Hidden => 2,
    }
}

fn order_children(values: &mut [BranchId], configured: Option<&Arc<[BranchId]>>) {
    let configured_position: HashMap<_, _> = configured
        .into_iter()
        .flat_map(|values| values.iter().enumerate())
        .map(|(index, branch)| (branch.clone(), index))
        .collect();
    values.sort_by(|left, right| {
        configured_position
            .get(left)
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(
                &configured_position
                    .get(right)
                    .copied()
                    .unwrap_or(usize::MAX),
            )
            .then_with(|| left.cmp(right))
    });
}
