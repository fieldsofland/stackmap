use super::common;

use std::sync::Arc;

use crate::adapters::github::{GitHubError, PrMatch, parse_json};
use crate::app::App;
use crate::model::{BranchId, PullRequest, PullRequestStatus};

#[test]
fn parses_batched_pr_json_and_ignores_unverifiable_entries() {
    let matches = parse_json(include_bytes!("../../tests/fixtures/github/pr-list.json")).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].branch, BranchId::new("feature/stack-map"));
    assert_eq!(matches[0].pull_request.number, 42);
    assert_eq!(matches[0].pull_request.status, PullRequestStatus::Approved);
}

#[test]
fn delayed_results_match_current_branch_by_id_and_oid() {
    let mut app = App::default();
    let snapshot = common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]);
    app.apply_snapshot(snapshot);
    let result = PrMatch {
        branch: BranchId::new("feature/stack-map"),
        oid: Arc::from("wrong-oid"),
        pull_request: PullRequest {
            number: 42,
            title: Arc::from("stale"),
            url: Arc::from("https://example.invalid/42"),
            status: PullRequestStatus::Open,
        },
    };
    app.apply_prs(vec![result.clone()]);
    assert!(app.selected_branch().unwrap().pr.is_none());
    app.apply_prs(vec![PrMatch {
        oid: Arc::from("oid-feature/stack-map"),
        ..result
    }]);
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        42
    );

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
