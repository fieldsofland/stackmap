mod common;

use std::sync::Arc;
use std::{fs, path::PathBuf};

use stackmap::adapters::git::GitAdapter;
use stackmap::app::App;
use stackmap::config::config_path;
use stackmap::model::BranchId;
use stackmap::model::RemoteRefEvidence;
use stackmap::refresh::builder::SnapshotBuilder;
use stackmap::refresh::upstream::{UpstreamBatch, UpstreamCommand, UpstreamResult};

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
            .branch(&stackmap::model::BranchId::new("new"))
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
fn active_view_never_requests_upstream_and_archive_targets_only_hidden_rows() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("hidden", None, "hidden", false),
        common::branch("visible", None, "visible", false),
    ]));
    assert!(app.take_upstream_command().is_none());
    app.config
        .set_archived_in_memory(&BranchId::new("hidden"), true);

    app.handle_key(stackmap::events::Key::Character('a'));
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

    app.handle_key(stackmap::events::Key::Character('a'));
    assert_eq!(app.take_upstream_command(), Some(UpstreamCommand::Cancel));
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
    app.handle_key(stackmap::events::Key::Character('a'));
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
