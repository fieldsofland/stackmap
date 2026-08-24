use super::common;

use std::sync::Arc;

use crate::adapters::github::{GitHubError, PrMatch, parse_json};
use crate::app::{Action, App, GitHubState};
use crate::events::Key;
use crate::model::{BranchId, PullRequest, PullRequestLookup, PullRequestMatch, PullRequestStatus};
use crate::refresh::github::{GithubBatch, GithubOutcome, GithubResult};

#[test]
fn parses_batched_pr_json_and_retains_missing_head_oid_as_stale_candidate() {
    let matches = parse_json(include_bytes!("../../tests/fixtures/github/pr-list.json")).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].branch, BranchId::new("feature/stack-map"));
    assert_eq!(matches[0].pull_request.number, 42);
    assert_eq!(matches[0].pull_request.status, PullRequestStatus::Approved);
    assert!(matches[1].head_oid.is_none());
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
        head_oid: Some(Arc::from("wrong-oid")),
        updated_at: Arc::from("2026-01-01T00:00:00Z"),
        pull_request: PullRequest {
            number: 42,
            title: Arc::from("stale"),
            url: Arc::from("https://example.invalid/42"),
            status: PullRequestStatus::Open,
            head_oid: Some(Arc::from("wrong-oid")),
            match_quality: PullRequestMatch::StaleTip,
        },
    };
    app.apply_prs(vec![result.clone()]);
    assert_eq!(
        app.selected_branch()
            .unwrap()
            .pr
            .as_ref()
            .unwrap()
            .match_quality,
        PullRequestMatch::StaleTip
    );
    app.apply_prs(vec![PrMatch {
        head_oid: Some(Arc::from("oid-feature/stack-map")),
        pull_request: PullRequest {
            head_oid: Some(Arc::from("oid-feature/stack-map")),
            ..result.pull_request.clone()
        },
        ..result
    }]);
    assert_eq!(
        app.selected_branch()
            .unwrap()
            .pr
            .as_ref()
            .unwrap()
            .match_quality,
        PullRequestMatch::ExactTip
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
fn partial_follow_up_does_not_clear_last_known_pull_request() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]));
    let Some(crate::refresh::github::GithubCommand::Request(request)) = app.take_github_command()
    else {
        panic!("structural snapshot should request PR enrichment");
    };
    let known = PrMatch {
        branch: BranchId::new("feature/stack-map"),
        head_oid: Some(Arc::from("oid-feature/stack-map")),
        updated_at: Arc::from("2026-01-01T00:00:00Z"),
        pull_request: PullRequest {
            number: 42,
            title: Arc::from("known"),
            url: Arc::from("https://example.invalid/42"),
            status: PullRequestStatus::Open,
            head_oid: Some(Arc::from("oid-feature/stack-map")),
            match_quality: PullRequestMatch::ExactTip,
        },
    };
    app.apply_github_batch(GithubBatch {
        generation: request.generation,
        results: Arc::from([GithubResult {
            branch: BranchId::new("feature/stack-map"),
            oid: Arc::from("oid-feature/stack-map"),
            outcome: GithubOutcome::Ready(Arc::from([known])),
        }]),
    });
    app.apply_github_batch(GithubBatch {
        generation: request.generation,
        results: Arc::from([GithubResult {
            branch: BranchId::new("feature/stack-map"),
            oid: Arc::from("oid-feature/stack-map"),
            outcome: GithubOutcome::Unavailable(GitHubError::NonZero(Arc::from("offline"))),
        }]),
    });

    assert_eq!(
        app.selected_branch()
            .unwrap()
            .pr
            .as_ref()
            .map(|pr| pr.number),
        Some(42),
        "a partial provider result must not erase retained branch evidence"
    );
    assert!(matches!(app.github_state, GitHubState::Unavailable(_)));
}

#[test]
fn new_tip_retains_pr_as_stale_when_follow_up_is_unavailable() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]));
    let Some(crate::refresh::github::GithubCommand::Request(request)) = app.take_github_command()
    else {
        panic!("initial request");
    };
    app.apply_github_batch(GithubBatch {
        generation: request.generation,
        results: Arc::from([GithubResult {
            branch: BranchId::new("feature/stack-map"),
            oid: Arc::from("oid-feature/stack-map"),
            outcome: GithubOutcome::Ready(Arc::from([PrMatch {
                branch: BranchId::new("feature/stack-map"),
                head_oid: Some(Arc::from("oid-feature/stack-map")),
                updated_at: Arc::from("2026-01-01T00:00:00Z"),
                pull_request: PullRequest {
                    number: 42,
                    title: Arc::from("known"),
                    url: Arc::from("https://example.invalid/42"),
                    status: PullRequestStatus::Open,
                    head_oid: Some(Arc::from("oid-feature/stack-map")),
                    match_quality: PullRequestMatch::ExactTip,
                },
            }])),
        }]),
    });

    let mut next = (*common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]))
    .clone();
    next.generation = request.generation + 1;
    Arc::make_mut(&mut next.branches)[0].oid = Arc::from("new-oid");
    app.apply_snapshot(Arc::new(next));
    let retained = app.selected_branch().unwrap().pr.as_ref().unwrap();
    assert_eq!(retained.number, 42);
    assert_eq!(retained.match_quality, PullRequestMatch::StaleTip);

    let Some(crate::refresh::github::GithubCommand::Request(follow_up)) = app.take_github_command()
    else {
        panic!("new tip should request PR enrichment");
    };
    app.apply_github_batch(GithubBatch {
        generation: follow_up.generation,
        results: Arc::from([GithubResult {
            branch: BranchId::new("feature/stack-map"),
            oid: Arc::from("new-oid"),
            outcome: GithubOutcome::Unavailable(GitHubError::NonZero(Arc::from("offline"))),
        }]),
    });
    assert_eq!(
        app.selected_branch().unwrap().pr.as_ref().unwrap().number,
        42
    );
    assert_eq!(
        app.handle_key(Key::Character('o')),
        Action::OpenUrl(Arc::from("https://example.invalid/42"))
    );
}

#[test]
fn merged_pr_without_head_oid_remains_visible_as_stale() {
    let matches = parse_json(
        br#"[{"number":9,"title":"Merged","url":"u","headRefName":"feature/stack-map","headRefOid":null,"state":"MERGED","reviewDecision":"APPROVED"}]"#,
    )
    .unwrap();
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature/stack-map",
        None,
        "feature/stack-map",
        true,
    )]));
    app.apply_prs(matches);
    let pr = app.selected_branch().unwrap().pr.as_ref().unwrap();
    assert_eq!(pr.status, PullRequestStatus::Merged);
    assert_eq!(pr.match_quality, PullRequestMatch::StaleTip);
    assert!(pr.head_oid.is_none());
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

#[test]
fn exact_match_selection_is_stable_under_response_permutation() {
    fn candidate(number: u64, oid: &str, status: PullRequestStatus, updated_at: &str) -> PrMatch {
        PrMatch {
            branch: BranchId::new("feature/stack-map"),
            head_oid: Some(Arc::from(oid)),
            updated_at: Arc::from(updated_at),
            pull_request: PullRequest {
                number,
                title: Arc::from(format!("PR {number}")),
                url: Arc::from(format!("https://example.invalid/{number}")),
                status,
                head_oid: Some(Arc::from(oid)),
                match_quality: PullRequestMatch::StaleTip,
            },
        }
    }
    let candidates = vec![
        candidate(
            99,
            "stale",
            PullRequestStatus::Approved,
            "2026-04-01T00:00:00Z",
        ),
        candidate(
            3,
            "oid-feature/stack-map",
            PullRequestStatus::Open,
            "2026-04-01T00:00:00Z",
        ),
        candidate(
            4,
            "oid-feature/stack-map",
            PullRequestStatus::Approved,
            "2026-01-01T00:00:00Z",
        ),
        candidate(
            8,
            "oid-feature/stack-map",
            PullRequestStatus::Approved,
            "2026-03-01T00:00:00Z",
        ),
        candidate(
            9,
            "oid-feature/stack-map",
            PullRequestStatus::Approved,
            "2026-03-01T00:00:00Z",
        ),
    ];
    for matches in [candidates.clone(), candidates.into_iter().rev().collect()] {
        let mut app = App::default();
        app.apply_snapshot(common::snapshot(vec![common::branch(
            "feature/stack-map",
            None,
            "feature/stack-map",
            true,
        )]));
        app.apply_prs(matches);
        assert_eq!(
            app.selected_branch().unwrap().pr.as_ref().unwrap().number,
            9
        );
    }
}

#[test]
fn lookup_batches_distinguish_states_and_reject_old_generation_or_oid() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("ready", None, "ready", true),
        common::branch("missing", None, "missing", false),
        common::branch("failed", None, "failed", false),
    ]));
    let Some(crate::refresh::github::GithubCommand::Request(request)) = app.take_github_command()
    else {
        panic!("structural snapshot should request PR enrichment");
    };
    assert!(request.targets.iter().all(|target| {
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&target.branch)
            .is_some_and(|branch| branch.pr_lookup == PullRequestLookup::Checking)
    }));
    let ready = PrMatch {
        branch: BranchId::new("ready"),
        head_oid: Some(Arc::from("oid-ready")),
        updated_at: Arc::from("2026-01-01T00:00:00Z"),
        pull_request: PullRequest {
            number: 7,
            title: Arc::from("Ready"),
            url: Arc::from("https://example.invalid/7"),
            status: PullRequestStatus::Approved,
            head_oid: Some(Arc::from("oid-ready")),
            match_quality: PullRequestMatch::StaleTip,
        },
    };
    app.apply_github_batch(GithubBatch {
        generation: request.generation,
        results: Arc::from([
            GithubResult {
                branch: BranchId::new("ready"),
                oid: Arc::from("oid-ready"),
                outcome: GithubOutcome::Ready(Arc::from([ready])),
            },
            GithubResult {
                branch: BranchId::new("missing"),
                oid: Arc::from("oid-missing"),
                outcome: GithubOutcome::NoMatch,
            },
            GithubResult {
                branch: BranchId::new("failed"),
                oid: Arc::from("oid-failed"),
                outcome: GithubOutcome::Unavailable(GitHubError::NonZero(Arc::from("offline"))),
            },
        ]),
    });
    let snapshot = app.snapshot.as_ref().unwrap();
    assert_eq!(
        snapshot.branch(&BranchId::new("ready")).unwrap().pr_lookup,
        PullRequestLookup::Ready
    );
    assert_eq!(
        snapshot
            .branch(&BranchId::new("missing"))
            .unwrap()
            .pr_lookup,
        PullRequestLookup::NoMatch
    );
    assert!(matches!(
        snapshot.branch(&BranchId::new("failed")).unwrap().pr_lookup,
        PullRequestLookup::Unavailable(_)
    ));

    app.apply_github_batch(GithubBatch {
        generation: request.generation + 1,
        results: Arc::from([GithubResult {
            branch: BranchId::new("missing"),
            oid: Arc::from("oid-missing"),
            outcome: GithubOutcome::Ready(Arc::from([])),
        }]),
    });
    app.apply_github_batch(GithubBatch {
        generation: request.generation,
        results: Arc::from([GithubResult {
            branch: BranchId::new("missing"),
            oid: Arc::from("old-oid"),
            outcome: GithubOutcome::Ready(Arc::from([])),
        }]),
    });
    assert_eq!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("missing"))
            .unwrap()
            .pr_lookup,
        PullRequestLookup::NoMatch
    );
}

#[test]
fn navigation_does_not_schedule_pull_request_provider_work() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("current", None, "current", true),
        common::branch("second", None, "second", false),
        common::branch("third", None, "third", false),
    ]));
    assert!(matches!(
        app.take_github_command(),
        Some(crate::refresh::github::GithubCommand::Request(_))
    ));

    app.handle_key(Key::Down);
    app.handle_key(Key::Down);

    assert!(app.take_github_command().is_none());
}

#[test]
fn newly_created_branch_is_in_the_next_bounded_working_set() {
    let mut app = App::default();
    let first = common::snapshot(vec![common::branch("main", None, "main", true)]);
    app.apply_snapshot(first);
    let _ = app.take_github_command();

    let mut next = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("new-branch", None, "new-branch", false),
    ]))
    .clone();
    next.generation = 2;
    app.apply_snapshot(Arc::new(next));

    let Some(crate::refresh::github::GithubCommand::Request(request)) = app.take_github_command()
    else {
        panic!("new branch should request pull request enrichment");
    };
    assert!(
        request
            .targets
            .iter()
            .any(|target| target.branch == BranchId::new("new-branch"))
    );
}

#[test]
fn pull_request_working_set_is_bounded_for_large_current_stack() {
    let mut branches = Vec::new();
    for index in 0..32 {
        let name = format!("stack-{index:02}");
        let parent = (index > 0).then(|| format!("stack-{:02}", index - 1));
        branches.push(common::branch(
            &name,
            parent.as_deref(),
            "stack-00",
            index == 31,
        ));
    }
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(branches));
    let Some(crate::refresh::github::GithubCommand::Request(request)) = app.take_github_command()
    else {
        panic!("current stack should request pull request enrichment");
    };

    assert_eq!(request.targets.len(), crate::refresh::github::MAX_TARGETS);
    assert!(
        request
            .targets
            .iter()
            .any(|target| target.branch == BranchId::new("stack-31"))
    );
}
