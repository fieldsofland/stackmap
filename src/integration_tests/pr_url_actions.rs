use super::common;

use std::sync::Arc;

use crate::adapters::github::PrMatch;
use crate::app::{Action, App, ConfigTarget, Overlay};
use crate::events::Key;
use crate::model::topology::ArchiveMode;
use crate::model::{Branch, BranchId, GraphiteProvenance, PullRequest, PullRequestStatus};

fn pull_request(number: u64) -> PullRequest {
    PullRequest {
        number,
        title: Arc::from(format!("PR {number}")),
        url: Arc::from(format!("https://example.invalid/pr/{number}")),
        status: PullRequestStatus::Open,
    }
}

fn pr_match(branch: &str, oid: &str, number: u64) -> PrMatch {
    PrMatch {
        branch: BranchId::new(branch),
        oid: Arc::from(oid),
        pull_request: pull_request(number),
    }
}

fn branch_with_pr(
    name: &str,
    parent: Option<&str>,
    root: &str,
    current: bool,
    number: u64,
) -> Branch {
    let mut branch = common::branch(name, parent, root, current);
    branch.pr = Some(pull_request(number));
    branch
}

fn tracked(name: &str, parent: Option<&str>, root: &str, trunk: &str, committed_at: i64) -> Branch {
    let mut branch = common::branch(name, parent, root, false);
    branch.trunk = Some(BranchId::new(trunk));
    branch.graphite = GraphiteProvenance::Tracked;
    branch.committed_at = committed_at;
    branch
}

fn linear_stack_snapshot() -> Arc<crate::model::RepositorySnapshot> {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        branch_with_pr("base", None, "base", false, 10),
        branch_with_pr("middle", Some("base"), "base", false, 20),
        branch_with_pr("tip", Some("middle"), "base", true, 30),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("main"), Arc::from([BranchId::new("base")])),
        (BranchId::new("base"), Arc::from([BranchId::new("middle")])),
        (BranchId::new("middle"), Arc::from([BranchId::new("tip")])),
    ]);
    Arc::new(snapshot)
}

fn tracked_with_pr(
    name: &str,
    parent: Option<&str>,
    root: &str,
    trunk: &str,
    committed_at: i64,
    number: u64,
    current: bool,
) -> Branch {
    let mut branch = tracked(name, parent, root, trunk, committed_at);
    branch.current = current;
    branch.pr = Some(pull_request(number));
    branch
}

fn fork_stack_snapshot() -> Arc<crate::model::RepositorySnapshot> {
    let mut snapshot = (*common::snapshot(vec![
        tracked("staging", None, "staging", "staging", 1),
        tracked_with_pr("1", None, "1", "staging", 2, 1, false),
        tracked_with_pr("2", Some("1"), "1", "staging", 3, 2, false),
        tracked("3", Some("2"), "1", "staging", 4),
        tracked("4", Some("3"), "1", "staging", 5),
        tracked("3b", Some("3"), "1", "staging", 6),
        tracked_with_pr("3c", Some("3b"), "1", "staging", 7, 3, true),
    ]))
    .clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("staging")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("staging"), Arc::from([BranchId::new("1")])),
        (BranchId::new("1"), Arc::from([BranchId::new("2")])),
        (BranchId::new("2"), Arc::from([BranchId::new("3")])),
        (
            BranchId::new("3"),
            Arc::from([BranchId::new("4"), BranchId::new("3b")]),
        ),
        (BranchId::new("3b"), Arc::from([BranchId::new("3c")])),
    ]);
    Arc::new(snapshot)
}

#[test]
fn lowercase_o_opens_name_matched_pull_request_with_different_oid() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![pr_match("feature/stack-map", "remote-head-oid", 42)]);
    assert_eq!(
        app.handle_key(Key::Character('o')),
        Action::OpenUrl(Arc::from("https://example.invalid/pr/42"))
    );
}

#[test]
fn uppercase_o_includes_name_matched_pull_requests_in_stack() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![
        common::branch("base", None, "base", false),
        common::branch("middle", Some("base"), "base", false),
        common::branch("tip", Some("middle"), "base", true),
    ]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![
        pr_match("base", "remote-base", 10),
        pr_match("middle", "remote-middle", 20),
        pr_match("tip", "remote-tip", 30),
    ]);
    app.selected = Some(BranchId::new("middle"));
    assert_eq!(
        app.handle_key(Key::Character('O')),
        Action::OpenUrls(vec![
            Arc::from("https://example.invalid/pr/10"),
            Arc::from("https://example.invalid/pr/20"),
            Arc::from("https://example.invalid/pr/30"),
        ])
    );
}

#[test]
fn lowercase_o_opens_selected_pull_request_url() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("middle"));
    assert_eq!(
        app.handle_key(Key::Character('o')),
        Action::OpenUrl(Arc::from("https://example.invalid/pr/20"))
    );
}

#[test]
fn lowercase_o_is_noop_without_pull_request() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
}

#[test]
fn lowercase_o_is_noop_in_archive_view() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("middle"));
    app.handle_key(Key::Character('a'));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
}

#[test]
fn lowercase_o_is_noop_when_label_is_selected() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("base"));
    app.selected_label = Some(ConfigTarget::Stack(BranchId::new("base")));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
}

#[test]
fn search_overlay_still_types_o_into_filter() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.handle_key(Key::Character('/'));
    assert!(matches!(app.overlay, Overlay::Search));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
    assert_eq!(app.filter, "o");
}

#[test]
fn uppercase_o_opens_all_stack_pull_requests_in_group_order() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("middle"));
    assert_eq!(
        app.handle_key(Key::Character('O')),
        Action::OpenUrls(vec![
            Arc::from("https://example.invalid/pr/10"),
            Arc::from("https://example.invalid/pr/20"),
            Arc::from("https://example.invalid/pr/30"),
        ])
    );
}

#[test]
fn uppercase_o_on_side_fork_opens_only_child_stack_pull_requests() {
    let mut app = App::default();
    app.apply_snapshot(fork_stack_snapshot());
    app.selected = Some(BranchId::new("3c"));
    assert_eq!(
        app.handle_key(Key::Character('O')),
        Action::OpenUrls(vec![Arc::from("https://example.invalid/pr/3")])
    );
}

#[test]
fn uppercase_o_skips_branches_without_pull_requests() {
    let mut snapshot = (*linear_stack_snapshot()).clone();
    let branches: Vec<Branch> = snapshot
        .branches
        .iter()
        .map(|branch| {
            if branch.id == BranchId::new("middle") {
                let mut without_pull_request = branch.clone();
                without_pull_request.pr = None;
                without_pull_request
            } else {
                branch.clone()
            }
        })
        .collect();
    snapshot.branches = Arc::from(branches);
    snapshot.branch_index = crate::model::RepositorySnapshot::index_branches(&snapshot.branches);

    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.selected = Some(BranchId::new("middle"));
    assert_eq!(
        app.handle_key(Key::Character('O')),
        Action::OpenUrls(vec![
            Arc::from("https://example.invalid/pr/10"),
            Arc::from("https://example.invalid/pr/30"),
        ])
    );
}

#[test]
fn uppercase_o_deduplicates_duplicate_pull_request_urls() {
    let mut snapshot = (*linear_stack_snapshot()).clone();
    let shared_url: Arc<str> = Arc::from("https://example.invalid/pr/shared");
    let branches: Vec<Branch> = snapshot
        .branches
        .iter()
        .map(|branch| {
            let mut updated = branch.clone();
            if branch.id == BranchId::new("middle") || branch.id == BranchId::new("tip") {
                updated.pr = Some(PullRequest {
                    number: 99,
                    title: Arc::from("Shared"),
                    url: shared_url.clone(),
                    status: PullRequestStatus::Open,
                });
            }
            updated
        })
        .collect();
    snapshot.branches = Arc::from(branches);
    snapshot.branch_index = crate::model::RepositorySnapshot::index_branches(&snapshot.branches);

    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.selected = Some(BranchId::new("middle"));
    assert_eq!(
        app.handle_key(Key::Character('O')),
        Action::OpenUrls(vec![Arc::from("https://example.invalid/pr/10"), shared_url,])
    );
}

#[test]
fn uppercase_o_is_noop_on_trunk() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("main"));
    assert_eq!(app.handle_key(Key::Character('O')), Action::None);
}

#[test]
fn uppercase_o_is_noop_in_archive_view() {
    let mut app = App::default();
    app.apply_snapshot(linear_stack_snapshot());
    app.selected = Some(BranchId::new("middle"));
    app.archive_mode = ArchiveMode::Archive;
    assert_eq!(app.handle_key(Key::Character('O')), Action::None);
}
