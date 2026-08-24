use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapters::git::{
    DeleteOutcome, DeleteRequest, GraphiteBranchExpectation, GraphiteEdgeExpectation,
    GraphiteMutationOutcome, MoveRequest, RestackRequest,
};
use crate::adapters::github::{GitHubError, PrMatch};
use crate::config::{ArchiveMutation, Config, ConfigMutation, MAX_STACK_NAME_CHARS, VisualSection};
use crate::events::{Input, Key, KeyPhase};
use crate::model::topology::{
    ArchiveMode, Emphasis, OrderMode, ProjectionOptions, ProjectionScope, TopologyIndex,
    TopologyProjection,
};
use crate::model::{Branch, BranchId, ConfiguredUpstream, PullRequestLookup, RepositorySnapshot};
use crate::refresh::github::{
    GithubBatch, GithubCommand, GithubOutcome, GithubRequest, GithubTarget,
};
use crate::refresh::graphite_health::{
    HealthBatch, HealthCommand, HealthRequest, HealthTarget, MAX_TARGETS as MAX_HEALTH_TARGETS,
};
use crate::refresh::upstream::{
    MAX_TARGETS, UpstreamBatch, UpstreamCommand, UpstreamRequest, UpstreamTarget,
};

pub const COLOR_OPTIONS: &[(&str, Option<&str>)] = &[
    ("Auto", None),
    ("Blue", Some("#7aa2f7")),
    ("Purple", Some("#bb9af7")),
    ("Cyan", Some("#7dcfff")),
    ("Orange", Some("#ff9e64")),
    ("Green", Some("#9ece6a")),
    ("Red", Some("#f7768e")),
    ("Teal", Some("#2ac3de")),
    ("Slate", Some("#c0caf5")),
];

const AUTO_LANE_PITCH: u16 = 3;
const MIN_LANE_PITCH: u16 = 2;
const MAX_LANE_PITCH: u16 = 6;
const DEFAULT_RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_NOTICE_DURATION: Duration = Duration::from_secs(5);
const ORDER_OPTIONS: &[OrderMode] = &[
    OrderMode::Recent,
    OrderMode::Alphabetical,
    OrderMode::Graphite,
];
type UpstreamWorkingSet = (u64, Vec<(BranchId, Arc<str>)>);
type HealthWorkingSet = (u64, Vec<(BranchId, Arc<str>, Arc<str>)>);
type GithubWorkingSet = (u64, Vec<(BranchId, Arc<str>)>);

fn changed_branch_candidates(
    current: &RepositorySnapshot,
    next: &RepositorySnapshot,
) -> Vec<BranchId> {
    let mut changed = next
        .branches
        .iter()
        .filter(|branch| {
            current
                .branch(&branch.id)
                .is_some_and(|previous| previous.oid != branch.oid)
        })
        .map(|branch| (branch.committed_at, branch.id.clone()))
        .collect::<Vec<_>>();
    changed.sort_by(|left, right| right.cmp(left));
    changed.into_iter().map(|(_, branch)| branch).collect()
}

fn github_changed_candidates(
    current: &RepositorySnapshot,
    next: &RepositorySnapshot,
) -> Vec<BranchId> {
    let mut changed = next
        .branches
        .iter()
        .filter(|branch| {
            current
                .branch(&branch.id)
                .is_none_or(|previous| previous.oid != branch.oid)
        })
        .map(|branch| (branch.committed_at, branch.id.clone()))
        .collect::<Vec<_>>();
    changed.sort_by(|left, right| right.cmp(left));
    changed.into_iter().map(|(_, branch)| branch).collect()
}

fn changed_health_candidates(
    current: &RepositorySnapshot,
    next: &RepositorySnapshot,
) -> Vec<BranchId> {
    next.branches
        .iter()
        .filter(|branch| {
            let Some(parent_id) = branch.diff_parent.as_ref() else {
                return false;
            };
            let Some(parent) = next.branch(parent_id) else {
                return false;
            };
            let Some(previous) = current.branch(&branch.id) else {
                return true;
            };
            previous.oid != branch.oid
                || previous.diff_parent.as_ref() != Some(parent_id)
                || previous
                    .diff_parent
                    .as_ref()
                    .and_then(|previous_parent| current.branch(previous_parent))
                    .is_none_or(|previous_parent| previous_parent.oid != parent.oid)
        })
        .map(|branch| branch.id.clone())
        .collect()
}

fn best_pr_match(branch: &Branch, matches: &[PrMatch]) -> Option<crate::model::PullRequest> {
    matches
        .iter()
        .filter(|candidate| candidate.branch == branch.id)
        .max_by(|left, right| {
            let lifecycle_priority = |candidate: &PrMatch| match candidate.pull_request.status {
                crate::model::PullRequestStatus::Approved => 3,
                crate::model::PullRequestStatus::Open => 2,
                crate::model::PullRequestStatus::Merged => 1,
                crate::model::PullRequestStatus::Closed => 0,
            };
            (left.head_oid.as_deref() == Some(branch.oid.as_ref()))
                .cmp(&(right.head_oid.as_deref() == Some(branch.oid.as_ref())))
                .then_with(|| lifecycle_priority(left).cmp(&lifecycle_priority(right)))
                .then_with(|| left.updated_at.cmp(&right.updated_at))
                .then_with(|| left.pull_request.number.cmp(&right.pull_request.number))
        })
        .map(|candidate| candidate.pull_request.clone())
}

mod archive;
mod mutation;
mod overlays;
mod state;

use archive::archive_range_slice;
use mutation::{confirmation_is_valid, deletion_refusal, stale_deletion_refusal};
use overlays::order_option_index;
pub use state::{
    Action, ArchiveRange, ClipboardRequest, ClipboardScope, ColorPicker, ConfigTarget,
    ConfigWriteRequest, DeleteConfirmation, DeletionResult, GitHubState, GraphiteActionResult,
    LanePitch, MovePreview, MutationState, OrderPicker, Overlay, ReconciliationOperation,
    StackNameEditor, ViewScope,
};
use state::{AllViewState, TransientNotice};

pub struct App {
    pub snapshot: Option<Arc<RepositorySnapshot>>,
    pub projection: TopologyProjection,
    topology: Option<TopologyIndex>,
    pub selected: Option<BranchId>,
    pub selected_label: Option<ConfigTarget>,
    pub scroll: usize,
    pub viewport_height: usize,
    pub filter: String,
    pub overlay: Overlay,
    pub mutation: MutationState,
    pub platform_running: bool,
    pub message: Option<Arc<str>>,
    pub refresh_error: Option<Arc<str>>,
    pub github_state: GitHubState,
    pub loading: bool,
    pub config: Config,
    pub order_mode: OrderMode,
    pub lane_pitch: LanePitch,
    pub scope: ViewScope,
    pub separators: bool,
    pub status_visible: bool,
    pub detail_sidebar: bool,
    pub archive_mode: ArchiveMode,
    restore_selection: Option<BranchId>,
    all_view_state: Option<AllViewState>,
    restore_all_after_reproject: bool,
    focus_needs_bottom_alignment: bool,
    startup_current_pending: bool,
    next_config_sequence: u64,
    latest_config_sequence: u64,
    pending_colors: HashMap<BranchId, (u64, Option<Arc<str>>)>,
    pending_archives: HashMap<BranchId, (u64, ArchiveMutation)>,
    pending_stack_names: HashMap<BranchId, (u64, Option<Arc<str>>)>,
    pending_visual_sections: HashMap<BranchId, (u64, Option<VisualSection>)>,
    queued_config_write: Option<ConfigWriteRequest>,
    active_view_state: Option<AllViewState>,
    notice: Option<TransientNotice>,
    reconciliation_timeout: Duration,
    notice_duration: Duration,
    upstream_request_pending: bool,
    upstream_cancel_pending: bool,
    upstream_target_offset: usize,
    last_upstream_working_set: Option<UpstreamWorkingSet>,
    health_request_pending: bool,
    health_cancel_pending: bool,
    health_demand_targets: HashSet<BranchId>,
    health_target_offset: usize,
    last_health_working_set: Option<HealthWorkingSet>,
    github_request_pending: bool,
    github_cancel_pending: bool,
    github_demand_targets: HashSet<BranchId>,
    last_github_working_set: Option<GithubWorkingSet>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            snapshot: None,
            projection: TopologyProjection::default(),
            topology: None,
            selected: None,
            selected_label: None,
            scroll: 0,
            viewport_height: 12,
            filter: String::new(),
            overlay: Overlay::None,
            mutation: MutationState::Idle,
            platform_running: false,
            message: None,
            refresh_error: None,
            github_state: GitHubState::Idle,
            loading: true,
            config: Config::default(),
            order_mode: OrderMode::Recent,
            lane_pitch: LanePitch::Auto,
            scope: ViewScope::All,
            separators: true,
            status_visible: true,
            detail_sidebar: false,
            archive_mode: ArchiveMode::Active,
            restore_selection: None,
            all_view_state: None,
            restore_all_after_reproject: false,
            focus_needs_bottom_alignment: false,
            startup_current_pending: false,
            next_config_sequence: 0,
            latest_config_sequence: 0,
            pending_colors: HashMap::new(),
            pending_archives: HashMap::new(),
            pending_stack_names: HashMap::new(),
            pending_visual_sections: HashMap::new(),
            queued_config_write: None,
            active_view_state: None,
            notice: None,
            reconciliation_timeout: DEFAULT_RECONCILIATION_TIMEOUT,
            notice_duration: DEFAULT_NOTICE_DURATION,
            upstream_request_pending: false,
            upstream_cancel_pending: false,
            upstream_target_offset: 0,
            last_upstream_working_set: None,
            health_request_pending: false,
            health_cancel_pending: false,
            health_demand_targets: HashSet::new(),
            health_target_offset: 0,
            last_health_working_set: None,
            github_request_pending: false,
            github_cancel_pending: false,
            github_demand_targets: HashSet::new(),
            last_github_working_set: None,
        }
    }
}

impl App {
    pub fn with_mutation_timing(
        reconciliation_timeout: Duration,
        notice_duration: Duration,
    ) -> Self {
        Self {
            reconciliation_timeout,
            notice_duration,
            ..Self::default()
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: Arc<RepositorySnapshot>) {
        self.apply_snapshot_kind(snapshot, true, None, Instant::now());
    }

    pub fn apply_structural_snapshot(
        &mut self,
        snapshot: Arc<RepositorySnapshot>,
        request_epoch: u64,
    ) {
        self.apply_structural_snapshot_at(snapshot, request_epoch, Instant::now());
    }

    pub fn apply_structural_snapshot_at(
        &mut self,
        snapshot: Arc<RepositorySnapshot>,
        request_epoch: u64,
        now: Instant,
    ) {
        self.apply_snapshot_kind(snapshot, true, Some(request_epoch), now);
    }

    pub fn apply_enriched_snapshot(&mut self, snapshot: Arc<RepositorySnapshot>) {
        self.apply_snapshot_kind(snapshot, false, None, Instant::now());
    }

    fn apply_snapshot_kind(
        &mut self,
        mut snapshot: Arc<RepositorySnapshot>,
        structural: bool,
        structural_epoch: Option<u64>,
        now: Instant,
    ) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| snapshot.generation < current.generation)
        {
            return;
        }
        let changed_branches = if structural {
            self.snapshot
                .as_deref()
                .map(|current| changed_branch_candidates(current, &snapshot))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let changed_github = if structural {
            self.snapshot
                .as_deref()
                .map(|current| github_changed_candidates(current, &snapshot))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let changed_health = if structural {
            self.snapshot
                .as_deref()
                .map(|current| changed_health_candidates(current, &snapshot))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some(current) = &self.snapshot {
            let current_prs: std::collections::HashMap<_, _> = current
                .branches
                .iter()
                .filter_map(|branch| {
                    branch
                        .pr
                        .clone()
                        .map(|pr| (branch.id.clone(), (branch.oid.clone(), pr)))
                })
                .collect();
            if !current_prs.is_empty() {
                let snapshot = Arc::make_mut(&mut snapshot);
                let branches = Arc::make_mut(&mut snapshot.branches);
                for branch in branches {
                    if branch.pr.is_none() {
                        branch.pr = current_prs.get(&branch.id).map(|(oid, pr)| {
                            let mut retained = pr.clone();
                            if oid != &branch.oid {
                                retained.match_quality = crate::model::PullRequestMatch::StaleTip;
                            }
                            retained
                        });
                    }
                }
            }
            let current_lookup: HashMap<_, _> = current
                .branches
                .iter()
                .filter(|branch| branch.pr_lookup != PullRequestLookup::NotRequested)
                .map(|branch| {
                    (
                        (branch.id.clone(), branch.oid.clone()),
                        branch.pr_lookup.clone(),
                    )
                })
                .collect();
            if !current_lookup.is_empty() {
                let snapshot = Arc::make_mut(&mut snapshot);
                for branch in Arc::make_mut(&mut snapshot.branches) {
                    if let Some(state) =
                        current_lookup.get(&(branch.id.clone(), branch.oid.clone()))
                    {
                        branch.pr_lookup = state.clone();
                    }
                }
            }
            if structural || current.generation == snapshot.generation {
                let current_evidence: HashMap<_, _> = current
                    .branches
                    .iter()
                    .map(|branch| {
                        (
                            (branch.id.clone(), branch.oid.clone()),
                            branch.remote_ref.clone(),
                        )
                    })
                    .collect();
                let current_health: HashMap<_, _> = current
                    .branches
                    .iter()
                    .map(|branch| {
                        (
                            (
                                branch.id.clone(),
                                branch.oid.clone(),
                                branch.diff_parent.clone(),
                                branch
                                    .diff_parent
                                    .as_ref()
                                    .and_then(|parent| current.branch(parent))
                                    .map(|parent| parent.oid.clone()),
                            ),
                            branch.graphite_health.clone(),
                        )
                    })
                    .collect();
                let current_upstreams: HashMap<_, _> = current
                    .branches
                    .iter()
                    .map(|branch| {
                        (
                            (branch.id.clone(), branch.oid.clone()),
                            branch.configured_upstream.clone(),
                        )
                    })
                    .collect();
                let next_oids: HashMap<_, _> = snapshot
                    .branches
                    .iter()
                    .map(|branch| (branch.id.clone(), branch.oid.clone()))
                    .collect();
                let has_matching_branch = snapshot.branches.iter().any(|branch| {
                    current_evidence.contains_key(&(branch.id.clone(), branch.oid.clone()))
                });
                if has_matching_branch {
                    let snapshot = Arc::make_mut(&mut snapshot);
                    for branch in Arc::make_mut(&mut snapshot.branches) {
                        if let Some(evidence) =
                            current_evidence.get(&(branch.id.clone(), branch.oid.clone()))
                        {
                            branch.remote_ref = evidence.clone();
                        }
                        if let Some(health) = current_health.get(&(
                            branch.id.clone(),
                            branch.oid.clone(),
                            branch.diff_parent.clone(),
                            branch
                                .diff_parent
                                .as_ref()
                                .and_then(|parent| next_oids.get(parent))
                                .cloned(),
                        )) {
                            branch.graphite_health = health.clone();
                        }
                        if structural
                            && matches!(
                                branch.configured_upstream,
                                ConfiguredUpstream::Diverged { .. }
                            )
                            && let Some(previous @ ConfiguredUpstream::Rewritten { .. }) =
                                current_upstreams.get(&(branch.id.clone(), branch.oid.clone()))
                        {
                            branch.configured_upstream = previous.clone();
                        }
                    }
                }
            }
        }
        if structural {
            match Config::load(&snapshot.common_dir) {
                Ok(mut config) => {
                    for (root, (_, color)) in &self.pending_colors {
                        if let Err(error) = config.set_color_in_memory(root, color.as_deref()) {
                            self.message = Some(Arc::from(format!(
                                "pending color ignored after validation changed: {error}"
                            )));
                        }
                    }
                    for (branch, (_, archived)) in &self.pending_archives {
                        match archived {
                            ArchiveMutation::Set(value) => {
                                config.set_archived_in_memory(branch, *value)
                            }
                            ArchiveMutation::Prune => config.set_archived_in_memory(branch, false),
                        }
                    }
                    for (stack, (_, name)) in &self.pending_stack_names {
                        if let Err(error) = config.set_stack_name_in_memory(stack, name.as_deref())
                        {
                            self.message = Some(Arc::from(format!(
                                "pending stack name ignored after validation changed: {error}"
                            )));
                        }
                    }
                    for (anchor, (_, section)) in &self.pending_visual_sections {
                        if let Err(error) =
                            config.set_visual_section_in_memory(anchor, section.clone())
                        {
                            self.message = Some(Arc::from(format!(
                                "pending visual section ignored after validation changed: {error}"
                            )));
                        }
                    }
                    self.config = config;
                }
                Err(error) => {
                    self.message = Some(Arc::from(format!(
                        "config ignored; keeping last valid values: {error}"
                    )));
                }
            }
        }
        let projection_changed = structural || self.topology.is_none();
        if projection_changed {
            self.topology = Some(TopologyIndex::build(&snapshot));
        }
        if structural {
            self.prune_invalid_visual_sections();
        }
        let protected_deletion_target = structural
            .then(|| self.deletion_target_in_flight())
            .flatten();
        let mut deletion_selection_after = None;
        if structural {
            if matches!(self.overlay, Overlay::ArchiveRange(_)) {
                self.overlay = Overlay::None;
                self.message = Some(Arc::from(
                    "repository changed; archive range preview cancelled",
                ));
            }
            if let Overlay::MovePreview(preview) = &self.overlay
                && !move_preview_is_valid(&snapshot, preview)
            {
                self.overlay = Overlay::None;
                self.message = Some(Arc::from("repository changed; move preview cancelled"));
            }
            match self.mutation.clone() {
                MutationState::ConfirmingDeletion(confirmation)
                    if !confirmation_is_valid(&snapshot, &confirmation) =>
                {
                    self.mutation = MutationState::Idle;
                    self.message = Some(Arc::from(
                        "repository changed; deletion confirmation cancelled",
                    ));
                }
                MutationState::Reconciling {
                    operation,
                    request_epoch,
                    ..
                } if structural_epoch.is_some_and(|epoch| epoch >= request_epoch) => {
                    let notice = reconciliation_notice(&snapshot, &operation);
                    if let ReconciliationOperation::Deletion {
                        request,
                        result: DeletionResult::Outcome(DeleteOutcome::Deleted),
                        selection_after,
                    } = &operation
                        && snapshot.branch(&request.branch).is_none()
                    {
                        let selection_needs_recovery = self.selected.as_ref()
                            == Some(&request.branch)
                            || self
                                .selected
                                .as_ref()
                                .is_none_or(|selected| snapshot.branch(selected).is_none());
                        self.cleanup_deleted_config_identity(&snapshot, &request.branch);
                        if selection_needs_recovery {
                            deletion_selection_after = Some(selection_after.clone());
                        }
                    }
                    self.mutation = MutationState::Idle;
                    self.set_notice(notice, now);
                }
                _ => {}
            }
        }
        self.snapshot = Some(snapshot);
        self.loading = false;
        if structural {
            self.refresh_error = None;
        }
        if structural {
            self.prune_invalid_archives(protected_deletion_target.as_ref());
        }
        if projection_changed {
            self.reproject();
        }
        let recovered_deleted_selection = if let Some(selection_after) = deletion_selection_after {
            self.selected = selection_after
                .filter(|branch| self.projection.branch_to_selectable.contains_key(branch));
            true
        } else {
            false
        };
        if !recovered_deleted_selection
            && let Some(changed) = changed_branches
                .into_iter()
                .find(|branch| self.projection.branch_to_selectable.contains_key(branch))
        {
            self.selected = Some(changed);
            self.selected_label = None;
        }
        self.reconcile_selection();
        self.revalidate_overlay();
        if structural && self.startup_current_pending {
            self.startup_current_pending = false;
            self.apply_current_startup();
        }
        if structural {
            if self.status_visible {
                self.upstream_request_pending = true;
            }
            self.health_demand_targets.extend(changed_health);
            self.demand_current_graphite_health();
            if self.status_visible && !self.health_demand_targets.is_empty() {
                self.health_request_pending = true;
            }
            self.github_demand_targets.extend(changed_github);
            self.demand_current_github();
            self.github_request_pending = self.status_visible
                && matches!(self.archive_mode, ArchiveMode::Active)
                && !self.github_demand_targets.is_empty();
        }
    }

    pub fn apply_graphite_health_batch(&mut self, batch: HealthBatch) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        if snapshot.generation != batch.generation {
            return;
        }
        let applicable: Vec<_> = batch
            .results
            .iter()
            .filter_map(|result| {
                let index = snapshot.branch_index.get(&result.branch).copied()?;
                let branch = snapshot.branches.get(index)?;
                let parent = snapshot.branch(&result.parent)?;
                (branch.oid == result.oid
                    && branch.diff_parent.as_ref() == Some(&result.parent)
                    && parent.oid == result.parent_oid)
                    .then_some((index, result.health.clone()))
            })
            .collect();
        if applicable.is_empty() {
            return;
        }
        let snapshot = Arc::make_mut(snapshot);
        let branches = Arc::make_mut(&mut snapshot.branches);
        for (index, health) in applicable {
            branches[index].graphite_health = health;
        }
    }

    pub fn take_graphite_health_command(&mut self) -> Option<HealthCommand> {
        if self.health_cancel_pending {
            self.health_cancel_pending = false;
            return Some(HealthCommand::Cancel);
        }
        if !self.status_visible {
            return None;
        }
        if !self.health_request_pending {
            return None;
        }
        self.health_request_pending = false;
        let snapshot = self.snapshot.as_ref()?;
        let mut candidates = snapshot
            .branches
            .iter()
            .filter_map(|branch| {
                if !self.health_demand_targets.contains(&branch.id) {
                    return None;
                }
                let parent_id = branch.diff_parent.as_ref()?;
                let parent = snapshot.branch(parent_id)?;
                Some(HealthTarget {
                    branch: branch.id.clone(),
                    oid: branch.oid.clone(),
                    parent: parent.id.clone(),
                    parent_oid: parent.oid.clone(),
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|target| {
            let branch = snapshot.branch(&target.branch);
            (
                !self.health_demand_targets.contains(&target.branch),
                !branch.is_some_and(|branch| branch.current),
                branch
                    .map(|branch| -branch.committed_at)
                    .unwrap_or_default(),
                target.branch.clone(),
            )
        });
        let target_count = candidates.len().min(MAX_HEALTH_TARGETS);
        let start = self
            .health_target_offset
            .min(candidates.len().saturating_sub(1));
        let targets = candidates
            .iter()
            .cycle()
            .skip(start)
            .take(target_count)
            .cloned()
            .collect::<Vec<_>>();
        self.health_target_offset = if candidates.is_empty() {
            0
        } else {
            (start + target_count) % candidates.len()
        };
        let explicit_demand = !self.health_demand_targets.is_empty();
        self.health_demand_targets.clear();
        let working_set = (
            snapshot.generation,
            targets
                .iter()
                .map(|target| {
                    (
                        target.branch.clone(),
                        target.parent_oid.clone(),
                        target.oid.clone(),
                    )
                })
                .collect(),
        );
        if !explicit_demand && self.last_health_working_set.as_ref() == Some(&working_set) {
            return None;
        }
        let had_previous_working_set = self.last_health_working_set.is_some();
        self.last_health_working_set = Some(working_set);
        if targets.is_empty() {
            return had_previous_working_set.then_some(HealthCommand::Cancel);
        }
        let snapshot = Arc::make_mut(self.snapshot.as_mut()?);
        for target in &targets {
            if let Some(index) = snapshot.branch_index.get(&target.branch).copied()
                && let Some(branch) = Arc::make_mut(&mut snapshot.branches).get_mut(index)
                && matches!(
                    branch.graphite_health,
                    crate::model::GraphiteHealth::NotRequested
                        | crate::model::GraphiteHealth::Unavailable(_)
                )
            {
                branch.graphite_health = crate::model::GraphiteHealth::Checking;
            }
        }
        Some(HealthCommand::Request(HealthRequest {
            generation: snapshot.generation,
            targets: Arc::from(targets),
        }))
    }

    pub fn apply_upstream_batch(&mut self, batch: UpstreamBatch) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        if snapshot.generation != batch.generation {
            return;
        }
        let applicable: Vec<_> = batch
            .results
            .iter()
            .filter_map(|result| {
                let index = snapshot.branch_index.get(&result.branch).copied()?;
                (snapshot.branches.get(index)?.oid == result.oid).then_some((index, result))
            })
            .collect();
        if applicable.is_empty() {
            return;
        }
        let snapshot = Arc::make_mut(snapshot);
        let branches = Arc::make_mut(&mut snapshot.branches);
        for (index, result) in applicable {
            branches[index].remote_ref = result.evidence.clone();
            if let Some(configured_upstream) = &result.configured_upstream {
                branches[index].configured_upstream = configured_upstream.clone();
            }
        }
    }

    pub fn take_upstream_command(&mut self) -> Option<UpstreamCommand> {
        if self.upstream_cancel_pending {
            self.upstream_cancel_pending = false;
            return Some(UpstreamCommand::Cancel);
        }
        if !self.status_visible {
            return None;
        }
        if !self.upstream_request_pending {
            return None;
        }
        self.upstream_request_pending = false;
        let snapshot = self.snapshot.as_ref()?;
        let candidates = snapshot
            .branches
            .iter()
            .map(|branch| UpstreamTarget {
                branch: branch.id.clone(),
                oid: branch.oid.clone(),
                configured_upstream: branch.configured_upstream.clone(),
            })
            .collect::<Vec<_>>();
        let target_count = candidates.len().min(MAX_TARGETS);
        let start = self
            .upstream_target_offset
            .min(candidates.len().saturating_sub(1));
        let targets = candidates
            .iter()
            .cycle()
            .skip(start)
            .take(target_count)
            .cloned()
            .collect::<Vec<_>>();
        self.upstream_target_offset = if candidates.is_empty() {
            0
        } else {
            (start + target_count) % candidates.len()
        };
        let working_set = (
            snapshot.generation,
            targets
                .iter()
                .map(|target| (target.branch.clone(), target.oid.clone()))
                .collect(),
        );
        if self.last_upstream_working_set.as_ref() == Some(&working_set) {
            return None;
        }
        self.last_upstream_working_set = Some(working_set);
        if targets.is_empty() {
            return None;
        }
        let snapshot = Arc::make_mut(self.snapshot.as_mut()?);
        for target in &targets {
            if let Some(index) = snapshot.branch_index.get(&target.branch).copied()
                && let Some(branch) = Arc::make_mut(&mut snapshot.branches).get_mut(index)
                && matches!(
                    branch.remote_ref,
                    crate::model::RemoteRefEvidence::NotRequested
                        | crate::model::RemoteRefEvidence::Unavailable { .. }
                )
            {
                branch.remote_ref = crate::model::RemoteRefEvidence::Checking;
            }
        }
        Some(UpstreamCommand::Request(UpstreamRequest {
            generation: snapshot.generation,
            targets: Arc::from(targets),
        }))
    }

    pub fn mark_stale(&mut self, error: Arc<str>) {
        self.loading = false;
        self.refresh_error = Some(error);
    }

    pub fn take_github_command(&mut self) -> Option<GithubCommand> {
        if self.github_cancel_pending {
            self.github_cancel_pending = false;
            return Some(GithubCommand::Cancel);
        }
        if !self.status_visible || matches!(self.archive_mode, ArchiveMode::Archive) {
            return None;
        }
        if !self.github_request_pending {
            return None;
        }
        self.github_request_pending = false;
        let snapshot = self.snapshot.as_ref()?;
        let selected = self.selected.as_ref();
        let selected_stack =
            selected.and_then(|selected| self.topology.as_ref()?.stack_for(selected));
        let mut indexed = snapshot
            .branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| self.github_demand_targets.contains(&branch.id))
            .collect::<Vec<_>>();
        indexed.sort_by_key(|(index, branch)| {
            let priority = if branch.current {
                0
            } else if selected == Some(&branch.id) {
                1
            } else if selected_stack.is_some_and(|stack| {
                self.topology
                    .as_ref()
                    .and_then(|topology| topology.stack_for(&branch.id))
                    == Some(stack)
            }) {
                2
            } else {
                3
            };
            (priority, std::cmp::Reverse(branch.committed_at), *index)
        });
        let targets = indexed
            .into_iter()
            .take(crate::refresh::github::MAX_TARGETS)
            .map(|(_, branch)| GithubTarget {
                branch: branch.id.clone(),
                oid: branch.oid.clone(),
            })
            .collect::<Vec<_>>();
        self.github_demand_targets.clear();
        let working_set = (
            snapshot.generation,
            targets
                .iter()
                .map(|target| (target.branch.clone(), target.oid.clone()))
                .collect(),
        );
        if self.last_github_working_set.as_ref() == Some(&working_set) {
            return None;
        }
        self.last_github_working_set = Some(working_set);
        let discovery = snapshot
            .branches
            .iter()
            .map(|branch| GithubTarget {
                branch: branch.id.clone(),
                oid: branch.oid.clone(),
            })
            .collect::<Vec<_>>();
        let snapshot = Arc::make_mut(self.snapshot.as_mut()?);
        for target in &targets {
            if let Some(index) = snapshot.branch_index.get(&target.branch).copied()
                && let Some(branch) = Arc::make_mut(&mut snapshot.branches).get_mut(index)
            {
                branch.pr_lookup = PullRequestLookup::Checking;
            }
        }
        self.github_state = GitHubState::Loading;
        Some(GithubCommand::Request(GithubRequest {
            generation: snapshot.generation,
            targets: Arc::from(targets),
            discovery: Arc::from(discovery),
        }))
    }

    pub fn apply_github_batch(&mut self, batch: GithubBatch) {
        let previous_error = match &self.github_state {
            GitHubState::Unavailable(error) => Some(error.clone()),
            _ => None,
        };
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        if snapshot.generation != batch.generation {
            return;
        }
        let mut unavailable = None;
        let snapshot = Arc::make_mut(snapshot);
        for result in batch.results.iter() {
            let Some(index) = snapshot.branch_index.get(&result.branch).copied() else {
                continue;
            };
            let Some(branch) = Arc::make_mut(&mut snapshot.branches).get_mut(index) else {
                continue;
            };
            if branch.oid != result.oid {
                continue;
            }
            match &result.outcome {
                GithubOutcome::Ready(matches) => {
                    if let Some(mut pull_request) = best_pr_match(branch, matches) {
                        pull_request.match_quality =
                            if pull_request.head_oid.as_deref() == Some(branch.oid.as_ref()) {
                                crate::model::PullRequestMatch::ExactTip
                            } else {
                                crate::model::PullRequestMatch::StaleTip
                            };
                        branch.pr = Some(pull_request);
                        branch.pr_lookup = PullRequestLookup::Ready;
                    } else {
                        branch.pr = None;
                        branch.pr_lookup = PullRequestLookup::NoMatch;
                    }
                }
                GithubOutcome::NoMatch => {
                    branch.pr = None;
                    branch.pr_lookup = PullRequestLookup::NoMatch;
                }
                GithubOutcome::Unavailable(error) => {
                    unavailable.get_or_insert_with(|| error.clone());
                    branch.pr_lookup = PullRequestLookup::Unavailable(Arc::from(error.to_string()));
                }
            }
        }
        let has_retained_unavailable = snapshot
            .branches
            .iter()
            .any(|branch| matches!(branch.pr_lookup, PullRequestLookup::Unavailable(_)));
        self.github_state = if let Some(error) =
            unavailable.or_else(|| previous_error.filter(|_| has_retained_unavailable))
        {
            GitHubState::Unavailable(error)
        } else if snapshot
            .branches
            .iter()
            .any(|branch| branch.pr_lookup == PullRequestLookup::Checking)
        {
            GitHubState::Loading
        } else {
            GitHubState::Ready
        };
    }

    pub fn apply_prs(&mut self, matches: Vec<PrMatch>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let generation = snapshot.generation;
        let results = snapshot
            .branches
            .iter()
            .filter_map(|branch| {
                let candidates = matches
                    .iter()
                    .filter(|candidate| candidate.branch == branch.id)
                    .cloned()
                    .collect::<Vec<_>>();
                (!candidates.is_empty()).then(|| crate::refresh::github::GithubResult {
                    branch: branch.id.clone(),
                    oid: branch.oid.clone(),
                    outcome: GithubOutcome::Ready(Arc::from(candidates)),
                })
            })
            .collect::<Vec<_>>();
        if results.is_empty() {
            return;
        }
        self.apply_github_batch(GithubBatch {
            generation,
            results: Arc::from(results),
        });
    }

    pub fn mark_github_loading(&mut self) {
        self.github_state = GitHubState::Loading;
    }

    pub fn mark_github_failed(&mut self, error: GitHubError) {
        self.github_state = GitHubState::Unavailable(error);
    }

    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = if self.is_focused() && height >= 2 {
            height - 1
        } else {
            height.max(1)
        };
        if self.focus_needs_bottom_alignment {
            self.align_focused_bottom();
            self.focus_needs_bottom_alignment = false;
        } else {
            self.keep_selected_visible();
        }
    }

    pub fn request_current_startup(&mut self) {
        self.startup_current_pending = true;
    }

    pub fn is_focused(&self) -> bool {
        !matches!(self.scope, ViewScope::All)
    }

    pub fn focused_section_bounds(&self) -> Option<(usize, usize)> {
        if !self.is_focused() {
            return None;
        }
        let range = match &self.scope {
            ViewScope::Untrunked => self
                .projection
                .section_ranges
                .iter()
                .find(|range| range.section.is_none()),
            ViewScope::Stack { .. } | ViewScope::Trunk { .. } => {
                self.projection.section_ranges.first()
            }
            ViewScope::All => None,
        }?;
        Some((range.start, range.end))
    }

    pub fn sticky_visual_row(&self) -> Option<usize> {
        if !self.is_focused() {
            return None;
        }
        match &self.scope {
            ViewScope::Untrunked => self
                .projection
                .section_ranges
                .iter()
                .find(|range| range.section.is_none())
                .map(|range| range.bottom),
            ViewScope::Stack { .. } | ViewScope::Trunk { .. } => self
                .projection
                .section_ranges
                .first()
                .map(|range| range.bottom),
            ViewScope::All => None,
        }
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        if self.selected_label.is_some() {
            return None;
        }
        let selected = self.selected.as_ref()?;
        self.snapshot.as_ref()?.branch(selected)
    }

    pub fn handle_input(&mut self, input: Input) -> Action {
        if matches!(self.overlay, Overlay::StackNameEditor(_)) {
            return self.handle_stack_name_editor_key(input.key);
        }
        if input.phase == KeyPhase::Repeat && !input.key.allows_repeat() {
            return Action::None;
        }
        if input.phase == KeyPhase::Repeat
            && matches!(
                &self.overlay,
                Overlay::OrderPicker(_) | Overlay::ColorPicker(_) | Overlay::StackNameEditor(_)
            )
        {
            return Action::None;
        }
        let key = input.key;
        if matches!(key, Key::Quit | Key::Character('q')) {
            if matches!(
                self.mutation,
                MutationState::Restacking(_) | MutationState::Moving(_)
            ) {
                self.message = Some(Arc::from(
                    "quit deferred until the Graphite command finishes",
                ));
                return Action::None;
            }
            return Action::Quit;
        }
        if matches!(self.mutation, MutationState::ConfirmingDeletion(_)) {
            return self.handle_delete_confirmation(key);
        }
        if matches!(self.mutation, MutationState::ConfirmingCheckout(_)) {
            return self.handle_checkout_confirmation(key);
        }
        if matches!(self.mutation, MutationState::ConfirmingRestack(_)) {
            return self.handle_restack_confirmation(key);
        }
        if matches!(self.mutation, MutationState::ConfirmingMove(_)) {
            return self.handle_move_confirmation(key);
        }
        match &self.overlay {
            Overlay::Search => return self.handle_search_key(key),
            Overlay::Help => {
                return match key {
                    Key::Escape | Key::Character('?') => {
                        self.overlay = Overlay::None;
                        Action::None
                    }
                    _ => Action::None,
                };
            }
            Overlay::OrderPicker(_) => return self.handle_order_picker_key(key),
            Overlay::ColorPicker(_) => return self.handle_color_picker_key(key),
            Overlay::StackNameEditor(_) => unreachable!("name editor handled before global keys"),
            Overlay::ArchiveRange(_) => return self.handle_archive_range_key(key),
            Overlay::MovePreview(_) => return self.handle_move_preview_key(key),
            Overlay::None => {}
        }
        self.handle_normal_key(key)
    }

    pub fn handle_key(&mut self, key: Key) -> Action {
        self.handle_input(Input::press(key))
    }

    fn handle_normal_key(&mut self, key: Key) -> Action {
        match key {
            Key::Quit | Key::Character('q') => Action::Quit,
            Key::Up | Key::Character('k') => {
                self.move_selection(-1);
                Action::None
            }
            Key::Down | Key::Character('j') => {
                self.move_selection(1);
                Action::None
            }
            Key::StackUp | Key::Character('K') => {
                self.move_stack(-1);
                Action::None
            }
            Key::StackDown | Key::Character('J') => {
                self.move_stack(1);
                Action::None
            }
            Key::SectionUp | Key::Character('g') => {
                self.move_section(-1);
                Action::None
            }
            Key::SectionDown | Key::Character('G') => {
                self.move_section(1);
                Action::None
            }
            Key::Character('/') => {
                self.overlay = Overlay::Search;
                self.restore_selection = self.selected.clone();
                Action::None
            }
            Key::Character('R') => {
                self.demand_selected_graphite_health();
                Action::Refresh
            }
            Key::Character('r') => {
                self.demand_selected_graphite_health();
                self.begin_restack_confirmation();
                Action::None
            }
            Key::Character('m') => {
                self.demand_selected_graphite_health();
                self.begin_move_preview();
                Action::None
            }
            Key::Character('t') => {
                self.order_mode = match self.order_mode {
                    OrderMode::Recent => OrderMode::Graphite,
                    OrderMode::Graphite | OrderMode::Alphabetical | OrderMode::Chronological => {
                        OrderMode::Recent
                    }
                };
                self.reproject();
                Action::None
            }
            Key::Character('T') => {
                self.overlay = Overlay::OrderPicker(OrderPicker {
                    original: self.order_mode,
                    pending: if ORDER_OPTIONS.contains(&self.order_mode) {
                        self.order_mode
                    } else {
                        OrderMode::Recent
                    },
                });
                Action::None
            }
            Key::Character('+') => {
                self.adjust_lane_pitch(1);
                Action::None
            }
            Key::Character('-') => {
                self.adjust_lane_pitch(-1);
                Action::None
            }
            Key::Character('0') => {
                if self.lane_pitch != LanePitch::Auto {
                    self.lane_pitch = LanePitch::Auto;
                    self.message = Some(Arc::from("lane pitch: Auto"));
                }
                Action::None
            }
            Key::Character('h') => {
                self.toggle_stack_scope();
                Action::None
            }
            Key::Character('H') => {
                self.toggle_trunk_scope();
                Action::None
            }
            Key::Character('s') => {
                self.status_visible = !self.status_visible;
                if self.status_visible {
                    self.upstream_request_pending = true;
                    self.demand_current_graphite_health();
                    self.health_request_pending = !self.health_demand_targets.is_empty();
                    self.last_upstream_working_set = None;
                    self.last_health_working_set = None;
                    self.demand_current_github();
                    self.github_request_pending = !self.github_demand_targets.is_empty();
                    self.last_github_working_set = None;
                } else {
                    self.upstream_request_pending = false;
                    self.health_request_pending = false;
                    self.upstream_cancel_pending = true;
                    self.health_cancel_pending = true;
                    self.github_request_pending = false;
                    self.github_cancel_pending = true;
                    self.github_demand_targets.clear();
                    if let Some(snapshot) = self.snapshot.as_mut() {
                        let snapshot = Arc::make_mut(snapshot);
                        for branch in Arc::make_mut(&mut snapshot.branches) {
                            if matches!(
                                branch.graphite_health,
                                crate::model::GraphiteHealth::Checking
                            ) {
                                branch.graphite_health = crate::model::GraphiteHealth::NotRequested;
                            }
                            if branch.pr_lookup == PullRequestLookup::Checking {
                                branch.pr_lookup = PullRequestLookup::NotRequested;
                            }
                        }
                    }
                }
                self.message = Some(Arc::from(if self.status_visible {
                    "status columns shown"
                } else {
                    "status columns hidden"
                }));
                Action::None
            }
            Key::Character('S') => {
                self.separators = !self.separators;
                self.reproject();
                Action::None
            }
            Key::Character('d') => {
                self.detail_sidebar = !self.detail_sidebar;
                self.message = Some(Arc::from(if self.detail_sidebar {
                    "detail sidebar enabled"
                } else {
                    "detail sidebar hidden"
                }));
                Action::None
            }
            Key::Character('a') => {
                self.toggle_archive_mode();
                Action::None
            }
            Key::Character('v') => {
                self.begin_archive_range();
                Action::None
            }
            Key::Character('x') => self.toggle_selected_archive(),
            Key::Character('X') => {
                self.begin_delete_confirmation();
                Action::None
            }
            Key::Character('?') => {
                self.overlay = Overlay::Help;
                Action::None
            }
            Key::CopyBranch => self
                .clipboard_request(ClipboardScope::Branch)
                .map(Action::Copy)
                .unwrap_or(Action::None),
            Key::CopySection => self
                .clipboard_request(ClipboardScope::Section)
                .map(Action::Copy)
                .unwrap_or(Action::None),
            Key::CopyStack => self
                .clipboard_request(ClipboardScope::Stack)
                .map(Action::Copy)
                .unwrap_or(Action::None),
            Key::Character('c') => self.cycle_color(),
            Key::Character('C') => {
                self.open_color_picker();
                Action::None
            }
            Key::Character('n') => {
                self.open_stack_name_editor();
                Action::None
            }
            Key::Character('i') => self.toggle_visual_section(),
            Key::Character('o') => match self.selected_url() {
                Some(url) => Action::OpenUrl(url),
                None => {
                    self.demand_selected_github();
                    Action::None
                }
            },
            Key::Character('O') => self
                .stack_pr_urls()
                .map(Action::OpenUrls)
                .unwrap_or(Action::None),
            Key::Character('y') => match self.selected_url() {
                Some(text) => Action::Copy(ClipboardRequest {
                    text,
                    scope: ClipboardScope::PullRequest,
                    branch_count: 1,
                }),
                None => {
                    self.demand_selected_github();
                    Action::None
                }
            },
            Key::Enter if matches!(self.mutation, MutationState::Idle) => {
                if let Some(target) = self.selected_label.clone() {
                    self.open_name_editor_for_target(target);
                    Action::None
                } else if let Some(reason) = self.checkout_disabled_reason() {
                    self.message = Some(Arc::from(reason));
                    Action::None
                } else if let Some(branch) = self.selected.clone() {
                    self.mutation = MutationState::ConfirmingCheckout(branch);
                    Action::None
                } else {
                    Action::None
                }
            }
            Key::Escape => {
                self.message = None;
                self.notice = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn cycle_color(&mut self) -> Action {
        let Some(common_dir) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.common_dir.clone())
        else {
            return Action::None;
        };
        let Some(target) = self.selected_color_target() else {
            return Action::None;
        };
        let current_index = COLOR_OPTIONS
            .iter()
            .position(|(_, value)| *value == self.target_color(&target))
            .unwrap_or(0);
        let next = (1..=COLOR_OPTIONS.len())
            .map(|offset| COLOR_OPTIONS[(current_index + offset) % COLOR_OPTIONS.len()].1)
            .find(|value| self.valid_color_choice(&target, *value))
            .flatten();
        if let Err(error) = self.set_target_color_in_memory(&target, next) {
            self.message = Some(Arc::from(format!("stack color was not changed: {error}")));
            return Action::None;
        }
        self.persist_target_color(target, next.map(Arc::from), common_dir)
    }

    fn persist_color(
        &mut self,
        root: BranchId,
        value: Option<Arc<str>>,
        common_dir: PathBuf,
    ) -> Action {
        self.message = Some(Arc::from(match value.as_deref() {
            Some(color) => format!("stack color set to {color}; saving"),
            None => "stack color reset to automatic; saving".to_owned(),
        }));
        let mut mutation = ConfigMutation::default();
        mutation.set_color(root, value);
        Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
    }

    fn persist_target_color(
        &mut self,
        target: ConfigTarget,
        value: Option<Arc<str>>,
        common_dir: PathBuf,
    ) -> Action {
        match target {
            ConfigTarget::Stack(stack) => self.persist_color(stack, value, common_dir),
            ConfigTarget::VisualSection(anchor) => {
                let Some(mut section) = self.config.visual_section(&anchor).cloned() else {
                    return Action::None;
                };
                let Some(value) = value else {
                    self.message = Some(Arc::from("visual sections require a concrete color"));
                    return Action::None;
                };
                section.color = value.to_string();
                let mut mutation = ConfigMutation::default();
                mutation.set_visual_section(anchor, Some(section));
                self.message = Some(Arc::from("section color changed; saving"));
                Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
            }
        }
    }

    fn adjust_lane_pitch(&mut self, delta: i16) {
        let next = self.lane_pitch.adjust(delta);
        if next == self.lane_pitch {
            return;
        }
        self.lane_pitch = next;
        if let LanePitch::Fixed(value) = next {
            self.message = Some(Arc::from(format!("lane pitch: {value}")));
        }
    }

    fn toggle_visual_section(&mut self) -> Action {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from(
                "visual section boundaries can only be toggled on branches",
            ));
            return Action::None;
        }
        let Some(anchor) = self.selected.clone() else {
            return Action::None;
        };
        let Some(row) = self.projection.row_for(&anchor) else {
            return Action::None;
        };
        if row.is_trunk {
            self.message = Some(Arc::from("trunk rows cannot start visual sections"));
            return Action::None;
        }
        let Some(common_dir) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.common_dir.clone())
        else {
            return Action::None;
        };
        let mut mutation = ConfigMutation::default();
        if self.config.visual_section(&anchor).is_some() {
            if let Err(error) = self.config.set_visual_section_in_memory(&anchor, None) {
                self.message = Some(Arc::from(format!("section was not removed: {error}")));
                return Action::None;
            }
            mutation.set_visual_section(anchor, None);
            self.message = Some(Arc::from("visual section removed; saving"));
        } else {
            let used = self.adjacent_section_colors(&anchor, &row.stack_id);
            let choices: Vec<_> = COLOR_OPTIONS
                .iter()
                .filter_map(|(_, value)| *value)
                .collect();
            let start = anchor
                .0
                .bytes()
                .fold(0usize, |sum, byte| sum.wrapping_add(byte as usize))
                % choices.len();
            let color = (0..choices.len())
                .map(|offset| choices[(start + offset) % choices.len()])
                .find(|color| !used.iter().any(|used| used == color))
                .unwrap_or(choices[start]);
            let section = VisualSection {
                color: color.to_owned(),
                name: None,
            };
            if let Err(error) = self
                .config
                .set_visual_section_in_memory(&anchor, Some(section.clone()))
            {
                self.message = Some(Arc::from(format!("section was not created: {error}")));
                return Action::None;
            }
            mutation.set_visual_section(anchor, Some(section));
            self.message = Some(Arc::from("visual section created; saving"));
        }
        self.reproject();
        Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
    }

    fn handle_order_picker_key(&mut self, key: Key) -> Action {
        let Overlay::OrderPicker(mut picker) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Up => {
                let index = order_option_index(picker.pending);
                picker.pending = ORDER_OPTIONS[index.saturating_sub(1)];
                self.overlay = Overlay::OrderPicker(picker);
            }
            Key::Down => {
                let index = order_option_index(picker.pending);
                picker.pending = ORDER_OPTIONS[(index + 1).min(ORDER_OPTIONS.len() - 1)];
                self.overlay = Overlay::OrderPicker(picker);
            }
            Key::Enter => {
                self.overlay = Overlay::None;
                if self.order_mode != picker.pending {
                    self.order_mode = picker.pending;
                    self.reproject();
                }
            }
            Key::Escape => self.overlay = Overlay::None,
            _ => {}
        }
        Action::None
    }

    fn open_color_picker(&mut self) {
        let Some(target) = self.selected_color_target() else {
            return;
        };
        let original = self.target_color(&target).map(Arc::from);
        let choice_index = COLOR_OPTIONS
            .iter()
            .position(|(_, value)| *value == original.as_deref())
            .unwrap_or(0);
        self.overlay = Overlay::ColorPicker(ColorPicker {
            target,
            pending: original.clone(),
            original,
            choice_index,
        });
    }

    fn selected_color_target(&mut self) -> Option<ConfigTarget> {
        if let Some(target) = self.selected_label.clone() {
            return Some(target);
        }
        let row = self
            .selected
            .as_ref()
            .and_then(|selected| self.projection.row_for(selected))?;
        if row.is_trunk {
            self.message = Some(Arc::from("trunk rows do not have a stack color"));
            return None;
        }
        Some(
            row.visual_section
                .clone()
                .map(ConfigTarget::VisualSection)
                .unwrap_or_else(|| ConfigTarget::Stack(row.stack_id.clone())),
        )
    }

    fn handle_color_picker_key(&mut self, key: Key) -> Action {
        let Overlay::ColorPicker(mut picker) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Up | Key::Down => {
                let direction = if key == Key::Up { -1 } else { 1 };
                let original_index = picker.choice_index;
                for _ in 0..COLOR_OPTIONS.len() {
                    picker.choice_index = picker
                        .choice_index
                        .saturating_add_signed(direction)
                        .min(COLOR_OPTIONS.len() - 1);
                    if self.valid_color_choice(&picker.target, COLOR_OPTIONS[picker.choice_index].1)
                    {
                        break;
                    }
                    if picker.choice_index == 0 && direction < 0
                        || picker.choice_index + 1 == COLOR_OPTIONS.len() && direction > 0
                    {
                        if !self.valid_color_choice(
                            &picker.target,
                            COLOR_OPTIONS[picker.choice_index].1,
                        ) {
                            picker.choice_index = original_index;
                        }
                        break;
                    }
                }
                picker.pending = COLOR_OPTIONS[picker.choice_index].1.map(Arc::from);
                if let Err(error) =
                    self.set_target_color_in_memory(&picker.target, picker.pending.as_deref())
                {
                    self.overlay = Overlay::None;
                    self.message = Some(Arc::from(format!(
                        "color picker closed after validation failed: {error}"
                    )));
                    return Action::None;
                }
                self.reproject();
                self.overlay = Overlay::ColorPicker(picker);
                Action::None
            }
            Key::Enter => {
                if !self.config_target_is_valid(&picker.target) {
                    self.close_invalid_color_picker(&picker);
                    return Action::None;
                }
                let Some(common_dir) = self
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.common_dir.clone())
                else {
                    self.close_invalid_color_picker(&picker);
                    return Action::None;
                };
                self.overlay = Overlay::None;
                self.persist_target_color(picker.target, picker.pending, common_dir)
            }
            Key::Escape => {
                if let Err(error) =
                    self.set_target_color_in_memory(&picker.target, picker.original.as_deref())
                {
                    self.message = Some(Arc::from(format!(
                        "color preview could not be restored: {error}"
                    )));
                }
                self.overlay = Overlay::None;
                self.reproject();
                Action::None
            }
            _ => Action::None,
        }
    }

    fn open_stack_name_editor(&mut self) {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("press Enter to edit the selected name"));
            return;
        }
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(row) = self.projection.row_for(selected) else {
            return;
        };
        if row.is_trunk {
            self.message = Some(Arc::from("trunk rows cannot be named as stacks"));
            return;
        }
        let target = row
            .visual_section
            .clone()
            .map(ConfigTarget::VisualSection)
            .unwrap_or_else(|| ConfigTarget::Stack(row.stack_id.clone()));
        self.begin_name_editor(target);
    }

    fn open_name_editor_for_target(&mut self, target: ConfigTarget) {
        if !self.config_target_is_valid(&target) {
            return;
        }
        self.begin_name_editor(target);
    }

    fn begin_name_editor(&mut self, target: ConfigTarget) {
        let draft = match &target {
            ConfigTarget::Stack(stack) => self.config.stack_name(stack),
            ConfigTarget::VisualSection(anchor) => self
                .config
                .visual_section(anchor)
                .and_then(|section| section.name.as_deref()),
        }
        .unwrap_or_default()
        .to_owned();
        let cursor = draft.chars().count();
        self.overlay = Overlay::StackNameEditor(StackNameEditor {
            target: target.clone(),
            draft,
            cursor,
        });
        self.selected_label = Some(target);
        self.reproject();
    }

    fn handle_stack_name_editor_key(&mut self, key: Key) -> Action {
        let Overlay::StackNameEditor(mut editor) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Escape => {
                let existed = match &editor.target {
                    ConfigTarget::Stack(stack) => self.config.stack_name(stack).is_some(),
                    ConfigTarget::VisualSection(anchor) => self
                        .config
                        .visual_section(anchor)
                        .is_some_and(|section| section.name.is_some()),
                };
                self.overlay = Overlay::None;
                if !existed {
                    self.selected = Some(editor.target.branch().clone());
                    self.selected_label = None;
                }
                self.reproject();
                Action::None
            }
            Key::Backspace => {
                if editor.cursor > 0 {
                    let mut characters = editor.draft.chars().collect::<Vec<_>>();
                    characters.remove(editor.cursor - 1);
                    editor.cursor -= 1;
                    editor.draft = characters.into_iter().collect();
                }
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::ClearNameDraft => {
                editor.draft.clear();
                editor.cursor = 0;
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Delete => {
                let mut characters = editor.draft.chars().collect::<Vec<_>>();
                if editor.cursor < characters.len() {
                    characters.remove(editor.cursor);
                    editor.draft = characters.into_iter().collect();
                }
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Left => {
                editor.cursor = editor.cursor.saturating_sub(1);
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Right => {
                editor.cursor = (editor.cursor + 1).min(editor.draft.chars().count());
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Home => {
                editor.cursor = 0;
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::End => {
                editor.cursor = editor.draft.chars().count();
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Character(character)
                if !character.is_control()
                    && editor.draft.chars().count() < MAX_STACK_NAME_CHARS =>
            {
                let mut characters = editor.draft.chars().collect::<Vec<_>>();
                characters.insert(editor.cursor, character);
                editor.cursor += 1;
                editor.draft = characters.into_iter().collect();
                self.overlay = Overlay::StackNameEditor(editor);
                self.reproject();
                Action::None
            }
            Key::Enter => {
                if !self.config_target_is_valid(&editor.target) {
                    self.close_invalid_stack_name_editor();
                    return Action::None;
                }
                let value = match editor.draft.trim() {
                    "" => None,
                    value => Some(Arc::<str>::from(value)),
                };
                let Some(common_dir) = self
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.common_dir.clone())
                else {
                    self.close_invalid_stack_name_editor();
                    return Action::None;
                };
                let update = self.set_target_name_in_memory(&editor.target, value.as_deref());
                if let Err(error) = update {
                    self.overlay = Overlay::None;
                    self.message = Some(Arc::from(format!("stack name was not changed: {error}")));
                    return Action::None;
                }
                let mut mutation = ConfigMutation::default();
                match &editor.target {
                    ConfigTarget::Stack(stack) => {
                        mutation.set_stack_name(stack.clone(), value.clone())
                    }
                    ConfigTarget::VisualSection(anchor) => {
                        let mut section = self
                            .config
                            .visual_section(anchor)
                            .cloned()
                            .expect("valid section editor target");
                        section.name = value.as_deref().map(str::to_owned);
                        mutation.set_visual_section(anchor.clone(), Some(section));
                    }
                }
                self.overlay = Overlay::None;
                if value.is_none() {
                    self.selected = Some(editor.target.branch().clone());
                    self.selected_label = None;
                }
                self.message = Some(Arc::from(match value {
                    Some(name) => format!("name set to {name}; saving"),
                    None => "name cleared; saving".to_owned(),
                }));
                self.reproject();
                Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
            }
            _ => Action::None,
        }
    }

    fn revalidate_overlay(&mut self) {
        match self.overlay.clone() {
            Overlay::ColorPicker(picker) => {
                if self.config_target_is_valid(&picker.target) {
                    if let Err(error) =
                        self.set_target_color_in_memory(&picker.target, picker.pending.as_deref())
                    {
                        self.overlay = Overlay::None;
                        self.message = Some(Arc::from(format!(
                            "color picker closed after validation failed: {error}"
                        )));
                    }
                } else {
                    self.close_invalid_color_picker(&picker);
                }
            }
            Overlay::StackNameEditor(editor) => {
                if !self.config_target_is_valid(&editor.target) {
                    self.close_invalid_stack_name_editor();
                }
            }
            Overlay::MovePreview(preview) => {
                if self.snapshot.as_ref().is_none_or(|snapshot| {
                    snapshot.branch(&preview.source).is_none()
                        || snapshot.branch(&preview.target).is_none()
                }) {
                    self.overlay = Overlay::None;
                    self.message = Some(Arc::from("repository changed; move preview cancelled"));
                }
            }
            Overlay::None
            | Overlay::Search
            | Overlay::Help
            | Overlay::OrderPicker(_)
            | Overlay::ArchiveRange(_) => {}
        }
    }

    fn stack_target_is_valid(&self, target: &BranchId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.branch(target).is_some())
            && self.topology.as_ref().is_some_and(|topology| {
                !topology.is_trunk(target) && topology.stack_for(target) == Some(target)
            })
    }

    fn config_target_is_valid(&self, target: &ConfigTarget) -> bool {
        match target {
            ConfigTarget::Stack(stack) => self.stack_target_is_valid(stack),
            ConfigTarget::VisualSection(anchor) => {
                self.config.visual_section(anchor).is_some()
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.branch(anchor).is_some())
                    && self
                        .topology
                        .as_ref()
                        .is_some_and(|topology| !topology.is_trunk(anchor))
            }
        }
    }

    fn target_color<'a>(&'a self, target: &'a ConfigTarget) -> Option<&'a str> {
        match target {
            ConfigTarget::Stack(stack) => self.config.color(stack),
            ConfigTarget::VisualSection(anchor) => self
                .config
                .visual_section(anchor)
                .map(|section| section.color.as_str()),
        }
    }

    fn valid_color_choice(&self, target: &ConfigTarget, value: Option<&str>) -> bool {
        match target {
            ConfigTarget::Stack(stack) => value.is_none_or(|color| {
                !self
                    .adjacent_section_colors(stack, stack)
                    .iter()
                    .any(|used| used == color)
            }),
            ConfigTarget::VisualSection(anchor) => value.is_some_and(|color| {
                let stack = self
                    .topology
                    .as_ref()
                    .and_then(|topology| topology.stack_for(anchor))
                    .unwrap_or(anchor);
                !self
                    .adjacent_section_colors(anchor, stack)
                    .iter()
                    .any(|used| used == color)
            }),
        }
    }

    fn adjacent_section_colors(&self, anchor: &BranchId, stack: &BranchId) -> Vec<String> {
        let mut boundaries: Vec<_> = self
            .projection
            .rows
            .iter()
            .filter_map(|row| {
                let boundary = row.visual_section.as_ref()?;
                let color = row.visual_color.as_ref()?;
                (row.stack_id == *stack)
                    .then(|| (row.manual_depth, boundary.clone(), color.to_string()))
            })
            .collect();
        boundaries.sort_by_key(|(depth, _, _)| *depth);
        boundaries.dedup_by(|left, right| left.1 == right.1);
        if anchor == stack {
            return boundaries
                .first()
                .map(|(_, _, color)| vec![color.clone()])
                .unwrap_or_default();
        }
        let anchor_depth = boundaries
            .iter()
            .find(|(_, boundary, _)| boundary == anchor)
            .map(|(depth, _, _)| *depth)
            .or_else(|| {
                self.projection
                    .row_for(anchor)
                    .map(|row| row.manual_depth.saturating_add(1))
            })
            .unwrap_or(1);
        let mut colors = Vec::with_capacity(2);
        if let Some((_, _, color)) = boundaries.iter().find(|(depth, boundary, _)| {
            *depth == anchor_depth.saturating_add(1) && boundary != anchor
        }) {
            colors.push(color.clone());
        }
        if let Some((_, _, color)) = boundaries
            .iter()
            .find(|(depth, boundary, _)| *depth + 1 == anchor_depth && boundary != anchor)
        {
            colors.push(color.clone());
        } else {
            colors.push(self.stack_color_hex(stack).to_owned());
        }
        colors
    }

    fn stack_color_hex<'a>(&'a self, stack: &'a BranchId) -> &'a str {
        if let Some(color) = self.config.color(stack) {
            return color;
        }
        const COLORS: [&str; 8] = [
            "#7aa2f7", "#bb9af7", "#7dcfff", "#ff9e64", "#9ece6a", "#f7768e", "#2ac3de", "#c0caf5",
        ];
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.repository_id.as_ref())
            .unwrap_or_default()
            .hash(&mut hasher);
        stack.hash(&mut hasher);
        COLORS[hasher.finish() as usize % COLORS.len()]
    }

    fn set_target_color_in_memory(
        &mut self,
        target: &ConfigTarget,
        color: Option<&str>,
    ) -> anyhow::Result<()> {
        match target {
            ConfigTarget::Stack(stack) => self.config.set_color_in_memory(stack, color),
            ConfigTarget::VisualSection(anchor) => {
                let mut section = self
                    .config
                    .visual_section(anchor)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("visual section no longer exists"))?;
                let color = color
                    .ok_or_else(|| anyhow::anyhow!("visual sections require a concrete color"))?;
                section.color = color.to_owned();
                self.config
                    .set_visual_section_in_memory(anchor, Some(section))
            }
        }
    }

    fn set_target_name_in_memory(
        &mut self,
        target: &ConfigTarget,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        match target {
            ConfigTarget::Stack(stack) => self.config.set_stack_name_in_memory(stack, name),
            ConfigTarget::VisualSection(anchor) => {
                let mut section = self
                    .config
                    .visual_section(anchor)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("visual section no longer exists"))?;
                section.name = name.map(str::to_owned);
                self.config
                    .set_visual_section_in_memory(anchor, Some(section))
            }
        }
    }

    fn close_invalid_color_picker(&mut self, picker: &ColorPicker) {
        if let Err(error) =
            self.set_target_color_in_memory(&picker.target, picker.original.as_deref())
        {
            self.message = Some(Arc::from(format!(
                "color preview could not be restored: {error}"
            )));
        }
        self.overlay = Overlay::None;
        self.message = Some(Arc::from(
            "color picker closed because its stack is no longer available",
        ));
    }

    fn close_invalid_stack_name_editor(&mut self) {
        self.overlay = Overlay::None;
        self.message = Some(Arc::from(
            "name editor closed because its stack is no longer available",
        ));
    }

    pub fn finish_config_persistence(&mut self, sequence: u64, result: anyhow::Result<()>) {
        if result.is_ok() {
            self.pending_colors
                .retain(|_, (pending_sequence, _)| *pending_sequence > sequence);
            self.pending_archives
                .retain(|_, (pending_sequence, _)| *pending_sequence > sequence);
            self.pending_stack_names
                .retain(|_, (pending_sequence, _)| *pending_sequence > sequence);
            self.pending_visual_sections
                .retain(|_, (pending_sequence, _)| *pending_sequence > sequence);
        }
        if sequence != self.latest_config_sequence {
            return;
        }
        self.message = Some(Arc::from(match result {
            Ok(()) => "configuration saved".to_owned(),
            Err(error) => format!("config save failed; in-memory choice retained: {error}"),
        }));
    }

    pub fn prepare_pending_config_persistence(
        &self,
        mut request: ConfigWriteRequest,
    ) -> Option<ConfigWriteRequest> {
        request.mutation = self.pending_config_mutation();
        if request.mutation.is_empty() {
            return None;
        }
        request.fallback = self.config.clone();
        Some(request)
    }

    pub fn finish_color_persistence(&mut self, sequence: u64, result: anyhow::Result<()>) {
        self.finish_config_persistence(sequence, result);
    }

    pub fn prepare_pending_color_persistence(
        &self,
        request: ConfigWriteRequest,
    ) -> Option<ConfigWriteRequest> {
        self.prepare_pending_config_persistence(request)
    }

    pub fn take_config_write_request(&mut self) -> Option<ConfigWriteRequest> {
        self.queued_config_write.take()
    }

    fn register_config_mutation(
        &mut self,
        mutation: ConfigMutation,
        common_dir: PathBuf,
    ) -> ConfigWriteRequest {
        self.next_config_sequence = self.next_config_sequence.wrapping_add(1).max(1);
        let sequence = self.next_config_sequence;
        self.latest_config_sequence = sequence;
        for (branch, color) in mutation.color_updates {
            self.pending_colors.insert(branch, (sequence, color));
        }
        for (branch, update) in mutation.archive_updates {
            self.pending_archives.insert(branch, (sequence, update));
        }
        for (stack, name) in mutation.stack_name_updates {
            self.pending_stack_names.insert(stack, (sequence, name));
        }
        for (anchor, section) in mutation.visual_section_updates {
            self.pending_visual_sections
                .insert(anchor, (sequence, section));
        }
        ConfigWriteRequest {
            sequence,
            common_dir,
            mutation: self.pending_config_mutation(),
            fallback: self.config.clone(),
        }
    }

    fn pending_config_mutation(&self) -> ConfigMutation {
        ConfigMutation {
            color_updates: self
                .pending_colors
                .iter()
                .map(|(branch, (_, color))| (branch.clone(), color.clone()))
                .collect(),
            archive_updates: self
                .pending_archives
                .iter()
                .map(|(branch, (_, update))| (branch.clone(), *update))
                .collect(),
            stack_name_updates: self
                .pending_stack_names
                .iter()
                .map(|(stack, (_, name))| (stack.clone(), name.clone()))
                .collect(),
            visual_section_updates: self
                .pending_visual_sections
                .iter()
                .map(|(anchor, (_, section))| (anchor.clone(), section.clone()))
                .collect(),
        }
    }

    fn toggle_archive_mode(&mut self) {
        self.overlay = Overlay::None;
        match self.archive_mode {
            ArchiveMode::Active => {
                self.active_view_state = Some(AllViewState {
                    selected: self.selected.clone(),
                    scroll: self.scroll,
                });
                self.archive_mode = ArchiveMode::Archive;
                self.github_request_pending = false;
                self.github_cancel_pending = true;
                self.github_demand_targets.clear();
                self.selected = None;
                self.scroll = 0;
                self.reproject();
            }
            ArchiveMode::Archive => {
                self.archive_mode = ArchiveMode::Active;
                self.reproject();
                if let Some(state) = self.active_view_state.take() {
                    self.selected = state
                        .selected
                        .filter(|branch| self.projection.branch_to_visual.contains_key(branch))
                        .or_else(|| self.projection.selectable.first().cloned());
                    self.scroll = state.scroll.min(
                        self.projection
                            .entries
                            .len()
                            .saturating_sub(self.viewport_height),
                    );
                    self.keep_selected_visible();
                }
                if self.status_visible {
                    self.demand_current_github();
                    self.github_request_pending = !self.github_demand_targets.is_empty();
                    self.last_github_working_set = None;
                }
            }
        }
    }

    fn toggle_selected_archive(&mut self) -> Action {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("labels cannot be archived"));
            return Action::None;
        }
        let Some(branch_id) = self.selected.clone() else {
            return Action::None;
        };
        if let Some(reason) = self.archive_refusal(&branch_id) {
            self.message = Some(Arc::from(reason));
            return Action::None;
        }
        let archived = matches!(self.archive_mode, ArchiveMode::Active);
        let selected_index = self
            .projection
            .branch_to_selectable
            .get(&branch_id)
            .copied()
            .unwrap_or(0);
        let selection_after = selected_index
            .checked_sub(1)
            .and_then(|index| self.projection.selectable.get(index))
            .or_else(|| self.projection.selectable.get(selected_index + 1))
            .cloned();
        let Some(common_dir) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.common_dir.clone())
        else {
            self.message = Some(Arc::from("repository changed; branch was not archived"));
            return Action::None;
        };
        self.config.set_archived_in_memory(&branch_id, archived);
        let mut mutation = ConfigMutation::default();
        mutation.set_archived(branch_id.clone(), archived);
        self.message = Some(Arc::from(if archived {
            format!("archived {branch_id} locally; saving")
        } else {
            format!("restored {branch_id}; saving")
        }));
        self.selected = selection_after;
        self.reproject();
        Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
    }

    fn archive_refusal(&self, branch: &BranchId) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        let branch_state = snapshot.branch(branch)?;
        if branch_state.current {
            return Some(format!(
                "{} is current and cannot be archived",
                branch_state.id
            ));
        }
        if self
            .topology
            .as_ref()
            .is_some_and(|topology| topology.is_trunk(branch))
        {
            return Some(format!("{branch} is a trunk and cannot be archived"));
        }
        None
    }

    fn begin_archive_range(&mut self) {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("archive ranges must start on a branch"));
            return;
        }
        let Some(anchor) = self.selected.clone() else {
            return;
        };
        if let Some(reason) = self.archive_refusal(&anchor) {
            self.message = Some(Arc::from(reason));
            return;
        }
        let Some(section) = self.section_for_branch(&anchor) else {
            self.message = Some(Arc::from("selected branch is outside a visible section"));
            return;
        };
        let eligible: Arc<[BranchId]> = self.archive_eligible_in_section(section.as_ref()).into();
        let eligible_index: HashMap<_, _> = eligible
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, branch)| (branch, index))
            .collect();
        let Some(&anchor_index) = eligible_index.get(&anchor) else {
            self.message = Some(Arc::from("selected branch cannot start an archive range"));
            return;
        };
        self.overlay = Overlay::ArchiveRange(ArchiveRange {
            mode: self.archive_mode,
            section,
            endpoint: anchor.clone(),
            anchor,
            eligible,
            eligible_index: Arc::new(eligible_index),
            anchor_index,
            endpoint_index: anchor_index,
        });
    }

    fn handle_archive_range_key(&mut self, key: Key) -> Action {
        let Overlay::ArchiveRange(mut range) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Escape => {
                self.overlay = Overlay::None;
                Action::None
            }
            Key::Up | Key::StackUp | Key::Character('k') | Key::Character('K') => {
                self.move_archive_range(&mut range, -1);
                self.overlay = Overlay::ArchiveRange(range);
                Action::None
            }
            Key::Down | Key::StackDown | Key::Character('j') | Key::Character('J') => {
                self.move_archive_range(&mut range, 1);
                self.overlay = Overlay::ArchiveRange(range);
                Action::None
            }
            Key::Enter => self.commit_archive_range(range),
            _ => Action::None,
        }
    }

    fn move_archive_range(&mut self, range: &mut ArchiveRange, delta: isize) {
        let next = range.endpoint_index.saturating_add_signed(delta);
        if next >= range.eligible.len() || next == range.endpoint_index {
            self.message = Some(Arc::from("archive range reached the section boundary"));
            return;
        }
        range.endpoint_index = next;
        range.endpoint = range.eligible[next].clone();
    }

    fn commit_archive_range(&mut self, range: ArchiveRange) -> Action {
        self.overlay = Overlay::None;
        if range.mode != self.archive_mode {
            self.message = Some(Arc::from("archive range changed; nothing was modified"));
            return Action::None;
        }
        let eligible = self.archive_eligible_in_section(range.section.as_ref());
        let expected = archive_range_slice(&eligible, &range.anchor, &range.endpoint);
        if expected != Some(range.branches()) {
            self.message = Some(Arc::from(
                "repository changed; archive range was not modified",
            ));
            return Action::None;
        }
        let archived = matches!(self.archive_mode, ArchiveMode::Active);
        let Some(common_dir) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.common_dir.clone())
        else {
            self.message = Some(Arc::from(
                "repository changed; archive range was not modified",
            ));
            return Action::None;
        };
        let mut mutation = ConfigMutation::default();
        for branch in range.branches() {
            self.config.set_archived_in_memory(branch, archived);
            mutation.set_archived(branch.clone(), archived);
        }
        self.message = Some(Arc::from(format!(
            "{} {} branches; saving",
            if archived { "archived" } else { "restored" },
            range.branch_count()
        )));
        self.reproject();
        Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
    }

    fn archive_eligible_in_section(&self, section: Option<&BranchId>) -> Vec<BranchId> {
        let Some(range) = self
            .projection
            .section_ranges
            .iter()
            .find(|range| range.section.as_ref() == section)
        else {
            return Vec::new();
        };
        let start = self
            .projection
            .selectable_visual_rows
            .partition_point(|visual| *visual < range.start);
        let end = self
            .projection
            .selectable_visual_rows
            .partition_point(|visual| *visual <= range.end);
        self.projection.selectable[start..end]
            .iter()
            .filter(|branch| self.archive_refusal(branch).is_none())
            .cloned()
            .collect()
    }

    fn section_for_branch(&self, branch: &BranchId) -> Option<Option<BranchId>> {
        let visual = *self.projection.branch_to_visual.get(branch)?;
        self.projection
            .section_ranges
            .iter()
            .find(|range| visual >= range.start && visual <= range.end)
            .map(|range| range.section.clone())
    }

    pub fn archive_range_contains(&self, branch: &BranchId) -> bool {
        matches!(&self.overlay, Overlay::ArchiveRange(range) if range.contains(branch))
    }

    fn deletion_target_in_flight(&self) -> Option<BranchId> {
        match &self.mutation {
            MutationState::ConfirmingDeletion(confirmation)
            | MutationState::Deleting(confirmation) => Some(confirmation.request.branch.clone()),
            MutationState::Reconciling {
                operation: ReconciliationOperation::Deletion { request, .. },
                ..
            } => Some(request.branch.clone()),
            MutationState::Idle
            | MutationState::ConfirmingCheckout(_)
            | MutationState::CheckingOut(_)
            | MutationState::Reconciling { .. }
            | MutationState::DeletionBlocked(_)
            | MutationState::ConfirmingRestack(_)
            | MutationState::Restacking(_)
            | MutationState::ConfirmingMove(_)
            | MutationState::Moving(_)
            | MutationState::GraphiteBlocked(_) => None,
        }
    }

    fn cleanup_deleted_config_identity(
        &mut self,
        snapshot: &RepositorySnapshot,
        target: &BranchId,
    ) {
        let mut mutation = ConfigMutation::default();
        if self.config.is_archived(target) {
            mutation.prune_archived([target.clone()]);
        }
        if self.config.color(target).is_some() {
            mutation.set_color(target.clone(), None);
        }
        if self.config.stack_name(target).is_some() {
            mutation.set_stack_name(target.clone(), None);
        }
        if self.config.visual_section(target).is_some() {
            mutation.set_visual_section(target.clone(), None);
        }
        if mutation.is_empty() {
            return;
        }
        if let Err(error) = self.config.apply_mutation_in_memory(&mutation) {
            self.message = Some(Arc::from(format!(
                "deleted branch configuration could not be cleaned: {error}"
            )));
            return;
        }
        self.queued_config_write =
            Some(self.register_config_mutation(mutation, snapshot.common_dir.clone()));
    }

    fn prune_invalid_archives(&mut self, protected: Option<&BranchId>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let local: HashSet<&str> = snapshot
            .branches
            .iter()
            .map(|branch| branch.id.0.as_ref())
            .collect();
        let protected_rows: HashSet<&str> = snapshot
            .branches
            .iter()
            .filter(|branch| branch.current)
            .map(|branch| branch.id.0.as_ref())
            .chain(
                snapshot
                    .configured_trunks
                    .iter()
                    .map(|trunk| trunk.0.as_ref()),
            )
            .collect();
        let invalid = self
            .config
            .archived
            .iter()
            .filter(|branch| {
                protected_rows.contains(branch.as_str())
                    || (!local.contains(branch.as_str())
                        && protected
                            .is_none_or(|protected| branch.as_str() != protected.0.as_ref()))
            })
            .cloned()
            .map(BranchId::new)
            .collect::<Vec<_>>();
        if invalid.is_empty() {
            return;
        }
        let common_dir = snapshot.common_dir.clone();
        let mut mutation = ConfigMutation::default();
        mutation.prune_archived(invalid);
        if let Err(error) = self.config.apply_mutation_in_memory(&mutation) {
            self.message = Some(Arc::from(format!(
                "invalid archive entries could not be pruned: {error}"
            )));
            return;
        }
        self.queued_config_write = Some(self.register_config_mutation(mutation, common_dir));
    }

    fn prune_invalid_visual_sections(&mut self) {
        let (Some(snapshot), Some(topology)) = (self.snapshot.as_ref(), self.topology.as_ref())
        else {
            return;
        };
        let invalid: Vec<_> = self
            .config
            .visual_sections
            .keys()
            .map(|anchor| BranchId::new(anchor.clone()))
            .filter(|anchor| {
                snapshot.branch(anchor).is_none()
                    || topology.is_trunk(anchor)
                    || topology.stack_for(anchor).is_none()
            })
            .collect();
        if invalid.is_empty() {
            return;
        }
        let common_dir = snapshot.common_dir.clone();
        let mut mutation = ConfigMutation::default();
        for anchor in invalid {
            mutation.set_visual_section(anchor, None);
        }
        if let Err(error) = self.config.apply_mutation_in_memory(&mutation) {
            self.message = Some(Arc::from(format!(
                "invalid visual sections could not be pruned: {error}"
            )));
            return;
        }
        self.queued_config_write = Some(self.register_config_mutation(mutation, common_dir));
    }

    fn nearby_selection_after(&self, target: &BranchId) -> Option<BranchId> {
        if self.selected.as_ref() != Some(target) {
            return None;
        }
        let index = self.projection.branch_to_selectable.get(target).copied()?;
        self.projection
            .selectable
            .get(index + 1)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|index| self.projection.selectable.get(index))
            })
            .cloned()
    }

    fn handle_search_key(&mut self, key: Key) -> Action {
        match key {
            Key::Enter => {
                self.overlay = Overlay::None;
            }
            Key::Escape => {
                self.filter.clear();
                self.overlay = Overlay::None;
                if let Some(previous) = self.restore_selection.take() {
                    self.selected = Some(previous);
                }
            }
            Key::Backspace => {
                self.filter.pop();
            }
            Key::Character(character) if !character.is_control() => self.filter.push(character),
            _ => {}
        }
        self.reproject();
        Action::None
    }

    fn handle_checkout_confirmation(&mut self, key: Key) -> Action {
        let MutationState::ConfirmingCheckout(target) = &self.mutation else {
            return Action::None;
        };
        let target = target.clone();
        match key {
            Key::Enter
                if self.selected.as_ref() == Some(&target) && self.selected_label.is_none() =>
            {
                if let Some(reason) = self.checkout_disabled_reason() {
                    self.mutation = MutationState::Idle;
                    self.message = Some(Arc::from(reason));
                    Action::None
                } else {
                    self.mutation = MutationState::CheckingOut(target.clone());
                    Action::Checkout(target)
                }
            }
            Key::Enter | Key::Escape => {
                self.mutation = MutationState::Idle;
                self.message = Some(Arc::from("checkout cancelled"));
                Action::None
            }
            _ => {
                self.mutation = MutationState::Idle;
                self.handle_normal_key(key)
            }
        }
    }

    fn selected_url(&self) -> Option<Arc<str>> {
        if matches!(self.archive_mode, ArchiveMode::Archive) {
            return None;
        }
        self.selected_branch()?.pr.as_ref().map(|pr| pr.url.clone())
    }

    fn stack_pr_urls(&self) -> Option<Vec<Arc<str>>> {
        if matches!(self.archive_mode, ArchiveMode::Archive) || self.selected_label.is_some() {
            return None;
        }
        let selected = self.selected.as_ref()?;
        let snapshot = self.snapshot.as_ref()?;
        let topology = self.topology.as_ref()?;
        let branches = topology.stack_branches_for(selected)?;
        let mut urls = Vec::new();
        let mut seen = HashSet::new();
        for branch_id in branches {
            let Some(url) = snapshot
                .branch(branch_id)
                .and_then(|branch| branch.pr.as_ref())
                .map(|pr| Arc::clone(&pr.url))
            else {
                continue;
            };
            if seen.insert(Arc::clone(&url)) {
                urls.push(url);
            }
        }
        (!urls.is_empty()).then_some(urls)
    }

    fn clipboard_request(&self, scope: ClipboardScope) -> Option<ClipboardRequest> {
        let target = if let Some(label) = &self.selected_label {
            label.branch()
        } else {
            let selected = self.selected.as_ref()?;
            self.projection
                .navigation
                .iter()
                .any(|target| {
                    matches!(
                        target,
                        crate::model::topology::SelectionTarget::Branch(branch)
                            if branch == selected
                    )
                })
                .then_some(selected)?
        };

        let branches = match scope {
            ClipboardScope::PullRequest => return None,
            ClipboardScope::Branch => vec![target.clone()],
            ClipboardScope::Stack => self.topology.as_ref()?.stack_branches_for(target)?.to_vec(),
            ClipboardScope::Section => {
                let group = self.topology.as_ref()?.stack_branches_for(target)?;
                let anchor = match &self.selected_label {
                    Some(ConfigTarget::VisualSection(anchor)) => anchor,
                    Some(ConfigTarget::Stack(_)) => group.first()?,
                    None => group[..=group.iter().position(|branch| branch == target)?]
                        .iter()
                        .rev()
                        .find(|branch| self.config.visual_section(branch).is_some())
                        .unwrap_or(group.first()?),
                };
                let start = group.iter().position(|branch| branch == anchor)?;
                group[start..].to_vec()
            }
        };
        let text = branches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        Some(ClipboardRequest {
            text: Arc::from(text),
            scope,
            branch_count: branches.len(),
        })
    }

    fn demand_selected_graphite_health(&mut self) {
        if !self.status_visible {
            return;
        }
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(stack) = self
            .topology
            .as_ref()
            .and_then(|topology| topology.stack_for(selected))
            .cloned()
        else {
            return;
        };
        self.health_demand_targets.extend(
            self.snapshot
                .as_ref()
                .into_iter()
                .flat_map(|snapshot| snapshot.branches.iter())
                .filter(|branch| {
                    branch.diff_parent.is_some()
                        && self
                            .topology
                            .as_ref()
                            .and_then(|topology| topology.stack_for(&branch.id))
                            == Some(&stack)
                })
                .map(|branch| branch.id.clone()),
        );
        self.health_request_pending = !self.health_demand_targets.is_empty();
    }

    fn demand_selected_github(&mut self) {
        if !self.status_visible || matches!(self.archive_mode, ArchiveMode::Archive) {
            self.message = Some(Arc::from("show status before checking pull requests"));
            return;
        }
        let Some(selected) = self.selected.clone() else {
            return;
        };
        self.github_demand_targets.insert(selected);
        self.github_request_pending = true;
        self.last_github_working_set = None;
        self.message = Some(Arc::from("checking selected branch for a pull request"));
    }

    fn demand_current_github(&mut self) {
        let Some(current) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.branches.iter().find(|branch| branch.current))
            .map(|branch| branch.id.clone())
        else {
            return;
        };
        self.github_demand_targets.insert(current.clone());
        let Some(stack) = self
            .topology
            .as_ref()
            .and_then(|topology| topology.stack_for(&current))
            .cloned()
        else {
            return;
        };
        self.github_demand_targets.extend(
            self.snapshot
                .as_ref()
                .into_iter()
                .flat_map(|snapshot| snapshot.branches.iter())
                .filter(|branch| {
                    self.topology
                        .as_ref()
                        .and_then(|topology| topology.stack_for(&branch.id))
                        == Some(&stack)
                })
                .map(|branch| branch.id.clone()),
        );
    }

    fn demand_current_graphite_health(&mut self) {
        let Some(current) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.branches.iter().find(|branch| branch.current))
            .map(|branch| branch.id.clone())
        else {
            return;
        };
        let Some(stack) = self
            .topology
            .as_ref()
            .and_then(|topology| topology.stack_for(&current))
        else {
            return;
        };
        self.health_demand_targets.extend(
            self.snapshot
                .as_ref()
                .into_iter()
                .flat_map(|snapshot| snapshot.branches.iter())
                .filter(|branch| {
                    self.topology
                        .as_ref()
                        .and_then(|topology| topology.stack_for(&branch.id))
                        == Some(stack)
                        && matches!(
                            branch.graphite_health,
                            crate::model::GraphiteHealth::NotRequested
                        )
                })
                .map(|branch| branch.id.clone()),
        );
    }

    fn graphite_descendants(&self, source: &BranchId) -> Vec<BranchId> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let mut descendants = Vec::new();
        let mut pending = vec![source.clone()];
        while let Some(parent) = pending.pop() {
            let mut children: Vec<_> = snapshot
                .branches
                .iter()
                .filter(|branch| branch.diff_parent.as_ref() == Some(&parent))
                .map(|branch| branch.id.clone())
                .collect();
            children.sort();
            for child in children.into_iter().rev() {
                descendants.push(child.clone());
                pending.push(child);
            }
        }
        descendants
    }

    fn graphite_expectation(&self, branch: &BranchId) -> Option<GraphiteBranchExpectation> {
        self.snapshot
            .as_ref()?
            .branch(branch)
            .map(|branch| GraphiteBranchExpectation {
                branch: branch.id.clone(),
                oid: branch.oid.clone(),
            })
    }

    fn graphite_edge_expectations<'a>(
        &self,
        branches: impl IntoIterator<Item = &'a BranchId>,
    ) -> Arc<[GraphiteEdgeExpectation]> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Arc::from([]);
        };
        Arc::from(
            branches
                .into_iter()
                .filter_map(|id| {
                    snapshot.branch(id).map(|branch| GraphiteEdgeExpectation {
                        branch: id.clone(),
                        parent: branch.diff_parent.clone(),
                    })
                })
                .collect::<Vec<_>>(),
        )
    }

    fn graphite_action_refusal(&self, branch: &Branch) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        if self.refresh_error.is_some() {
            return Some("repository data is stale; press R to refresh".into());
        }
        if !matches!(snapshot.state, crate::model::RepositoryState::Ready) {
            return Some(format!(
                "Graphite action disabled while repository is {:?}",
                snapshot.state
            ));
        }
        if snapshot
            .branches
            .iter()
            .any(|candidate| candidate.current && candidate.dirty)
        {
            return Some("Graphite action requires a clean current worktree".into());
        }
        if snapshot.configured_trunks.contains(&branch.id) {
            return Some("configured trunks cannot be moved or restacked".into());
        }
        if branch.graphite != crate::model::GraphiteProvenance::Tracked {
            return Some("Graphite action requires a tracked branch".into());
        }
        if branch.id.0.starts_with('-') {
            return Some("option-shaped branch names cannot be mutated safely".into());
        }
        None
    }

    fn begin_restack_confirmation(&mut self) {
        if self.selected_label.is_some() || !matches!(self.mutation, MutationState::Idle) {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if let Some(reason) = self.graphite_action_refusal(branch) {
            self.message = Some(Arc::from(reason));
            return;
        }
        let recorded_parent = match &branch.graphite_health {
            crate::model::GraphiteHealth::NeedsRestack { recorded_parent } => recorded_parent,
            crate::model::GraphiteHealth::NotRequested
            | crate::model::GraphiteHealth::Checking
            | crate::model::GraphiteHealth::Unavailable(_)
                if self.status_visible =>
            {
                self.message = Some(Arc::from(
                    "checking Graphite ancestry; press r again when ready",
                ));
                return;
            }
            _ => {
                self.message = Some(Arc::from("selected branch does not need restacking"));
                return;
            }
        };
        let source = GraphiteBranchExpectation {
            branch: branch.id.clone(),
            oid: branch.oid.clone(),
        };
        let affected: Vec<_> = std::iter::once(branch.id.clone())
            .chain(self.graphite_descendants(&branch.id))
            .filter_map(|id| self.graphite_expectation(&id))
            .collect();
        let topology =
            self.graphite_edge_expectations(affected.iter().map(|expectation| &expectation.branch));
        self.mutation = MutationState::ConfirmingRestack(RestackRequest {
            repository_id: snapshot.repository_id.clone(),
            source,
            expected_parent: recorded_parent.clone(),
            affected: Arc::from(affected),
            topology,
        });
    }

    fn handle_restack_confirmation(&mut self, key: Key) -> Action {
        let MutationState::ConfirmingRestack(request) = &self.mutation else {
            return Action::None;
        };
        let request = request.clone();
        match key {
            Key::Enter => {
                self.mutation = MutationState::Restacking(request.clone());
                Action::Restack(request)
            }
            Key::Escape => {
                self.mutation = MutationState::Idle;
                self.message = Some(Arc::from("restack cancelled"));
                Action::None
            }
            _ => Action::None,
        }
    }

    fn valid_move_targets(&self, preview: &MovePreview) -> Vec<BranchId> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let Some(source_trunk) = snapshot
            .branch(&preview.source)
            .and_then(|branch| branch.trunk.as_ref())
        else {
            return Vec::new();
        };
        snapshot
            .branches
            .iter()
            .filter(|branch| {
                branch.id != preview.source
                    && !preview.affected.contains(&branch.id)
                    && branch.graphite == crate::model::GraphiteProvenance::Tracked
                    && !snapshot.configured_trunks.contains(&branch.id)
                    && branch.trunk.as_ref() == Some(source_trunk)
            })
            .map(|branch| branch.id.clone())
            .collect()
    }

    fn begin_move_preview(&mut self) {
        if self.selected_label.is_some() || !matches!(self.mutation, MutationState::Idle) {
            return;
        }
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if let Some(reason) = self.graphite_action_refusal(branch) {
            self.message = Some(Arc::from(reason));
            return;
        }
        let source = branch.id.clone();
        let affected: Arc<[BranchId]> = Arc::from(self.graphite_descendants(&source));
        let affected_expectations: Arc<[GraphiteBranchExpectation]> = Arc::from(
            std::iter::once(source.clone())
                .chain(affected.iter().cloned())
                .filter_map(|id| self.graphite_expectation(&id))
                .collect::<Vec<_>>(),
        );
        let source_expectation = self
            .graphite_expectation(&source)
            .expect("selected branch exists");
        let original_parent = branch.diff_parent.clone();
        let topology =
            self.graphite_edge_expectations(std::iter::once(&source).chain(affected.iter()));
        let seed = MovePreview {
            source: source.clone(),
            target: source,
            only: false,
            affected,
            affected_expectations,
            source_expectation: source_expectation.clone(),
            target_expectation: source_expectation,
            original_parent,
            topology,
        };
        let Some(target) = self.valid_move_targets(&seed).into_iter().next() else {
            self.message = Some(Arc::from("no valid Graphite move target is available"));
            return;
        };
        let Some(target_expectation) = self.graphite_expectation(&target) else {
            return;
        };
        self.overlay = Overlay::MovePreview(MovePreview {
            target,
            target_expectation,
            ..seed
        });
        self.reproject();
    }

    fn handle_move_preview_key(&mut self, key: Key) -> Action {
        let Overlay::MovePreview(mut preview) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Up | Key::Down | Key::Character('j') | Key::Character('k') => {
                let targets = self.valid_move_targets(&preview);
                if let Some(index) = targets.iter().position(|target| target == &preview.target) {
                    let delta = if matches!(key, Key::Up | Key::Character('k')) {
                        -1
                    } else {
                        1
                    };
                    let next = index
                        .saturating_add_signed(delta)
                        .min(targets.len().saturating_sub(1));
                    preview.target = targets[next].clone();
                    if let Some(expectation) = self.graphite_expectation(&preview.target) {
                        preview.target_expectation = expectation;
                    }
                }
                self.overlay = Overlay::MovePreview(preview);
                self.reproject();
            }
            Key::Tab => {
                preview.only = !preview.only;
                self.overlay = Overlay::MovePreview(preview);
                self.reproject();
            }
            Key::Enter => {
                let Some(snapshot) = self.snapshot.as_ref() else {
                    return Action::None;
                };
                if self.graphite_expectation(&preview.source).as_ref()
                    != Some(&preview.source_expectation)
                    || self.graphite_expectation(&preview.target).as_ref()
                        != Some(&preview.target_expectation)
                    || snapshot
                        .branch(&preview.source)
                        .and_then(|branch| branch.diff_parent.clone())
                        != preview.original_parent
                {
                    self.overlay = Overlay::None;
                    self.message = Some(Arc::from("repository changed; move preview cancelled"));
                    self.reproject();
                    return Action::None;
                }
                let affected_ids: Vec<_> = if preview.only {
                    std::iter::once(preview.source.clone())
                        .chain(
                            preview
                                .topology
                                .iter()
                                .filter(|edge| edge.parent.as_ref() == Some(&preview.source))
                                .map(|edge| edge.branch.clone()),
                        )
                        .collect()
                } else {
                    std::iter::once(preview.source.clone())
                        .chain(preview.affected.iter().cloned())
                        .collect()
                };
                let affected: Vec<_> = preview
                    .affected_expectations
                    .iter()
                    .filter(|expectation| affected_ids.contains(&expectation.branch))
                    .cloned()
                    .collect();
                self.overlay = Overlay::None;
                self.mutation = MutationState::ConfirmingMove(MoveRequest {
                    repository_id: snapshot.repository_id.clone(),
                    source: preview.source_expectation,
                    target: preview.target_expectation,
                    expected_parent: preview.original_parent,
                    affected: Arc::from(affected),
                    topology: preview.topology,
                    only: preview.only,
                });
                self.reproject();
            }
            Key::Escape => {
                self.overlay = Overlay::None;
                self.message = Some(Arc::from("move preview cancelled"));
                self.reproject();
            }
            _ => {}
        }
        Action::None
    }

    fn handle_move_confirmation(&mut self, key: Key) -> Action {
        let MutationState::ConfirmingMove(request) = &self.mutation else {
            return Action::None;
        };
        let request = request.clone();
        match key {
            Key::Enter => {
                self.mutation = MutationState::Moving(request.clone());
                Action::Move(request)
            }
            Key::Escape => {
                let affected: Arc<[BranchId]> = Arc::from(
                    request
                        .affected
                        .iter()
                        .filter(|item| item.branch != request.source.branch)
                        .map(|item| item.branch.clone())
                        .collect::<Vec<_>>(),
                );
                self.overlay = Overlay::MovePreview(MovePreview {
                    source: request.source.branch.clone(),
                    target: request.target.branch.clone(),
                    only: request.only,
                    affected,
                    affected_expectations: request.affected.clone(),
                    source_expectation: request.source.clone(),
                    target_expectation: request.target.clone(),
                    original_parent: request.expected_parent.clone(),
                    topology: request.topology.clone(),
                });
                self.mutation = MutationState::Idle;
                self.message = Some(Arc::from("returned to move preview"));
                self.reproject();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn begin_delete_confirmation(&mut self) {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("labels cannot be deleted"));
            return;
        }
        if !matches!(self.mutation, MutationState::Idle) {
            self.message = Some(Arc::from("another repository mutation is active"));
            return;
        }
        if self.viewport_height < 9 {
            self.message = Some(Arc::from(
                "terminal is too short to show deletion safety text",
            ));
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if let Some(refusal) = deletion_refusal(snapshot, branch, self.refresh_error.as_deref()) {
            self.message = Some(Arc::from(refusal));
            return;
        }
        self.mutation = MutationState::ConfirmingDeletion(DeleteConfirmation {
            repository_id: snapshot.repository_id.clone(),
            request: DeleteRequest {
                branch: branch.id.clone(),
                expected_oid: branch.oid.clone(),
                expected_provenance: branch.graphite,
            },
            pr_number: branch.pr.as_ref().map(|pr| pr.number),
            selection_after: self.nearby_selection_after(&branch.id),
        });
    }

    fn handle_delete_confirmation(&mut self, key: Key) -> Action {
        match key {
            Key::Character('y') => {
                if let Some(error) = &self.refresh_error {
                    self.mutation = MutationState::Idle;
                    self.message = Some(Arc::from(stale_deletion_refusal(error)));
                    return Action::None;
                }
                let MutationState::ConfirmingDeletion(confirmation) = &self.mutation else {
                    return Action::None;
                };
                let request = confirmation.request.clone();
                self.mutation = MutationState::Deleting(confirmation.clone());
                Action::Delete(request)
            }
            Key::Character('n') | Key::Escape => {
                self.mutation = MutationState::Idle;
                self.message = Some(Arc::from("deletion cancelled"));
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn finish_checkout(&mut self, result: anyhow::Result<()>, request_epoch: u64) {
        self.finish_checkout_at(result, request_epoch, Instant::now());
    }

    pub fn finish_checkout_at(
        &mut self,
        result: anyhow::Result<()>,
        request_epoch: u64,
        now: Instant,
    ) {
        let MutationState::CheckingOut(target) = &self.mutation else {
            return;
        };
        let operation = ReconciliationOperation::Checkout {
            target: target.clone(),
            command_error: result.err().map(|error| Arc::from(error.to_string())),
        };
        self.mutation = MutationState::Reconciling {
            operation,
            request_epoch,
            deadline: now + self.reconciliation_timeout,
        };
    }

    pub fn finish_deletion(&mut self, result: anyhow::Result<DeleteOutcome>, request_epoch: u64) {
        self.finish_deletion_at(result, request_epoch, Instant::now());
    }

    pub fn finish_deletion_at(
        &mut self,
        result: anyhow::Result<DeleteOutcome>,
        request_epoch: u64,
        now: Instant,
    ) {
        match result {
            Ok(DeleteOutcome::Inconsistent) => {
                let message: Arc<str> = Arc::from(
                    "deletion result is inconsistent; inspect Git/Graphite before retrying",
                );
                self.message = Some(message.clone());
                self.mutation = MutationState::DeletionBlocked(message);
            }
            result => {
                let MutationState::Deleting(confirmation) = &self.mutation else {
                    return;
                };
                let result = match result {
                    Ok(outcome) => DeletionResult::Outcome(outcome),
                    Err(error) => DeletionResult::Error(Arc::from(error.to_string())),
                };
                self.mutation = MutationState::Reconciling {
                    operation: ReconciliationOperation::Deletion {
                        request: confirmation.request.clone(),
                        result,
                        selection_after: confirmation.selection_after.clone(),
                    },
                    request_epoch,
                    deadline: now + self.reconciliation_timeout,
                };
            }
        }
    }

    pub fn finish_graphite_action(
        &mut self,
        result: anyhow::Result<GraphiteMutationOutcome>,
        request_epoch: u64,
        now: Instant,
    ) {
        let operation = match &self.mutation {
            MutationState::Restacking(request) => ReconciliationOperation::Restack {
                request: request.clone(),
                result: match result {
                    Ok(GraphiteMutationOutcome::Inconsistent) => {
                        let message: Arc<str> = Arc::from(
                            "Graphite restack left inconsistent state; inspect Git/Graphite",
                        );
                        self.message = Some(message.clone());
                        self.mutation = MutationState::GraphiteBlocked(message);
                        return;
                    }
                    Ok(outcome) => GraphiteActionResult::Outcome(outcome),
                    Err(error) => GraphiteActionResult::Error(Arc::from(error.to_string())),
                },
            },
            MutationState::Moving(request) => ReconciliationOperation::Move {
                request: request.clone(),
                result: match result {
                    Ok(GraphiteMutationOutcome::Inconsistent) => {
                        let message: Arc<str> = Arc::from(
                            "Graphite move left inconsistent state; inspect Git/Graphite",
                        );
                        self.message = Some(message.clone());
                        self.mutation = MutationState::GraphiteBlocked(message);
                        return;
                    }
                    Ok(outcome) => GraphiteActionResult::Outcome(outcome),
                    Err(error) => GraphiteActionResult::Error(Arc::from(error.to_string())),
                },
            },
            _ => return,
        };
        self.mutation = MutationState::Reconciling {
            operation,
            request_epoch,
            deadline: now + self.reconciliation_timeout,
        };
    }

    pub fn mutation_progress(&self) -> Option<String> {
        match &self.mutation {
            MutationState::ConfirmingCheckout(target) => {
                Some(format!("checkout {target}? Enter confirm · Esc cancel"))
            }
            MutationState::CheckingOut(target) => Some(format!("switching to {target}…")),
            MutationState::Deleting(confirmation) => {
                Some(format!("deleting {} locally…", confirmation.request.branch))
            }
            MutationState::Restacking(request) => {
                Some(format!("restacking {}…", request.source.branch))
            }
            MutationState::Moving(request) => Some(format!(
                "moving {} onto {}…",
                request.source.branch, request.target.branch
            )),
            MutationState::Reconciling { operation, .. } => Some(match operation {
                ReconciliationOperation::Checkout { target, .. } => {
                    format!("verifying checkout of {target}…")
                }
                ReconciliationOperation::Deletion { request, .. } => {
                    format!("verifying deletion of {}…", request.branch)
                }
                ReconciliationOperation::Restack { request, .. } => {
                    format!("verifying restack of {}…", request.source.branch)
                }
                ReconciliationOperation::Move { request, .. } => {
                    format!("verifying move of {}…", request.source.branch)
                }
            }),
            MutationState::DeletionBlocked(message) => Some(format!("DELETION BLOCKED: {message}")),
            MutationState::GraphiteBlocked(message) => Some(format!("GRAPHITE BLOCKED: {message}")),
            MutationState::Idle
            | MutationState::ConfirmingDeletion(_)
            | MutationState::ConfirmingRestack(_)
            | MutationState::ConfirmingMove(_) => None,
        }
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_ref())
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now >= notice.expires_at)
        {
            self.notice = None;
            changed = true;
        }
        let timed_out = match &self.mutation {
            MutationState::Reconciling {
                operation,
                deadline,
                ..
            } if now >= *deadline => Some(operation.clone()),
            _ => None,
        };
        if let Some(operation) = timed_out {
            let target = operation.target();
            self.mutation = MutationState::Idle;
            self.set_notice(
                Arc::from(format!(
                    "could not verify {} for {target}; press R to refresh",
                    operation.description()
                )),
                now,
            );
            changed = true;
        }
        changed
    }

    fn set_notice(&mut self, text: Arc<str>, now: Instant) {
        self.notice = Some(TransientNotice {
            text,
            expires_at: now + self.notice_duration,
        });
    }

    pub fn checkout_running(&self) -> bool {
        matches!(self.mutation, MutationState::CheckingOut(_))
    }

    fn checkout_disabled_reason(&self) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        let branch = self.selected_branch()?;
        if branch.current {
            return Some(format!("{} is already current", branch.id));
        }
        if !matches!(snapshot.state, crate::model::RepositoryState::Ready) {
            return Some(format!(
                "checkout disabled while repository is {:?}",
                snapshot.state
            ));
        }
        None
    }

    fn reproject(&mut self) {
        if self.snapshot.is_some() && self.topology.is_some() {
            self.validate_scope();
            let preview_topology = match &self.overlay {
                Overlay::MovePreview(preview) => self.move_preview_topology(preview),
                _ => None,
            };
            let Some(topology) = preview_topology.as_ref().or(self.topology.as_ref()) else {
                return;
            };
            let scope = self.resolved_scope();
            let mut stack_names: HashMap<BranchId, Arc<str>> = self
                .config
                .stack_names
                .iter()
                .map(|(stack, name)| (BranchId::new(stack.clone()), Arc::from(name.as_str())))
                .collect();
            let mut visual_sections: HashMap<BranchId, crate::model::topology::VisualSectionSpec> =
                self.config
                    .visual_sections
                    .iter()
                    .map(|(anchor, section)| {
                        (
                            BranchId::new(anchor.clone()),
                            crate::model::topology::VisualSectionSpec {
                                color: Arc::from(section.color.as_str()),
                                name: section.name.as_deref().map(Arc::from),
                            },
                        )
                    })
                    .collect();
            if let Overlay::StackNameEditor(editor) = &self.overlay {
                match &editor.target {
                    ConfigTarget::Stack(stack) => {
                        stack_names.insert(stack.clone(), Arc::from(editor.draft.as_str()));
                    }
                    ConfigTarget::VisualSection(anchor) => {
                        if let Some(section) = visual_sections.get_mut(anchor) {
                            section.name = Some(Arc::from(editor.draft.as_str()));
                        }
                    }
                }
            }
            let stack_colors = visual_sections
                .keys()
                .filter_map(|anchor| topology.stack_for(anchor).cloned())
                .map(|stack| {
                    let color = Arc::from(self.stack_color_hex(&stack));
                    (stack, color)
                })
                .collect();
            self.projection = topology.project(&ProjectionOptions {
                order: self.order_mode,
                scope,
                separators: self.separators,
                filter: self.filter.clone(),
                archive_mode: self.archive_mode,
                archived: self
                    .config
                    .archived
                    .iter()
                    .cloned()
                    .map(BranchId::new)
                    .collect(),
                stack_names,
                stack_colors,
                visual_sections,
            });
            if matches!(self.scope, ViewScope::Untrunked) {
                self.restrict_projection_to_untrunked();
            }
            if self.filter.is_empty()
                && let Some(previous) = self.restore_selection.take()
                && self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.branch(&previous).is_some())
            {
                self.selected = Some(previous);
            }
            self.reconcile_selection();
            if self.restore_all_after_reproject {
                self.restore_all_after_reproject = false;
                if let Some(state) = self.all_view_state.take() {
                    self.selected = state
                        .selected
                        .filter(|selected| self.projection.branch_to_visual.contains_key(selected));
                    if self.selected.is_none() {
                        self.selected = self.projection.selectable.first().cloned();
                    }
                    self.scroll = state.scroll.min(
                        self.projection
                            .entries
                            .len()
                            .saturating_sub(self.viewport_height),
                    );
                }
            }
        }
    }

    fn move_preview_topology(&self, preview: &MovePreview) -> Option<TopologyIndex> {
        let snapshot = self.snapshot.as_ref()?;
        let target_trunk = snapshot.branch(&preview.target)?.trunk.clone();
        let original_parent = snapshot
            .branch(&preview.source)
            .and_then(|branch| branch.diff_parent.clone());
        let mut preview_snapshot = (**snapshot).clone();
        let branches = Arc::make_mut(&mut preview_snapshot.branches);
        for branch in branches.iter_mut() {
            if branch.id == preview.source {
                branch.parent = (!snapshot.configured_trunks.contains(&preview.target))
                    .then(|| preview.target.clone());
                branch.diff_parent = Some(preview.target.clone());
                branch.trunk = target_trunk.clone().or_else(|| {
                    snapshot
                        .configured_trunks
                        .contains(&preview.target)
                        .then(|| preview.target.clone())
                });
                branch.stack_root = preview.source.clone();
            } else if preview.only && branch.diff_parent.as_ref() == Some(&preview.source) {
                branch.diff_parent = original_parent.clone();
                branch.parent = original_parent.clone().and_then(|parent| {
                    (!snapshot.configured_trunks.contains(&parent)).then_some(parent)
                });
            } else if !preview.only && preview.affected.contains(&branch.id) {
                branch.trunk = target_trunk.clone().or_else(|| {
                    snapshot
                        .configured_trunks
                        .contains(&preview.target)
                        .then(|| preview.target.clone())
                });
            }
        }
        preview_snapshot.branch_index = RepositorySnapshot::index_branches(branches);
        Some(TopologyIndex::build(&preview_snapshot))
    }

    fn validate_scope(&mut self) {
        let Some(topology) = self.topology.as_ref() else {
            return;
        };
        match &self.scope {
            ViewScope::All => {}
            ViewScope::Stack { anchor }
                if topology.stack_for(anchor).is_none() && !topology.is_trunk(anchor) =>
            {
                self.restore_all_after_reproject = true;
                self.scope = ViewScope::All;
                self.message = Some(Arc::from("view anchor disappeared; showing all branches"));
            }
            ViewScope::Trunk { anchor }
                if topology.trunk_for(anchor).is_none() && !topology.is_trunk(anchor) =>
            {
                self.restore_all_after_reproject = true;
                self.scope = ViewScope::All;
                self.message = Some(Arc::from(
                    "view anchor no longer has a configured trunk; showing all branches",
                ));
            }
            ViewScope::Untrunked
                if !self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .branches
                        .iter()
                        .any(|branch| branch.trunk.is_none() && !topology.is_trunk(&branch.id))
                }) =>
            {
                self.restore_all_after_reproject = true;
                self.scope = ViewScope::All;
                self.message = Some(Arc::from(
                    "Untrunked section disappeared; showing all branches",
                ));
            }
            _ => {}
        }
    }

    fn resolved_scope(&self) -> ProjectionScope {
        let Some(topology) = self.topology.as_ref() else {
            return ProjectionScope::All;
        };
        match &self.scope {
            ViewScope::All => ProjectionScope::All,
            ViewScope::Stack { anchor } => ProjectionScope::Stack(anchor.clone()),
            ViewScope::Trunk { anchor } => topology
                .trunk_for(anchor)
                .cloned()
                .or_else(|| topology.is_trunk(anchor).then(|| anchor.clone()))
                .map(ProjectionScope::Trunk)
                .unwrap_or(ProjectionScope::All),
            ViewScope::Untrunked => ProjectionScope::All,
        }
    }

    fn toggle_stack_scope(&mut self) {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("select a branch to focus its stack"));
            return;
        }
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let desired = if self
            .topology
            .as_ref()
            .is_some_and(|topology| topology.is_trunk(&selected))
        {
            ViewScope::Trunk { anchor: selected }
        } else {
            ViewScope::Stack { anchor: selected }
        };
        if self.scope_matches(&desired) {
            self.exit_focus();
        } else {
            self.enter_scope(desired);
        }
    }

    fn toggle_trunk_scope(&mut self) {
        if self.selected_label.is_some() {
            self.message = Some(Arc::from("select a branch to focus its trunk"));
            return;
        }
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(topology) = self.topology.as_ref() else {
            return;
        };
        let desired = topology
            .trunk_for(&selected)
            .cloned()
            .or_else(|| topology.is_trunk(&selected).then(|| selected.clone()))
            .map(|anchor| ViewScope::Trunk { anchor })
            .unwrap_or(ViewScope::Untrunked);
        if self.scope_matches(&desired) {
            self.exit_focus();
        } else {
            self.enter_scope(desired);
        }
    }

    pub fn scope_label(&self) -> String {
        match &self.scope {
            ViewScope::All => "ALL".into(),
            ViewScope::Stack { anchor } => format!("STACK: {anchor}"),
            ViewScope::Trunk { anchor } => self
                .topology
                .as_ref()
                .and_then(|topology| topology.trunk_for(anchor))
                .map(|trunk| format!("TRUNK: {trunk}"))
                .unwrap_or_else(|| format!("TRUNK: {anchor}")),
            ViewScope::Untrunked => "UNTRUNKED".into(),
        }
    }

    fn enter_scope(&mut self, scope: ViewScope) {
        if matches!(self.scope, ViewScope::All) {
            self.all_view_state = Some(AllViewState {
                selected: self.selected.clone(),
                scroll: self.scroll,
            });
        }
        self.scope = scope;
        self.focus_needs_bottom_alignment = true;
        self.reproject();
        self.align_focused_bottom();
    }

    fn exit_focus(&mut self) {
        self.scope = ViewScope::All;
        self.restore_all_after_reproject = true;
        self.focus_needs_bottom_alignment = false;
        self.reproject();
    }

    fn scope_matches(&self, desired: &ViewScope) -> bool {
        let Some(topology) = self.topology.as_ref() else {
            return false;
        };
        match (&self.scope, desired) {
            (ViewScope::All, ViewScope::All) | (ViewScope::Untrunked, ViewScope::Untrunked) => true,
            (ViewScope::Stack { anchor: current }, ViewScope::Stack { anchor: desired }) => {
                topology.stack_for(current) == topology.stack_for(desired)
            }
            (ViewScope::Trunk { anchor: current }, ViewScope::Trunk { anchor: desired }) => {
                let resolve = |anchor: &BranchId| {
                    topology
                        .trunk_for(anchor)
                        .cloned()
                        .or_else(|| topology.is_trunk(anchor).then(|| anchor.clone()))
                };
                resolve(current) == resolve(desired)
            }
            _ => false,
        }
    }

    fn restrict_projection_to_untrunked(&mut self) {
        let Some(range) = self
            .projection
            .section_ranges
            .iter()
            .find(|range| range.section.is_none())
            .cloned()
        else {
            self.projection.selectable.clear();
            self.projection.selectable_visual_rows.clear();
            self.projection.navigation.clear();
            self.projection.navigation_visual_rows.clear();
            self.projection.branch_to_selectable.clear();
            self.projection.branch_to_visual.clear();
            self.projection.stack_heads.clear();
            self.projection.stack_starts.clear();
            self.projection.rows.clear();
            self.projection.section_ranges.clear();
            for emphasis in self.projection.emphasis_by_branch.values_mut() {
                *emphasis = Emphasis::Hidden;
            }
            return;
        };
        let mut selectable = Vec::new();
        let mut selectable_rows = Vec::new();
        for (row, branch) in self
            .projection
            .selectable_visual_rows
            .iter()
            .copied()
            .zip(self.projection.selectable.iter().cloned())
        {
            if row >= range.start && row <= range.end {
                selectable_rows.push(row);
                selectable.push(branch);
            }
        }
        self.projection.selectable = selectable;
        self.projection.selectable_visual_rows = selectable_rows;
        let mut navigation = Vec::new();
        let mut navigation_rows = Vec::new();
        for (target, row) in self
            .projection
            .navigation
            .iter()
            .cloned()
            .zip(self.projection.navigation_visual_rows.iter().copied())
        {
            if row >= range.start && row <= range.end {
                navigation.push(target);
                navigation_rows.push(row);
            }
        }
        self.projection.navigation = navigation;
        self.projection.navigation_visual_rows = navigation_rows;
        self.projection.branch_to_selectable = self
            .projection
            .selectable
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, branch)| (branch, index))
            .collect();
        self.projection
            .branch_to_visual
            .retain(|_, row| *row >= range.start && *row <= range.end);
        self.projection
            .stack_heads
            .retain(|head| head.visual_row >= range.start && head.visual_row <= range.end);
        self.projection.stack_starts = self
            .projection
            .stack_heads
            .iter()
            .filter_map(|head| {
                self.projection
                    .branch_to_selectable
                    .get(&head.branch)
                    .copied()
            })
            .collect();
        self.projection
            .rows
            .retain(|row| self.projection.branch_to_visual.contains_key(&row.branch));
        for (branch, emphasis) in &mut self.projection.emphasis_by_branch {
            *emphasis = if self.projection.branch_to_visual.contains_key(branch) {
                Emphasis::Full
            } else {
                Emphasis::Hidden
            };
        }
        self.projection.section_ranges = vec![range];
    }

    fn apply_current_startup(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if !matches!(snapshot.state, crate::model::RepositoryState::Ready) {
            self.message = Some(Arc::from(
                "--current unavailable for detached, unborn, or incomplete repository state",
            ));
            return;
        }
        let Some(current) = snapshot
            .branches
            .iter()
            .find(|branch| branch.current)
            .map(|branch| branch.id.clone())
        else {
            self.message = Some(Arc::from(
                "--current found no current local branch; showing All",
            ));
            return;
        };
        let Some(topology) = self.topology.as_ref() else {
            self.message = Some(Arc::from("--current topology unavailable; showing All"));
            return;
        };
        let scope = if topology.is_trunk(&current) {
            ViewScope::Trunk { anchor: current }
        } else if topology.stack_for(&current).is_some() {
            ViewScope::Stack { anchor: current }
        } else {
            self.message = Some(Arc::from(
                "--current branch is missing from the stack map; showing All",
            ));
            return;
        };
        self.enter_scope(scope);
    }

    fn reconcile_selection(&mut self) {
        if let Some(label) = &self.selected_label {
            let visible = self
                .projection
                .navigation
                .iter()
                .any(|target| match (target, label) {
                    (
                        crate::model::topology::SelectionTarget::StackLabel(left),
                        ConfigTarget::Stack(right),
                    ) => left == right,
                    (
                        crate::model::topology::SelectionTarget::VisualSectionLabel(left),
                        ConfigTarget::VisualSection(right),
                    ) => left == right,
                    _ => false,
                });
            if !visible {
                self.selected = Some(label.branch().clone());
                self.selected_label = None;
            }
        }
        if self.selected.is_none() {
            self.selected = self.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .branches
                    .iter()
                    .find(|branch| {
                        branch.current && self.projection.branch_to_visual.contains_key(&branch.id)
                    })
                    .map(|branch| branch.id.clone())
            });
        }
        let selected_visible = self
            .selected
            .as_ref()
            .is_some_and(|selected| self.projection.branch_to_visual.contains_key(selected));
        if !selected_visible {
            self.selected = self.projection.selectable.first().cloned();
        }
        self.keep_selected_visible();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.projection.navigation.is_empty() {
            return;
        }
        let current = self
            .projection
            .navigation
            .iter()
            .position(|target| match target {
                crate::model::topology::SelectionTarget::Branch(branch) => {
                    self.selected_label.is_none() && self.selected.as_ref() == Some(branch)
                }
                crate::model::topology::SelectionTarget::StackLabel(stack) => {
                    self.selected_label.as_ref() == Some(&ConfigTarget::Stack(stack.clone()))
                }
                crate::model::topology::SelectionTarget::VisualSectionLabel(anchor) => {
                    self.selected_label.as_ref()
                        == Some(&ConfigTarget::VisualSection(anchor.clone()))
                }
            })
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(self.projection.navigation.len() - 1);
        match self.projection.navigation[next].clone() {
            crate::model::topology::SelectionTarget::Branch(branch) => {
                self.selected = Some(branch);
                self.selected_label = None;
            }
            crate::model::topology::SelectionTarget::StackLabel(stack) => {
                self.selected = Some(stack.clone());
                self.selected_label = Some(ConfigTarget::Stack(stack));
            }
            crate::model::topology::SelectionTarget::VisualSectionLabel(anchor) => {
                self.selected = Some(anchor.clone());
                self.selected_label = Some(ConfigTarget::VisualSection(anchor));
            }
        }
        self.keep_selected_visible();
    }

    fn move_stack(&mut self, delta: isize) {
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        if !self.projection.is_true_stack(selected) {
            self.move_selection(delta.saturating_mul(10));
            return;
        }
        let Some(current_stack) = self
            .projection
            .row_for(selected)
            .map(|row| row.stack_id.clone())
        else {
            return;
        };
        let current_row = self
            .projection
            .branch_to_visual
            .get(selected)
            .copied()
            .unwrap_or(0);
        let target = if delta < 0 {
            self.projection
                .stack_heads
                .iter()
                .rev()
                .find(|head| head.visual_row < current_row && head.stack_id != current_stack)
                .map(|head| head.branch.clone())
        } else {
            let section = self
                .projection
                .section_ranges
                .iter()
                .find(|range| current_row >= range.start && current_row <= range.end);
            let next_stack = self.projection.stack_heads.iter().find(|head| {
                head.visual_row > current_row
                    && head.stack_id != current_stack
                    && section.is_none_or(|range| head.visual_row <= range.end)
                    && section
                        .and_then(|range| range.trunk_row)
                        .is_none_or(|trunk_row| head.visual_row < trunk_row)
            });
            next_stack.map(|head| head.branch.clone()).or_else(|| {
                section
                    .and_then(|range| range.trunk_row)
                    .filter(|trunk_row| *trunk_row > current_row)
                    .and_then(|trunk_row| self.projection.entries.get(trunk_row))
                    .and_then(|entry| match entry {
                        crate::model::topology::ProjectionEntry::Branch(row) => {
                            Some(row.branch.clone())
                        }
                        _ => None,
                    })
            })
        };
        let Some(target) = target else {
            return;
        };
        self.selected = Some(target);
        self.selected_label = None;
        if self.selected_visual_row() == self.sticky_visual_row() {
            self.align_focused_bottom();
        } else {
            self.keep_selected_visible();
        }
    }

    fn move_section(&mut self, delta: isize) {
        if self.projection.section_ranges.is_empty() {
            return;
        }
        let current_row = self
            .selected
            .as_ref()
            .and_then(|selected| self.projection.branch_to_visual.get(selected).copied())
            .unwrap_or(0);
        let Some(range) = self
            .projection
            .section_ranges
            .iter()
            .find(|range| current_row >= range.start && current_row <= range.end)
        else {
            return;
        };
        let target = if delta < 0 {
            self.projection
                .selectable_visual_rows
                .iter()
                .zip(&self.projection.selectable)
                .find(|(row, _)| **row >= range.start && **row <= range.end)
        } else {
            self.projection
                .selectable_visual_rows
                .iter()
                .zip(&self.projection.selectable)
                .rev()
                .find(|(row, _)| **row >= range.start && **row <= range.end)
        };
        let Some((_, branch)) = target else {
            return;
        };
        self.selected = Some(branch.clone());
        self.selected_label = None;
        self.keep_selected_visible();
    }

    fn keep_selected_visible(&mut self) {
        self.keep_selected_visible_inner();
    }

    fn keep_selected_visible_inner(&mut self) {
        let Some(index) = self.selected_visual_row() else {
            self.scroll = 0;
            return;
        };
        if let (Some((start, _)), Some(sticky)) =
            (self.focused_section_bounds(), self.sticky_visual_row())
        {
            let max_scroll = sticky.saturating_sub(self.viewport_height).max(start);
            self.scroll = self.scroll.clamp(start, max_scroll);
            if index != sticky {
                if index < self.scroll {
                    self.scroll = index.max(start);
                }
                if index >= self.scroll + self.viewport_height {
                    self.scroll = (index + 1 - self.viewport_height).min(max_scroll);
                }
            }
            return;
        }
        if index < self.scroll {
            self.scroll = index;
        }
        if index >= self.scroll + self.viewport_height {
            self.scroll = index + 1 - self.viewport_height;
        }
        self.scroll = self.scroll.min(
            self.projection
                .entries
                .len()
                .saturating_sub(self.viewport_height),
        );
    }

    fn selected_visual_row(&self) -> Option<usize> {
        if let Some(label) = &self.selected_label {
            return self
                .projection
                .navigation
                .iter()
                .zip(&self.projection.navigation_visual_rows)
                .find_map(|(target, row)| match (target, label) {
                    (
                        crate::model::topology::SelectionTarget::StackLabel(left),
                        ConfigTarget::Stack(right),
                    ) if left == right => Some(*row),
                    (
                        crate::model::topology::SelectionTarget::VisualSectionLabel(left),
                        ConfigTarget::VisualSection(right),
                    ) if left == right => Some(*row),
                    _ => None,
                });
        }
        self.selected
            .as_ref()
            .and_then(|selected| self.projection.branch_to_visual.get(selected).copied())
    }

    pub fn selected_visual_row_for_ui(&self) -> Option<usize> {
        self.selected_visual_row()
    }

    fn align_focused_bottom(&mut self) {
        let (Some((start, _)), Some(sticky)) =
            (self.focused_section_bounds(), self.sticky_visual_row())
        else {
            self.keep_selected_visible();
            return;
        };
        self.scroll = sticky.saturating_sub(self.viewport_height).max(start);
        self.keep_selected_visible();
    }
}

impl ReconciliationOperation {
    fn target(&self) -> &BranchId {
        match self {
            Self::Checkout { target, .. } => target,
            Self::Deletion { request, .. } => &request.branch,
            Self::Restack { request, .. } => &request.source.branch,
            Self::Move { request, .. } => &request.source.branch,
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Checkout { .. } => "checkout",
            Self::Deletion { .. } => "deletion",
            Self::Restack { .. } => "restack",
            Self::Move { .. } => "move",
        }
    }
}

fn move_preview_is_valid(snapshot: &RepositorySnapshot, preview: &MovePreview) -> bool {
    let matches_expectation = |expected: &GraphiteBranchExpectation| {
        snapshot
            .branch(&expected.branch)
            .is_some_and(|branch| branch.oid == expected.oid)
    };
    matches_expectation(&preview.source_expectation)
        && matches_expectation(&preview.target_expectation)
        && preview
            .affected_expectations
            .iter()
            .all(matches_expectation)
        && snapshot
            .branch(&preview.source)
            .and_then(|branch| branch.diff_parent.clone())
            == preview.original_parent
        && preview.topology.iter().all(|edge| {
            snapshot
                .branch(&edge.branch)
                .is_some_and(|branch| branch.diff_parent == edge.parent)
        })
}

fn reconciliation_notice(
    snapshot: &RepositorySnapshot,
    operation: &ReconciliationOperation,
) -> Arc<str> {
    match operation {
        ReconciliationOperation::Checkout {
            target,
            command_error,
        } => {
            if snapshot.branch(target).is_some_and(|branch| branch.current) {
                return Arc::from(format!("checked out {target}"));
            }
            let repository_position = snapshot
                .branches
                .iter()
                .find(|branch| branch.current)
                .map(|branch| branch.id.to_string())
                .unwrap_or_else(|| match &snapshot.state {
                    crate::model::RepositoryState::Detached => "detached HEAD".into(),
                    crate::model::RepositoryState::Unborn => "an unborn branch".into(),
                    crate::model::RepositoryState::OperationInProgress(operation) => {
                        format!("operation in progress ({operation})")
                    }
                    crate::model::RepositoryState::Ready => "no current branch".into(),
                });
            if let Some(error) = command_error {
                Arc::from(format!(
                    "checkout of {target} failed ({error}); repository is on {repository_position}; press R to refresh"
                ))
            } else {
                Arc::from(format!(
                    "checkout of {target} was not confirmed; repository is on {repository_position}; press R to refresh"
                ))
            }
        }
        ReconciliationOperation::Deletion {
            request, result, ..
        } => {
            let current = snapshot.branch(&request.branch);
            if current.is_some_and(|branch| branch.oid != request.expected_oid) {
                return Arc::from(format!(
                    "{} changed since deletion began; inspect it before retrying",
                    request.branch
                ));
            }
            match (result, current) {
                (DeletionResult::Outcome(DeleteOutcome::Deleted), None) => {
                    Arc::from(format!("deleted {} locally", request.branch))
                }
                (DeletionResult::Outcome(DeleteOutcome::Deleted), Some(_)) => Arc::from(format!(
                    "deletion of {} was not confirmed; branch still exists; press R to refresh",
                    request.branch
                )),
                (DeletionResult::Outcome(DeleteOutcome::Unchanged), Some(_)) => {
                    Arc::from(format!("{} was not deleted", request.branch))
                }
                (DeletionResult::Outcome(DeleteOutcome::Unchanged), None) => Arc::from(format!(
                    "{} disappeared although deletion reported no change; inspect before retrying",
                    request.branch
                )),
                (DeletionResult::Outcome(DeleteOutcome::Inconsistent), _) => Arc::from(
                    "deletion result is inconsistent; inspect Git/Graphite before retrying",
                ),
                (DeletionResult::Error(error), Some(_)) => Arc::from(format!(
                    "deletion of {} failed ({error}); press R to refresh",
                    request.branch
                )),
                (DeletionResult::Error(error), None) => Arc::from(format!(
                    "{} disappeared after deletion failed ({error}); inspect before retrying",
                    request.branch
                )),
            }
        }
        ReconciliationOperation::Restack { request, result } => match result {
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Applied) => {
                Arc::from(format!("restacked {}", request.source.branch))
            }
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Unchanged) => {
                Arc::from(format!("{} was not restacked", request.source.branch))
            }
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Inconsistent) => {
                Arc::from("restack result is inconsistent; inspect Git/Graphite before retrying")
            }
            GraphiteActionResult::Error(error) => Arc::from(format!(
                "restack of {} failed ({error}); press R to refresh",
                request.source.branch
            )),
        },
        ReconciliationOperation::Move { request, result } => match result {
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Applied) => Arc::from(format!(
                "moved {} onto {}",
                request.source.branch, request.target.branch
            )),
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Unchanged) => {
                Arc::from(format!("{} was not moved", request.source.branch))
            }
            GraphiteActionResult::Outcome(GraphiteMutationOutcome::Inconsistent) => {
                Arc::from("move result is inconsistent; inspect Git/Graphite before retrying")
            }
            GraphiteActionResult::Error(error) => Arc::from(format!(
                "move of {} failed ({error}); press R to refresh",
                request.source.branch
            )),
        },
    }
}
