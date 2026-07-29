use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::adapters::git::{DeleteOutcome, DeleteRequest};
use crate::adapters::github::GitHubError;
use crate::config::{Config, ConfigMutation};
use crate::model::BranchId;
use crate::model::topology::{ArchiveMode, OrderMode};

use super::{AUTO_LANE_PITCH, MAX_LANE_PITCH, MIN_LANE_PITCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    Checkout(BranchId),
    Delete(DeleteRequest),
    OpenUrl(Arc<str>),
    CopyUrl(Arc<str>),
    PersistConfig(ConfigWriteRequest),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigWriteRequest {
    pub sequence: u64,
    pub common_dir: PathBuf,
    pub mutation: ConfigMutation,
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
pub(super) struct AllViewState {
    pub(super) selected: Option<BranchId>,
    pub(super) scroll: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveRange {
    pub mode: ArchiveMode,
    pub section: Option<BranchId>,
    pub anchor: BranchId,
    pub endpoint: BranchId,
    pub(super) eligible: Arc<[BranchId]>,
    pub(super) eligible_index: Arc<HashMap<BranchId, usize>>,
    pub(super) anchor_index: usize,
    pub(super) endpoint_index: usize,
}

impl ArchiveRange {
    pub fn branches(&self) -> &[BranchId] {
        &self.eligible[self.index_range()]
    }

    pub fn branch_count(&self) -> usize {
        self.anchor_index.abs_diff(self.endpoint_index) + 1
    }

    pub(super) fn contains(&self, branch: &BranchId) -> bool {
        self.eligible_index
            .get(branch)
            .is_some_and(|index| self.index_range().contains(index))
    }

    fn index_range(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor_index.min(self.endpoint_index)..=self.anchor_index.max(self.endpoint_index)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteConfirmation {
    pub repository_id: Arc<str>,
    pub request: DeleteRequest,
    pub pr_number: Option<u64>,
    pub selection_after: Option<BranchId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum MutationState {
    #[default]
    Idle,
    ConfirmingCheckout(BranchId),
    CheckingOut(BranchId),
    ConfirmingDeletion(DeleteConfirmation),
    Deleting(DeleteConfirmation),
    Reconciling {
        operation: ReconciliationOperation,
        request_epoch: u64,
        deadline: Instant,
    },
    DeletionBlocked(Arc<str>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationOperation {
    Checkout {
        target: BranchId,
        command_error: Option<Arc<str>>,
    },
    Deletion {
        request: DeleteRequest,
        result: DeletionResult,
        selection_after: Option<BranchId>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeletionResult {
    Outcome(DeleteOutcome),
    Error(Arc<str>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransientNotice {
    pub(super) text: Arc<str>,
    pub(super) expires_at: Instant,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LanePitch {
    #[default]
    Auto,
    Fixed(u16),
}

impl LanePitch {
    pub(super) fn adjust(self, delta: i16) -> Self {
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
    pub target: ConfigTarget,
    pub original: Option<Arc<str>>,
    pub pending: Option<Arc<str>>,
    pub choice_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackNameEditor {
    pub target: ConfigTarget,
    pub draft: String,
    pub cursor: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigTarget {
    Stack(BranchId),
    VisualSection(BranchId),
}

impl ConfigTarget {
    pub fn branch(&self) -> &BranchId {
        match self {
            Self::Stack(branch) | Self::VisualSection(branch) => branch,
        }
    }
}

impl std::fmt::Display for ConfigTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.branch().fmt(formatter)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    Search,
    Help,
    OrderPicker(OrderPicker),
    ColorPicker(ColorPicker),
    StackNameEditor(StackNameEditor),
    ArchiveRange(ArchiveRange),
}
