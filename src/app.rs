use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::git::{DeleteOutcome, DeleteRequest};
use crate::adapters::github::{GitHubError, PrMatch};
use crate::config::Config;
use crate::events::{Input, Key, KeyPhase};
use crate::model::topology::{
    Emphasis, OrderMode, ProjectionOptions, ProjectionScope, TopologyIndex, TopologyProjection,
};
use crate::model::{Branch, BranchId, RepositorySnapshot};

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
const ORDER_OPTIONS: &[OrderMode] = &[
    OrderMode::Recent,
    OrderMode::Alphabetical,
    OrderMode::Graphite,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    Checkout(BranchId),
    Delete(DeleteRequest),
    OpenUrl(Arc<str>),
    CopyUrl(Arc<str>),
    PersistColor(ColorWriteRequest),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorWriteRequest {
    pub sequence: u64,
    pub common_dir: PathBuf,
    pub updates: BTreeMap<BranchId, Option<Arc<str>>>,
    pub fallback: Config,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum GitHubState {
    #[default]
    Idle,
    Loading,
    Ready,
    Unavailable(GitHubError),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ViewScope {
    #[default]
    All,
    Stack {
        anchor: BranchId,
    },
    Trunk {
        anchor: BranchId,
    },
    Untrunked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AllViewState {
    selected: Option<BranchId>,
    scroll: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteConfirmation {
    pub repository_id: Arc<str>,
    pub request: DeleteRequest,
    pub pr_number: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum MutationState {
    #[default]
    Idle,
    CheckingOut(BranchId),
    ConfirmingDeletion(DeleteConfirmation),
    Deleting(DeleteConfirmation),
    Reconciling {
        request_epoch: u64,
    },
    DeletionBlocked(Arc<str>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LanePitch {
    #[default]
    Auto,
    Fixed(u16),
}

impl LanePitch {
    fn adjust(self, delta: i16) -> Self {
        let current = match self {
            Self::Auto => AUTO_LANE_PITCH,
            Self::Fixed(value) => value,
        };
        Self::Fixed(
            current
                .saturating_add_signed(delta)
                .clamp(MIN_LANE_PITCH, MAX_LANE_PITCH),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderPicker {
    pub original: OrderMode,
    pub pending: OrderMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorPicker {
    pub target: BranchId,
    pub original: Option<Arc<str>>,
    pub pending: Option<Arc<str>>,
    pub choice_index: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    Search,
    Help,
    OrderPicker(OrderPicker),
    ColorPicker(ColorPicker),
}

pub struct App {
    pub snapshot: Option<Arc<RepositorySnapshot>>,
    pub projection: TopologyProjection,
    topology: Option<TopologyIndex>,
    pub selected: Option<BranchId>,
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
    restore_selection: Option<BranchId>,
    all_view_state: Option<AllViewState>,
    restore_all_after_reproject: bool,
    focus_needs_bottom_alignment: bool,
    startup_current_pending: bool,
    next_color_sequence: u64,
    latest_color_sequence: u64,
    pending_colors: HashMap<BranchId, (u64, Option<Arc<str>>)>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            snapshot: None,
            projection: TopologyProjection::default(),
            topology: None,
            selected: None,
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
            restore_selection: None,
            all_view_state: None,
            restore_all_after_reproject: false,
            focus_needs_bottom_alignment: false,
            startup_current_pending: false,
            next_color_sequence: 0,
            latest_color_sequence: 0,
            pending_colors: HashMap::new(),
        }
    }
}

impl App {
    pub fn apply_snapshot(&mut self, snapshot: Arc<RepositorySnapshot>) {
        self.apply_snapshot_kind(snapshot, true, None);
    }

    pub fn apply_structural_snapshot(
        &mut self,
        snapshot: Arc<RepositorySnapshot>,
        request_epoch: u64,
    ) {
        self.apply_snapshot_kind(snapshot, true, Some(request_epoch));
    }

    pub fn apply_enriched_snapshot(&mut self, snapshot: Arc<RepositorySnapshot>) {
        self.apply_snapshot_kind(snapshot, false, None);
    }

    fn apply_snapshot_kind(
        &mut self,
        mut snapshot: Arc<RepositorySnapshot>,
        structural: bool,
        structural_epoch: Option<u64>,
    ) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| snapshot.generation < current.generation)
        {
            return;
        }
        if let Some(current) = &self.snapshot {
            let current_prs: std::collections::HashMap<_, _> = current
                .branches
                .iter()
                .filter_map(|branch| {
                    branch
                        .pr
                        .clone()
                        .map(|pr| ((branch.id.clone(), branch.oid.clone()), pr))
                })
                .collect();
            if !current_prs.is_empty() {
                let snapshot = Arc::make_mut(&mut snapshot);
                let branches = Arc::make_mut(&mut snapshot.branches);
                for branch in branches {
                    if branch.pr.is_none() {
                        branch.pr = current_prs
                            .get(&(branch.id.clone(), branch.oid.clone()))
                            .cloned();
                    }
                }
            }
        }
        if structural {
            match Config::load(&snapshot.common_dir) {
                Ok(mut config) => {
                    for (root, (_, color)) in &self.pending_colors {
                        config
                            .set_color_in_memory(root, color.as_deref())
                            .expect("pending colors were already validated");
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
            match &self.mutation {
                MutationState::ConfirmingDeletion(confirmation)
                    if !confirmation_is_valid(&snapshot, confirmation) =>
                {
                    self.mutation = MutationState::Idle;
                    self.message = Some(Arc::from(
                        "repository changed; deletion confirmation cancelled",
                    ));
                }
                MutationState::Reconciling { request_epoch }
                    if structural_epoch.is_some_and(|epoch| epoch >= *request_epoch) =>
                {
                    self.mutation = MutationState::Idle;
                }
                _ => {}
            }
        }
        self.snapshot = Some(snapshot);
        self.loading = false;
        if structural {
            self.refresh_error = None;
        }
        if projection_changed {
            self.reproject();
        }
        self.reconcile_selection();
        self.revalidate_overlay();
        if structural && self.startup_current_pending {
            self.startup_current_pending = false;
            self.apply_current_startup();
        }
    }

    pub fn mark_stale(&mut self, error: Arc<str>) {
        self.loading = false;
        self.refresh_error = Some(error);
    }

    pub fn apply_prs(&mut self, matches: Vec<PrMatch>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let mut branches = snapshot.branches.to_vec();
        for branch in &mut branches {
            branch.pr = None;
        }
        for result in matches {
            if let Some(branch) = branches
                .iter_mut()
                .find(|branch| branch.id == result.branch && branch.oid == result.oid)
            {
                branch.pr = Some(result.pull_request);
            }
        }
        self.snapshot = Some(Arc::new(RepositorySnapshot {
            branches: Arc::from(branches),
            ..(**snapshot).clone()
        }));
        self.github_state = GitHubState::Ready;
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
        let selected = self.selected.as_ref()?;
        self.snapshot.as_ref()?.branch(selected)
    }

    pub fn handle_input(&mut self, input: Input) -> Action {
        if input.phase == KeyPhase::Repeat && !input.key.allows_repeat() {
            return Action::None;
        }
        if input.phase == KeyPhase::Repeat
            && matches!(
                &self.overlay,
                Overlay::OrderPicker(_) | Overlay::ColorPicker(_)
            )
        {
            return Action::None;
        }
        let key = input.key;
        if key == Key::Quit {
            return Action::Quit;
        }
        if matches!(self.mutation, MutationState::ConfirmingDeletion(_)) {
            return self.handle_delete_confirmation(key);
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
            Key::Character('r') => Action::Refresh,
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
                self.separators = !self.separators;
                self.reproject();
                Action::None
            }
            Key::Character('x') => {
                self.begin_delete_confirmation();
                Action::None
            }
            Key::Character('?') => {
                self.overlay = Overlay::Help;
                Action::None
            }
            Key::Character('c') => self.cycle_color(),
            Key::Character('C') => {
                self.open_color_picker();
                Action::None
            }
            Key::Character('o') => self
                .selected_url()
                .map(Action::OpenUrl)
                .unwrap_or(Action::None),
            Key::Character('y') => self
                .selected_url()
                .map(Action::CopyUrl)
                .unwrap_or(Action::None),
            Key::Enter if matches!(self.mutation, MutationState::Idle) => {
                if let Some(reason) = self.checkout_disabled_reason() {
                    self.message = Some(Arc::from(reason));
                    Action::None
                } else if let Some(branch) = self.selected.clone() {
                    self.mutation = MutationState::CheckingOut(branch.clone());
                    Action::Checkout(branch)
                } else {
                    Action::None
                }
            }
            Key::Escape => {
                self.message = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn cycle_color(&mut self) -> Action {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Action::None;
        };
        let Some(selected) = self.selected.as_ref() else {
            return Action::None;
        };
        let Some(row) = self.projection.row_for(selected) else {
            return Action::None;
        };
        if row.is_trunk {
            self.message = Some(Arc::from("trunk rows do not have a stack color"));
            return Action::None;
        }
        let root = row.stack_id.clone();
        let common_dir = snapshot.common_dir.clone();
        let next = match self.config.color(&root) {
            None => COLOR_OPTIONS[1].1,
            Some(current) => COLOR_OPTIONS
                .iter()
                .position(|(_, value)| *value == Some(current))
                .and_then(|index| COLOR_OPTIONS.get(index + 1))
                .and_then(|(_, value)| *value),
        };
        self.config
            .set_color_in_memory(&root, next)
            .expect("palette colors are valid");
        self.persist_color(root, next.map(Arc::from), common_dir)
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
        self.next_color_sequence = self.next_color_sequence.wrapping_add(1).max(1);
        self.latest_color_sequence = self.next_color_sequence;
        self.pending_colors
            .insert(root.clone(), (self.next_color_sequence, value));
        Action::PersistColor(ColorWriteRequest {
            sequence: self.next_color_sequence,
            common_dir,
            updates: self
                .pending_colors
                .iter()
                .map(|(root, (_, color))| (root.clone(), color.clone()))
                .collect(),
            fallback: self.config.clone(),
        })
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
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(row) = self.projection.row_for(selected) else {
            return;
        };
        if row.is_trunk {
            self.message = Some(Arc::from("trunk rows do not have a stack color"));
            return;
        }
        let target = row.stack_id.clone();
        let original = self.config.color(&target).map(Arc::from);
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

    fn handle_color_picker_key(&mut self, key: Key) -> Action {
        let Overlay::ColorPicker(mut picker) = self.overlay.clone() else {
            return Action::None;
        };
        match key {
            Key::Up | Key::Down => {
                picker.choice_index = if key == Key::Up {
                    picker.choice_index.saturating_sub(1)
                } else {
                    (picker.choice_index + 1).min(COLOR_OPTIONS.len() - 1)
                };
                picker.pending = COLOR_OPTIONS[picker.choice_index].1.map(Arc::from);
                self.config
                    .set_color_in_memory(&picker.target, picker.pending.as_deref())
                    .expect("picker colors are valid");
                self.overlay = Overlay::ColorPicker(picker);
                Action::None
            }
            Key::Enter => {
                if !self.color_picker_target_is_valid(&picker.target) {
                    self.close_invalid_color_picker(&picker);
                    return Action::None;
                }
                let common_dir = self
                    .snapshot
                    .as_ref()
                    .expect("a valid picker target has a snapshot")
                    .common_dir
                    .clone();
                self.overlay = Overlay::None;
                self.persist_color(picker.target, picker.pending, common_dir)
            }
            Key::Escape => {
                self.config
                    .set_color_in_memory(&picker.target, picker.original.as_deref())
                    .expect("original config color was valid");
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn revalidate_overlay(&mut self) {
        let Overlay::ColorPicker(picker) = self.overlay.clone() else {
            return;
        };
        if self.color_picker_target_is_valid(&picker.target) {
            self.config
                .set_color_in_memory(&picker.target, picker.pending.as_deref())
                .expect("picker colors are valid");
        } else {
            self.close_invalid_color_picker(&picker);
        }
    }

    fn color_picker_target_is_valid(&self, target: &BranchId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.branch(target).is_some())
            && self.topology.as_ref().is_some_and(|topology| {
                !topology.is_trunk(target) && topology.stack_for(target) == Some(target)
            })
    }

    fn close_invalid_color_picker(&mut self, picker: &ColorPicker) {
        self.config
            .set_color_in_memory(&picker.target, picker.original.as_deref())
            .expect("original config color was valid");
        self.overlay = Overlay::None;
        self.message = Some(Arc::from(
            "color picker closed because its stack is no longer available",
        ));
    }

    pub fn finish_color_persistence(&mut self, sequence: u64, result: anyhow::Result<()>) {
        if result.is_ok() {
            self.pending_colors
                .retain(|_, (pending_sequence, _)| *pending_sequence > sequence);
        }
        if sequence != self.latest_color_sequence {
            return;
        }
        self.message = Some(Arc::from(match result {
            Ok(()) => "stack color saved".to_owned(),
            Err(error) => format!("color save failed; in-memory choice retained: {error}"),
        }));
    }

    pub fn prepare_pending_color_persistence(
        &self,
        mut request: ColorWriteRequest,
    ) -> Option<ColorWriteRequest> {
        request.updates = self
            .pending_colors
            .iter()
            .map(|(root, (_, color))| (root.clone(), color.clone()))
            .collect();
        if request.updates.is_empty() {
            return None;
        }
        request.fallback = self.config.clone();
        Some(request)
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

    fn selected_url(&self) -> Option<Arc<str>> {
        self.selected_branch()?.pr.as_ref().map(|pr| pr.url.clone())
    }

    fn begin_delete_confirmation(&mut self) {
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
        if let Some(refusal) = deletion_refusal(snapshot, branch) {
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
        });
    }

    fn handle_delete_confirmation(&mut self, key: Key) -> Action {
        match key {
            Key::Character('y') => {
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
        self.message = Some(Arc::from(match result {
            Ok(()) => "checkout complete; reconciling".to_owned(),
            Err(error) => format!("checkout blocked: {error}"),
        }));
        self.mutation = MutationState::Reconciling { request_epoch };
    }

    pub fn finish_deletion(&mut self, result: anyhow::Result<DeleteOutcome>, request_epoch: u64) {
        match result {
            Ok(DeleteOutcome::Deleted) => {
                self.message = Some(Arc::from("branch deleted locally; reconciling"));
                self.mutation = MutationState::Reconciling { request_epoch };
            }
            Ok(DeleteOutcome::Unchanged) => {
                self.message = Some(Arc::from("branch was not deleted; reconciling"));
                self.mutation = MutationState::Reconciling { request_epoch };
            }
            Ok(DeleteOutcome::Inconsistent) => {
                let message: Arc<str> = Arc::from(
                    "deletion result is inconsistent; inspect Git/Graphite before retrying",
                );
                self.message = Some(message.clone());
                self.mutation = MutationState::DeletionBlocked(message);
            }
            Err(error) => {
                self.message = Some(Arc::from(format!("deletion blocked: {error}")));
                self.mutation = MutationState::Reconciling { request_epoch };
            }
        }
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
        if branch
            .worktree
            .as_ref()
            .is_some_and(|path| path != &snapshot.root)
        {
            return Some(format!("{} is checked out in another worktree", branch.id));
        }
        None
    }

    fn reproject(&mut self) {
        if self.snapshot.is_some() && self.topology.is_some() {
            self.validate_scope();
            let scope = self.resolved_scope();
            self.projection =
                self.topology
                    .as_ref()
                    .expect("topology exists")
                    .project(&ProjectionOptions {
                        order: self.order_mode,
                        scope,
                        separators: self.separators,
                        filter: self.filter.clone(),
                        ..ProjectionOptions::default()
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
        if self.projection.selectable.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|selected| self.projection.branch_to_selectable.get(selected).copied())
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(self.projection.selectable.len() - 1);
        self.selected = Some(self.projection.selectable[next].clone());
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
        } else {
            self.projection
                .stack_heads
                .iter()
                .find(|head| head.visual_row > current_row && head.stack_id != current_stack)
        };
        let Some(target) = target else {
            return;
        };
        self.selected = Some(target.branch.clone());
        self.keep_selected_visible();
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
        self.keep_selected_visible();
    }

    fn keep_selected_visible(&mut self) {
        let Some(index) = self
            .selected
            .as_ref()
            .and_then(|selected| self.projection.branch_to_visual.get(selected).copied())
        else {
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

fn order_option_index(mode: OrderMode) -> usize {
    ORDER_OPTIONS
        .iter()
        .position(|option| *option == mode)
        .unwrap_or(0)
}

fn confirmation_is_valid(snapshot: &RepositorySnapshot, confirmation: &DeleteConfirmation) -> bool {
    snapshot.repository_id == confirmation.repository_id
        && snapshot
            .branch(&confirmation.request.branch)
            .is_some_and(|branch| {
                branch.oid == confirmation.request.expected_oid
                    && branch.graphite == confirmation.request.expected_provenance
                    && deletion_refusal(snapshot, branch).is_none()
            })
}

fn deletion_refusal(snapshot: &RepositorySnapshot, branch: &Branch) -> Option<String> {
    if branch.graphite == crate::model::GraphiteProvenance::Tracked && branch.id.0.starts_with('-')
    {
        Some(format!(
            "cannot safely delete option-shaped branch {}",
            branch.id
        ))
    } else if branch.current {
        Some(format!("cannot delete current branch {}", branch.id))
    } else if snapshot.configured_trunks.contains(&branch.id) {
        Some(format!("cannot delete configured trunk {}", branch.id))
    } else if branch.worktree.is_some() {
        Some(format!("{} is checked out in a worktree", branch.id))
    } else if !matches!(snapshot.state, crate::model::RepositoryState::Ready) {
        Some(format!(
            "deletion disabled while repository is {:?}",
            snapshot.state
        ))
    } else if branch.graphite == crate::model::GraphiteProvenance::Degraded {
        Some("Graphite tracking is degraded; deletion is disabled".into())
    } else if branch.graphite == crate::model::GraphiteProvenance::Tracked
        && snapshot
            .branches
            .iter()
            .any(|candidate| candidate.diff_parent.as_ref() == Some(&branch.id))
    {
        Some(format!("tracked branch {} has children", branch.id))
    } else {
        None
    }
}
