use super::DeleteConfirmation;
use crate::model::{Branch, GraphiteProvenance, RepositorySnapshot, RepositoryState};

pub(super) fn confirmation_is_valid(
    snapshot: &RepositorySnapshot,
    confirmation: &DeleteConfirmation,
) -> bool {
    snapshot.repository_id == confirmation.repository_id
        && snapshot
            .branch(&confirmation.request.branch)
            .is_some_and(|branch| {
                branch.oid == confirmation.request.expected_oid
                    && branch.graphite == confirmation.request.expected_provenance
                    && deletion_refusal(snapshot, branch, None).is_none()
            })
}

pub(super) fn deletion_refusal(
    snapshot: &RepositorySnapshot,
    branch: &Branch,
    stale_error: Option<&str>,
) -> Option<String> {
    if let Some(error) = stale_error {
        Some(stale_deletion_refusal(error))
    } else if branch.graphite == GraphiteProvenance::Tracked && branch.id.0.starts_with('-') {
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
    } else if !matches!(snapshot.state, RepositoryState::Ready) {
        Some(format!(
            "deletion disabled while repository is {:?}",
            snapshot.state
        ))
    } else if branch.graphite == GraphiteProvenance::Degraded {
        Some("Graphite tracking is degraded; deletion is disabled".into())
    } else if branch.graphite == GraphiteProvenance::Tracked
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

pub(super) fn stale_deletion_refusal(error: &str) -> String {
    format!("repository data is stale ({error}); press r before deleting")
}
