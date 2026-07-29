use super::common;

use std::sync::Arc;
use std::{fs, path::PathBuf};

use crate::adapters::git::GitAdapter;
use crate::app::{Action, App};
use crate::config::config_path;
use crate::events::Key;
use crate::model::BranchId;
use crate::model::RemoteRefEvidence;
use crate::refresh::builder::SnapshotBuilder;
use crate::refresh::upstream::{UpstreamBatch, UpstreamCommand, UpstreamResult};

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
fn active_and_archive_views_request_remote_evidence_for_visible_rows() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("hidden", None, "hidden", false),
        common::branch("visible", None, "visible", false),
    ]));
    let Some(UpstreamCommand::Request(active)) = app.take_upstream_command() else {
        panic!("active view should request visible remote evidence");
    };
    assert_eq!(active.targets.len(), 3);
    app.config
        .set_archived_in_memory(&BranchId::new("hidden"), true);

    app.handle_key(crate::events::Key::Character('a'));
    let Some(UpstreamCommand::Request(request)) = app.take_upstream_command() else {
        panic!("entering Archive should request visible hidden evidence");
    };
    assert_eq!(request.targets.len(), 1);
    assert_eq!(request.targets[0].branch, BranchId::new("hidden"));
    assert!(matches!(
        app.snapshot
            .as_ref()
            .unwrap()
            .branch(&BranchId::new("hidden"))
            .unwrap()
            .remote_ref,
        RemoteRefEvidence::Checking
    ));
    assert!(app.take_upstream_command().is_none());

    app.handle_key(crate::events::Key::Character('a'));
    let Some(UpstreamCommand::Request(active_again)) = app.take_upstream_command() else {
        panic!("returning to Active should request visible remote evidence");
    };
    assert_eq!(active_again.targets.len(), 2);
    assert!(app.take_upstream_command().is_none());
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
        }]),
    };
    app.apply_upstream_batch(batch(
        request.generation + 1,
        11,
        request.targets[0].oid.clone(),
    ));
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
    app.apply_upstream_batch(batch(
        request.generation,
        11,
        request.targets[0].oid.clone(),
    ));
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
