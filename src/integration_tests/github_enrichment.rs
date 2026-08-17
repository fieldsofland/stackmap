use super::common;

use std::sync::Arc;

use crate::adapters::github::{GitHubError, PrMatch, parse_json};
use crate::app::App;
use crate::model::{BranchId, PullRequest, PullRequestStatus};

fn pull_request(number: u64, status: PullRequestStatus) -> PullRequest {
    PullRequest {
        number,
        title: Arc::from(format!("PR {number}")),
        url: Arc::from(format!("https://example.invalid/pr/{number}")),
        status,
    }
}

fn pr_match(branch: &str, oid: &str, number: u64, status: PullRequestStatus) -> PrMatch {
    PrMatch {
        branch: BranchId::new(branch),
        oid: Arc::from(oid),
        pull_request: pull_request(number, status),
    }
}

#[test]
fn parses_batched_pr_json_including_missing_oid() {
    let matches = parse_json(include_bytes!("../../tests/fixtures/github/pr-list.json")).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].branch, BranchId::new("feature/stack-map"));
    assert_eq!(matches[0].pull_request.number, 42);
    assert_eq!(matches[0].pull_request.status, PullRequestStatus::Approved);
    assert_eq!(matches[1].branch, BranchId::new("ambiguous"));
    assert_eq!(matches[1].oid.as_ref(), "");
    assert_eq!(matches[1].pull_request.number, 7);
}

#[test]
fn name_and_oid_match_attaches_pull_request() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![pr_match(
        "feature/stack-map",
        "oid-feature/stack-map",
        42,
        PullRequestStatus::Open,
    )]);
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        42
    );
}

#[test]
fn name_match_with_different_oid_still_attaches_pull_request() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![pr_match(
        "feature/stack-map",
        "remote-head-oid",
        42,
        PullRequestStatus::Open,
    )]);
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        42
    );
}

#[test]
fn pull_request_attaches_only_to_matching_branch_name() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![
        common::branch("feature/stack-map", None, "feature/stack-map", true),
        common::branch("other-branch", None, "other-branch", false),
    ]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![pr_match(
        "feature/stack-map",
        "remote-head-oid",
        42,
        PullRequestStatus::Open,
    )]);

    let snapshot = app.snapshot.as_ref().unwrap();
    let feature = snapshot
        .branch(&BranchId::new("feature/stack-map"))
        .unwrap();
    let other = snapshot.branch(&BranchId::new("other-branch")).unwrap();
    assert_eq!(feature.pr.as_ref().unwrap().number, 42);
    assert!(other.pr.is_none());
}

#[test]
fn open_pull_request_wins_over_closed_for_same_branch_name() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![
        pr_match(
            "feature/stack-map",
            "closed-oid",
            10,
            PullRequestStatus::Closed,
        ),
        pr_match("feature/stack-map", "open-oid", 20, PullRequestStatus::Open),
    ]);
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        20
    );
}

#[test]
fn delayed_results_persist_across_snapshot_refresh_by_branch_name() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    app.apply_prs(vec![pr_match(
        "feature/stack-map",
        "oid-feature/stack-map",
        42,
        PullRequestStatus::Open,
    )]);

    let mut next = (*common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]))
    .clone();
    next.generation = 2;
    app.apply_snapshot(Arc::new(next));
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        42
    );
}

#[test]
fn malformed_provider_response_has_a_typed_failure() {
    let error = parse_json(b"not json").unwrap_err();
    assert!(matches!(error, GitHubError::Malformed(_)));
}

#[test]
fn parses_merged_closed_and_open_pull_request_states() {
    let matches = parse_json(
        br#"[
            {"number":1,"title":"Merged","url":"u","headRefName":"merged","headRefOid":"a","state":"MERGED","reviewDecision":"APPROVED"},
            {"number":2,"title":"Closed","url":"u","headRefName":"closed","headRefOid":"b","state":"CLOSED","reviewDecision":"APPROVED"},
            {"number":3,"title":"Open","url":"u","headRefName":"open","headRefOid":"c","state":"OPEN","reviewDecision":null}
        ]"#,
    )
    .unwrap();

    assert_eq!(matches[0].pull_request.status, PullRequestStatus::Merged);
    assert_eq!(matches[1].pull_request.status, PullRequestStatus::Closed);
    assert_eq!(matches[2].pull_request.status, PullRequestStatus::Open);
}
