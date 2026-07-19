mod common;

use std::sync::Arc;

use stackmap::adapters::github::{GitHubError, PrMatch, parse_json};
use stackmap::app::App;
use stackmap::model::{BranchId, PullRequest};

#[test]
fn parses_batched_pr_json_and_ignores_unverifiable_entries() {
    let matches = parse_json(include_bytes!("fixtures/github/pr-list.json")).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].branch, BranchId::new("feature/stack-map"));
    assert_eq!(matches[0].pull_request.number, 42);
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
