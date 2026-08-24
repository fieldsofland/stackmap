use super::common;

use std::sync::Arc;
use std::{fs, path::PathBuf};

use crate::adapters::git::GitAdapter;
use crate::app::{Action, App};
use crate::config::config_path;
use crate::events::Key;
use crate::model::{BranchId, ConfiguredUpstream, RemoteRefEvidence};
use crate::refresh::builder::SnapshotBuilder;
use crate::refresh::github::GithubCommand;
use crate::refresh::graphite_health::HealthCommand;
use crate::refresh::upstream::{MAX_TARGETS, UpstreamBatch, UpstreamCommand, UpstreamResult};

#[test]
fn builder_publishes_complete_generations_after_ref_changes() {
    let repository = common::init_repo();
    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let mut builder = SnapshotBuilder::new(adapter);
    let first = builder.build().unwrap();
    assert_eq!(first.branches.len(), 1);
    common::git(repository.path(), &["branch", "feature"]);
    let second = builder.build().unwrap();
    assert_eq!(second.branches.len(), 2);
    assert!(second.generation > first.generation);
    assert!(first.validate().is_ok());
    assert!(second.validate().is_ok());
}

#[test]
fn graphite_health_targets_are_snapshot_scoped_and_focus_launches_no_more_work() {
    let mut main = common::branch("main", None, "main", false);
    main.graphite_health = crate::model::GraphiteHealth::Healthy;
    let mut a_root = common::branch("a-root", None, "a-root", false);
    let mut a_tip = common::branch("a-tip", Some("a-root"), "a-root", true);
    let mut b_root = common::branch("b-root", None, "b-root", false);
    let mut b_tip = common::branch("b-tip", Some("b-root"), "b-root", false);
    for branch in [&mut a_root, &mut a_tip, &mut b_root, &mut b_tip] {
        branch.graphite = crate::model::GraphiteProvenance::Tracked;
        branch.graphite_health = crate::model::GraphiteHealth::NotRequested;
        branch.trunk = Some(BranchId::new("main"));
    }
    a_root.diff_parent = Some(BranchId::new("main"));
    b_root.diff_parent = Some(BranchId::new("main"));
    let mut app = App::default();
    let mut snapshot = (*common::snapshot(vec![main, a_root, a_tip, b_root, b_tip])).clone();
    snapshot.graphite_children = Arc::from([
        (
            BranchId::new("main"),
            Arc::from([BranchId::new("a-root"), BranchId::new("b-root")]),
        ),
        (BranchId::new("a-root"), Arc::from([BranchId::new("a-tip")])),
        (BranchId::new("b-root"), Arc::from([BranchId::new("b-tip")])),
    ]);
    app.apply_snapshot(Arc::new(snapshot));
    let Some(HealthCommand::Request(initial)) = app.take_graphite_health_command() else {
        panic!("structural snapshot should request Graphite health");
    };
    assert!(
        initial
            .targets
            .iter()
            .any(|target| target.branch.0.starts_with("a-"))
    );
    assert!(
        initial
            .targets
            .iter()
            .all(|target| !target.branch.0.starts_with("b-"))
    );

    app.selected = Some(BranchId::new("a-tip"));
    app.handle_key(Key::Character('h'));
    assert!(app.take_graphite_health_command().is_none());
    app.handle_key(Key::Character('h'));
    assert!(app.take_graphite_health_command().is_none());
}

#[test]
fn unchanged_unavailable_graphite_pair_is_not_retried_on_structural_refresh() {
    let mut main = common::branch("main", None, "main", false);
    main.graphite_health = crate::model::GraphiteHealth::Healthy;
    let mut feature = common::branch("feature", None, "feature", true);
    feature.graphite = crate::model::GraphiteProvenance::Tracked;
    feature.graphite_health = crate::model::GraphiteHealth::NotRequested;
    feature.diff_parent = Some(BranchId::new("main"));
    feature.trunk = Some(BranchId::new("main"));
    let initial = common::snapshot(vec![main, feature]);
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    let Some(HealthCommand::Request(request)) = app.take_graphite_health_command() else {
        panic!("initial discovery request");
    };
    let target = request
        .targets
        .iter()
        .find(|target| target.branch == BranchId::new("feature"))
        .unwrap();
    app.apply_graphite_health_batch(crate::refresh::graphite_health::HealthBatch {
        generation: request.generation,
        results: Arc::from([crate::refresh::graphite_health::HealthResult {
            branch: target.branch.clone(),
            oid: target.oid.clone(),
            parent: target.parent.clone(),
            parent_oid: target.parent_oid.clone(),
            health: crate::model::GraphiteHealth::Unavailable(Arc::from("offline")),
        }]),
    });

    let mut refreshed = (*initial).clone();
    refreshed.generation += 1;
    app.apply_snapshot(Arc::new(refreshed));

    assert!(app.take_graphite_health_command().is_none());
}

#[test]
fn hiding_status_cancels_all_optional_provider_work() {
    let mut app = App::default();
    let mut main = common::branch("main", None, "main", false);
    main.graphite_health = crate::model::GraphiteHealth::Healthy;
    let mut feature = common::branch("feature", None, "feature", true);
    feature.graphite = crate::model::GraphiteProvenance::Tracked;
    feature.graphite_health = crate::model::GraphiteHealth::NotRequested;
    feature.diff_parent = Some(BranchId::new("main"));
    feature.trunk = Some(BranchId::new("main"));
    app.apply_snapshot(common::snapshot(vec![main, feature]));
    let _ = app.take_upstream_command();
    let Some(HealthCommand::Request(_)) = app.take_graphite_health_command() else {
        panic!("current stack should request Graphite health");
    };
    let Some(GithubCommand::Request(_)) = app.take_github_command() else {
        panic!("current stack should request pull request enrichment");
    };
    assert_eq!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("feature"))
            .unwrap()
            .graphite_health,
        crate::model::GraphiteHealth::Checking
    );

    app.handle_key(Key::Character('s'));

    assert_eq!(app.take_upstream_command(), Some(UpstreamCommand::Cancel));
    assert_eq!(
        app.take_graphite_health_command(),
        Some(HealthCommand::Cancel)
    );
    assert_eq!(app.take_github_command(), Some(GithubCommand::Cancel));
    assert_eq!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("feature"))
            .unwrap()
            .graphite_health,
        crate::model::GraphiteHealth::NotRequested
    );

    app.handle_key(Key::Character('s'));
    let Some(HealthCommand::Request(request)) = app.take_graphite_health_command() else {
        panic!("showing status should retry reset in-flight work");
    };
    assert!(
        request
            .targets
            .iter()
            .any(|target| target.branch == BranchId::new("feature"))
    );
}

#[test]
fn reducer_does_not_retain_obsolete_snapshot_generations() {
    let mut app = App::default();
    let first = common::snapshot(vec![common::branch("one", None, "one", true)]);
    let weak = Arc::downgrade(&first);
    app.apply_snapshot(first.clone());
    drop(first);
    let mut second_value =
        (*common::snapshot(vec![common::branch("two", None, "two", true)])).clone();
    second_value.generation = 2;
    app.apply_snapshot(Arc::new(second_value));
    assert!(
        weak.upgrade().is_none(),
        "obsolete immutable snapshot was retained"
    );
}

#[test]
fn pending_visual_section_mutations_coalesce_and_stale_completion_cannot_restore_removed_data() {
    let mut app = App::default();
    let mut root = common::branch("feature", None, "feature", false);
    root.trunk = Some(BranchId::new("main"));
    root.graphite = crate::model::GraphiteProvenance::Tracked;
    let mut main = common::branch("main", None, "main", true);
    main.trunk = Some(BranchId::new("main"));
    main.graphite = crate::model::GraphiteProvenance::Tracked;
    let mut snapshot = (*common::snapshot(vec![main, root])).clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    snapshot.graphite_children =
        Arc::from([(BranchId::new("main"), Arc::from([BranchId::new("feature")]))]);
    app.apply_snapshot(Arc::new(snapshot.clone()));
    app.selected = Some(BranchId::new("feature"));

    let Action::PersistConfig(created) = app.handle_key(Key::Character('i')) else {
        panic!("create");
    };
    let Action::PersistConfig(recolored) = app.handle_key(Key::Character('c')) else {
        panic!("recolor");
    };
    app.handle_key(Key::Character('n'));
    for character in "Named".chars() {
        app.handle_key(Key::Character(character));
    }
    let Action::PersistConfig(named) = app.handle_key(Key::Enter) else {
        panic!("name");
    };
    app.selected_label = None;
    app.selected = Some(BranchId::new("feature"));
    let Action::PersistConfig(removed) = app.handle_key(Key::Character('i')) else {
        panic!("remove");
    };
    assert_eq!(
        removed.mutation.visual_section_updates[&BranchId::new("feature")],
        None
    );

    app.finish_config_persistence(created.sequence, Ok(()));
    app.finish_config_persistence(recolored.sequence, Ok(()));
    app.finish_config_persistence(named.sequence, Ok(()));
    let pending = app
        .prepare_pending_config_persistence(removed.clone())
        .expect("latest removal remains pending after older completions");
    assert_eq!(
        pending.mutation.visual_section_updates[&BranchId::new("feature")],
        None
    );

    snapshot.generation = 2;
    app.apply_snapshot(Arc::new(snapshot));
    assert!(
        app.config
            .visual_section(&BranchId::new("feature"))
            .is_none()
    );
}

#[test]
fn one_hundred_refresh_generations_keep_only_latest_snapshot() {
    let mut app = App::default();
    let mut weak = Vec::new();
    for generation in 1..=100 {
        let mut value = (*common::snapshot(vec![common::branch(
            &format!("branch-{generation}"),
            None,
            &format!("branch-{generation}"),
            true,
        )]))
        .clone();
        value.generation = generation;
        let value = Arc::new(value);
        weak.push(Arc::downgrade(&value));
        app.apply_snapshot(value);
    }
    assert!(
        weak[..99]
            .iter()
            .all(|snapshot| snapshot.upgrade().is_none())
    );
    assert!(weak[99].upgrade().is_some());
}

#[test]
fn older_enriched_snapshot_cannot_replace_newer_structure() {
    let mut app = App::default();
    let mut old = (*common::snapshot(vec![common::branch("old", None, "old", true)])).clone();
    old.generation = 1;
    let mut new = (*common::snapshot(vec![common::branch("new", None, "new", true)])).clone();
    new.generation = 2;
    app.apply_snapshot(Arc::new(new));
    app.apply_snapshot(Arc::new(old));
    assert_eq!(app.snapshot.as_ref().unwrap().generation, 2);
    assert!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&crate::model::BranchId::new("new"))
            .is_some()
    );
}

#[test]
fn valid_snapshot_clears_persistent_stale_health() {
    let mut app = App::default();
    app.mark_stale(Arc::from("temporary failure"));
    assert!(app.refresh_error.is_some());
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    assert!(app.refresh_error.is_none());
}

#[test]
fn delayed_diff_enrichment_does_not_clear_structural_failure() {
    let mut app = App::default();
    let structural = common::snapshot(vec![common::branch("main", None, "main", true)]);
    app.apply_snapshot(structural.clone());
    app.mark_stale(Arc::from("inventory failed"));
    app.apply_enriched_snapshot(structural);
    assert_eq!(app.refresh_error.as_deref(), Some("inventory failed"));
}

#[test]
fn structural_tip_change_moves_selection_to_changed_branch() {
    let initial = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("agent-work", None, "agent-work", false),
        common::branch("other", None, "other", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("other"));

    let mut changed = (*initial).clone();
    changed.generation = 2;
    let branch = Arc::make_mut(&mut changed.branches)
        .iter_mut()
        .find(|branch| branch.id == BranchId::new("agent-work"))
        .unwrap();
    branch.oid = Arc::from("oid-agent-work-updated");
    branch.committed_at += 10;
    app.apply_snapshot(Arc::new(changed));

    assert_eq!(app.selected, Some(BranchId::new("agent-work")));
    assert!(app.selected_label.is_none());
}

#[test]
fn newest_selectable_tip_change_wins_and_hidden_changes_do_not_move_selection() {
    let initial = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("older", None, "older", false),
        common::branch("newer", None, "newer", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("main"));

    let mut changed = (*initial).clone();
    changed.generation = 2;
    for branch in Arc::make_mut(&mut changed.branches) {
        if branch.id == BranchId::new("older") {
            branch.oid = Arc::from("oid-older-updated");
            branch.committed_at += 10;
        } else if branch.id == BranchId::new("newer") {
            branch.oid = Arc::from("oid-newer-updated");
            branch.committed_at += 20;
        }
    }
    app.apply_snapshot(Arc::new(changed.clone()));
    assert_eq!(app.selected, Some(BranchId::new("newer")));

    app.filter = "main".into();
    app.selected = Some(BranchId::new("main"));
    changed.generation = 3;
    Arc::make_mut(&mut changed.branches)
        .iter_mut()
        .find(|branch| branch.id == BranchId::new("older"))
        .unwrap()
        .oid = Arc::from("oid-older-updated-again");
    app.apply_snapshot(Arc::new(changed));
    assert_eq!(app.selected, Some(BranchId::new("main")));
}

#[test]
fn enrichment_tip_change_does_not_move_selection() {
    let initial = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("agent-work", None, "agent-work", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("main"));

    let mut enriched = (*initial).clone();
    enriched.generation = 2;
    Arc::make_mut(&mut enriched.branches)
        .iter_mut()
        .find(|branch| branch.id == BranchId::new("agent-work"))
        .unwrap()
        .oid = Arc::from("oid-agent-work-updated");
    app.apply_enriched_snapshot(Arc::new(enriched));

    assert_eq!(app.selected, Some(BranchId::new("main")));
}

#[test]
fn invalid_config_keeps_last_valid_in_memory_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_path(directory.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "not valid = [toml").unwrap();
    let mut app = App::default();
    app.config.colors.insert("main".into(), "#7aa2f7".into());
    let mut snapshot =
        (*common::snapshot(vec![common::branch("main", None, "main", true)])).clone();
    snapshot.common_dir = PathBuf::from(directory.path());
    app.apply_snapshot(Arc::new(snapshot));
    assert_eq!(app.config.color(&BranchId::new("main")), Some("#7aa2f7"));
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("keeping last valid")
    );
}

#[test]
fn status_enrichment_is_snapshot_scoped_and_view_changes_do_not_request_git_work() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("hidden", None, "hidden", false),
        common::branch("visible", None, "visible", false),
    ]));
    let Some(UpstreamCommand::Request(initial)) = app.take_upstream_command() else {
        panic!("a structural snapshot should request remote evidence");
    };
    assert_eq!(initial.targets.len(), 3);
    app.config
        .set_archived_in_memory(&BranchId::new("hidden"), true);

    app.handle_key(crate::events::Key::Character('a'));
    assert!(app.take_upstream_command().is_none());

    app.handle_key(crate::events::Key::Down);
    assert!(app.take_upstream_command().is_none());

    app.handle_key(crate::events::Key::Character('a'));
    assert!(app.take_upstream_command().is_none());
}

#[test]
fn bounded_status_requests_rotate_through_large_snapshots() {
    let branches = (0..=MAX_TARGETS)
        .map(|index| {
            common::branch(
                &format!("branch-{index}"),
                None,
                &format!("branch-{index}"),
                index == 0,
            )
        })
        .collect::<Vec<_>>();
    let first = common::snapshot(branches);
    let mut app = App::default();
    app.apply_snapshot(first.clone());
    let Some(UpstreamCommand::Request(first_request)) = app.take_upstream_command() else {
        panic!("first bounded request");
    };
    let retained_target = &first_request.targets[0];
    let retained = RemoteRefEvidence::Contained {
        reference: Arc::from("origin/main"),
        source_token: 7,
        checked_at: std::time::SystemTime::UNIX_EPOCH,
    };
    app.apply_upstream_batch(UpstreamBatch {
        generation: first_request.generation,
        remote_ref_token: Some(7),
        results: Arc::from([UpstreamResult {
            branch: retained_target.branch.clone(),
            oid: retained_target.oid.clone(),
            evidence: retained.clone(),
            configured_upstream: None,
        }]),
    });

    let mut second = (*first).clone();
    second.generation += 1;
    app.apply_snapshot(Arc::new(second));
    let Some(UpstreamCommand::Request(second_request)) = app.take_upstream_command() else {
        panic!("rotated bounded request");
    };

    assert_eq!(first_request.targets.len(), MAX_TARGETS);
    assert_eq!(second_request.targets.len(), MAX_TARGETS);
    let rotated_target = second_request
        .targets
        .iter()
        .find(|target| {
            !first_request
                .targets
                .iter()
                .any(|first| first.branch == target.branch)
        })
        .expect("second request should include the branch omitted from the first");
    app.apply_upstream_batch(UpstreamBatch {
        generation: second_request.generation,
        remote_ref_token: Some(7),
        results: Arc::from([UpstreamResult {
            branch: rotated_target.branch.clone(),
            oid: rotated_target.oid.clone(),
            evidence: RemoteRefEvidence::LocalOnly {
                source_token: 7,
                checked_at: std::time::SystemTime::UNIX_EPOCH,
            },
            configured_upstream: None,
        }]),
    });
    assert_eq!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&retained_target.branch)
            .unwrap()
            .remote_ref,
        retained
    );
}

#[test]
fn structural_refresh_keeps_last_status_visible_while_revalidating() {
    let mut feature = common::branch("feature", None, "feature", false);
    feature.configured_upstream = ConfiguredUpstream::Diverged {
        reference: Arc::from("refs/remotes/origin/feature"),
        ahead: 4,
        behind: 2,
    };
    let initial = common::snapshot(vec![common::branch("main", None, "main", true), feature]);
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    let Some(UpstreamCommand::Request(request)) = app.take_upstream_command() else {
        panic!("initial status request");
    };
    let feature_target = request
        .targets
        .iter()
        .find(|target| target.branch == BranchId::new("feature"))
        .unwrap();
    app.apply_upstream_batch(UpstreamBatch {
        generation: initial.generation,
        remote_ref_token: Some(7),
        results: Arc::from([UpstreamResult {
            branch: BranchId::new("feature"),
            oid: feature_target.oid.clone(),
            evidence: RemoteRefEvidence::NotRequested,
            configured_upstream: Some(ConfiguredUpstream::Rewritten {
                reference: Arc::from("refs/remotes/origin/feature"),
                ahead: 4,
                behind: 2,
            }),
        }]),
    });

    let mut refreshed = (*initial).clone();
    refreshed.generation += 1;
    app.apply_snapshot(Arc::new(refreshed));
    assert!(matches!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("feature"))
            .unwrap()
            .configured_upstream,
        ConfiguredUpstream::Rewritten { .. }
    ));
}

#[test]
fn upstream_reducer_rejects_generation_oid_and_token_races() {
    let mut app = App::default();
    let value = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("hidden", None, "hidden", false),
    ]))
    .clone();
    app.apply_snapshot(Arc::new(value));
    app.config
        .set_archived_in_memory(&BranchId::new("hidden"), true);
    app.handle_key(crate::events::Key::Character('a'));
    let Some(UpstreamCommand::Request(request)) = app.take_upstream_command() else {
        panic!("expected request");
    };
    let ready = RemoteRefEvidence::Contained {
        reference: Arc::from("origin/main"),
        source_token: 11,
        checked_at: std::time::SystemTime::UNIX_EPOCH,
    };
    let batch = |generation, token, oid: Arc<str>| UpstreamBatch {
        generation,
        remote_ref_token: Some(token),
        results: Arc::from([UpstreamResult {
            branch: BranchId::new("hidden"),
            oid,
            evidence: ready.clone(),
            configured_upstream: None,
        }]),
    };
    let hidden_oid = request
        .targets
        .iter()
        .find(|target| target.branch == BranchId::new("hidden"))
        .unwrap()
        .oid
        .clone();
    app.apply_upstream_batch(batch(request.generation + 1, 11, hidden_oid.clone()));
    app.apply_upstream_batch(batch(request.generation, 11, Arc::from("changed-oid")));
    assert!(matches!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("hidden"))
            .unwrap()
            .remote_ref,
        RemoteRefEvidence::Checking
    ));
    app.apply_upstream_batch(batch(request.generation, 11, hidden_oid));
    assert_eq!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("hidden"))
            .unwrap()
            .remote_ref,
        ready
    );
}
