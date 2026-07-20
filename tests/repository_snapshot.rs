mod common;

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use rusqlite::Connection;
use stackmap::adapters::git::{DeleteOutcome, DeleteRequest, GitAdapter};
use stackmap::adapters::graphite::read_topology;
use stackmap::app::App;
use stackmap::model::topology::{DividerRow, ProjectionEntry};
use stackmap::model::{BranchId, ConfiguredUpstream, ValidationError};
use stackmap::refresh::builder::SnapshotBuilder;

#[test]
fn git_inventory_tracks_every_local_branch_and_dirty_state() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "feature/one"]);
    common::git(repository.path(), &["branch", "feature/naïve"]);
    fs::write(repository.path().join("untracked.txt"), "dirty\n").unwrap();

    let inventory = GitAdapter::discover(repository.path())
        .unwrap()
        .inventory()
        .unwrap();
    let names: HashSet<_> = inventory
        .branches
        .iter()
        .map(|branch| branch.id.0.as_ref())
        .collect();
    assert_eq!(
        names,
        HashSet::from(["main", "feature/one", "feature/naïve"])
    );
    assert_eq!(inventory.current, Some(BranchId::new("main")));
    assert!(inventory.dirty);
}

#[test]
fn local_remote_fixture_classifies_upstreams_and_containment_without_fetching() {
    let repository = common::init_repo();
    let remote = tempfile::tempdir().unwrap();
    common::git(remote.path(), &["init", "--bare"]);
    common::git(
        repository.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    common::git(repository.path(), &["push", "-u", "origin", "main"]);
    let base = common::git(repository.path(), &["rev-parse", "HEAD"]);

    fs::write(repository.path().join("file.txt"), "base\nremote-newer\n").unwrap();
    common::git(repository.path(), &["add", "file.txt"]);
    common::git(repository.path(), &["commit", "-m", "remote newer"]);
    common::git(repository.path(), &["push", "origin", "main"]);

    common::git(repository.path(), &["branch", "equal", "origin/main"]);
    common::git(
        repository.path(),
        &["branch", "--set-upstream-to=origin/main", "equal"],
    );
    common::git(repository.path(), &["switch", "-c", "ahead", "origin/main"]);
    fs::write(repository.path().join("ahead.txt"), "ahead\n").unwrap();
    common::git(repository.path(), &["add", "ahead.txt"]);
    common::git(repository.path(), &["commit", "-m", "ahead"]);
    common::git(repository.path(), &["switch", "main"]);

    common::git(repository.path(), &["branch", "behind", &base]);
    common::git(
        repository.path(),
        &["branch", "--set-upstream-to=origin/main", "behind"],
    );
    common::git(repository.path(), &["switch", "-c", "diverged", &base]);
    fs::write(repository.path().join("diverged.txt"), "diverged\n").unwrap();
    common::git(repository.path(), &["add", "diverged.txt"]);
    common::git(repository.path(), &["commit", "-m", "diverged"]);
    common::git(
        repository.path(),
        &["branch", "--set-upstream-to=origin/main", "diverged"],
    );
    common::git(repository.path(), &["switch", "main"]);

    common::git(
        repository.path(),
        &["update-ref", "refs/remotes/origin/gone", &base],
    );
    common::git(repository.path(), &["branch", "gone", &base]);
    common::git(
        repository.path(),
        &["branch", "--set-upstream-to=origin/gone", "gone"],
    );
    common::git(
        repository.path(),
        &["update-ref", "-d", "refs/remotes/origin/gone"],
    );
    common::git(
        repository.path(),
        &["branch", "--no-track", "contained", "origin/main"],
    );
    common::git(
        repository.path(),
        &["branch", "--no-track", "local-only", "ahead"],
    );
    common::git(repository.path(), &["branch", "malformed", &base]);
    common::git(
        repository.path(),
        &["config", "branch.malformed.remote", "missing"],
    );
    common::git(
        repository.path(),
        &["config", "branch.malformed.merge", "refs/heads/nowhere"],
    );

    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let inventory = adapter.inventory().unwrap();
    let upstream = |name: &str| {
        &inventory
            .branches
            .iter()
            .find(|branch| branch.id == BranchId::new(name))
            .unwrap()
            .configured_upstream
    };
    assert!(matches!(
        upstream("equal"),
        ConfiguredUpstream::Equal { .. }
    ));
    assert!(matches!(
        upstream("ahead"),
        ConfiguredUpstream::Ahead { ahead: 1, .. }
    ));
    assert!(matches!(
        upstream("behind"),
        ConfiguredUpstream::Behind { behind: 1, .. }
    ));
    assert!(matches!(
        upstream("diverged"),
        ConfiguredUpstream::Diverged {
            ahead: 1,
            behind: 1,
            ..
        }
    ));
    assert!(matches!(upstream("gone"), ConfiguredUpstream::Gone { .. }));
    assert_eq!(upstream("contained"), &ConfiguredUpstream::None);
    assert!(matches!(
        upstream("malformed"),
        ConfiguredUpstream::Unavailable { .. }
    ));

    let contained_oid = common::git(repository.path(), &["rev-parse", "contained"]);
    let local_only_oid = common::git(repository.path(), &["rev-parse", "local-only"]);
    let token = adapter.remote_ref_token().unwrap();
    common::git(
        repository.path(),
        &["update-ref", "refs/remotes/origin/aaa", &contained_oid],
    );
    common::git(
        repository.path(),
        &["update-ref", "refs/remotes/origin/zzz", &contained_oid],
    );
    assert_eq!(
        adapter
            .containing_remote_ref(&contained_oid)
            .unwrap()
            .as_deref(),
        Some("origin/aaa")
    );
    assert_eq!(
        adapter.containing_remote_ref(&local_only_oid).unwrap(),
        None
    );
    let multiple_refs_token = adapter.remote_ref_token().unwrap();
    assert_ne!(multiple_refs_token, token);
    common::git(
        repository.path(),
        &["update-ref", "refs/remotes/origin/token-change", &base],
    );
    let added_token = adapter.remote_ref_token().unwrap();
    assert_ne!(added_token, multiple_refs_token);
    common::git(
        repository.path(),
        &[
            "update-ref",
            "refs/remotes/origin/token-change",
            "origin/main",
        ],
    );
    assert_ne!(adapter.remote_ref_token().unwrap(), added_token);
}

#[test]
fn oversized_remote_ref_output_does_not_block_active_local_inventory() {
    let repository = common::init_repo();
    let oid = common::git(repository.path(), &["rev-parse", "HEAD"]);
    let mut packed = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for index in 0..5_000 {
        packed.push_str(&format!(
            "{oid} refs/remotes/origin/archive-{index:05}-with-a-long-name\n"
        ));
    }
    fs::write(repository.path().join(".git/packed-refs"), packed).unwrap();

    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let inventory = adapter.inventory().unwrap();
    assert_eq!(inventory.branches.len(), 1);
    let error = adapter.remote_ref_token().unwrap_err().to_string();
    assert!(error.contains("exceeded"), "unexpected error: {error}");
}

#[test]
fn oversized_upstream_config_degrades_without_blocking_active_inventory() {
    let repository = common::init_repo();
    let config_path = repository.path().join(".git/config");
    let mut config = fs::read_to_string(&config_path).unwrap();
    for index in 0..5_000 {
        config.push_str(&format!(
            "\n[branch \"archived-{index:05}-with-a-long-name\"]\n\tremote = origin\n\tmerge = refs/heads/archive-{index:05}\n"
        ));
    }
    fs::write(config_path, config).unwrap();

    let inventory = GitAdapter::discover(repository.path())
        .unwrap()
        .inventory()
        .unwrap();
    assert_eq!(inventory.branches.len(), 1);
    assert!(matches!(
        inventory.branches[0].configured_upstream,
        ConfiguredUpstream::Unavailable { .. }
    ));
}

#[test]
fn graphite_fixture_loads_exact_parents_read_only() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "feature/root"]);
    common::git(repository.path(), &["branch", "feature/child"]);
    let git_dir = repository.path().join(".git");
    fs::write(git_dir.join(".graphite_repo_config"), r#"{"trunk":"main"}"#).unwrap();
    let connection = Connection::open(git_dir.join(".graphite_metadata.db")).unwrap();
    connection
        .execute_batch(include_str!("fixtures/graphite/supported-schema.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["feature/root", "main"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["feature/child", "feature/root"],
        )
        .unwrap();
    drop(connection);
    let local = HashSet::from([
        BranchId::new("main"),
        BranchId::new("feature/root"),
        BranchId::new("feature/child"),
    ]);

    let topology = read_topology(&git_dir, &local);
    assert_eq!(topology.default_trunk, Some(BranchId::new("main")));
    assert_eq!(
        topology.parents[&BranchId::new("feature/child")],
        BranchId::new("feature/root")
    );
    assert_eq!(
        topology.parents[&BranchId::new("feature/root")],
        BranchId::new("main")
    );
}

#[test]
fn real_git_and_graphite_fork_builds_exact_app_lanes_and_trunk_connector() {
    let repository = common::init_repo();
    for branch in ["1", "2", "3", "4", "3b"] {
        common::git(repository.path(), &["branch", branch]);
    }
    let git_dir = repository.path().join(".git");
    fs::write(git_dir.join(".graphite_repo_config"), r#"{"trunk":"main"}"#).unwrap();
    let connection = Connection::open(git_dir.join(".graphite_metadata.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE branch_metadata (\
                branch_name TEXT PRIMARY KEY NOT NULL, \
                parent_branch_name TEXT, \
                children TEXT\
            );",
        )
        .unwrap();
    for (branch, parent, children) in [
        ("main", None, r#"["1"]"#),
        ("1", Some("main"), r#"["2"]"#),
        ("2", Some("1"), r#"["3"]"#),
        ("3", Some("2"), r#"["4","3b"]"#),
        ("4", Some("3"), "[]"),
        ("3b", Some("3"), "[]"),
    ] {
        connection
            .execute(
                "INSERT INTO branch_metadata VALUES (?1, ?2, ?3)",
                rusqlite::params![branch, parent, children],
            )
            .unwrap();
    }
    drop(connection);

    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let snapshot = SnapshotBuilder::new(adapter).build().unwrap();
    assert_eq!(
        snapshot.branch(&BranchId::new("4")).unwrap().parent,
        Some(BranchId::new("3"))
    );
    assert_eq!(
        snapshot.branch(&BranchId::new("3b")).unwrap().parent,
        Some(BranchId::new("3"))
    );
    assert_eq!(
        snapshot.branch(&BranchId::new("3")).unwrap().stack_root,
        BranchId::new("1")
    );
    assert_eq!(
        snapshot.branch(&BranchId::new("3")).unwrap().trunk,
        Some(BranchId::new("main"))
    );

    let mut app = App::default();
    app.apply_snapshot(snapshot);
    let branch_rows: Vec<_> = app
        .projection
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectionEntry::Branch(row) => Some(row.branch.0.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(branch_rows, ["4", "3b", "3", "2", "1", "main"]);
    assert_eq!(app.projection.row_for(&BranchId::new("4")).unwrap().lane, 1);
    assert_eq!(
        app.projection.row_for(&BranchId::new("3b")).unwrap().lane,
        2
    );
    let side_connector = app
        .projection
        .entries
        .iter()
        .find_map(|entry| match entry {
            ProjectionEntry::Divider(DividerRow::Connector(connector))
                if connector.stack_id == BranchId::new("3b") =>
            {
                Some(connector)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(side_connector.parent, Some(BranchId::new("3")));
    let trunk_connector = app
        .projection
        .entries
        .iter()
        .find_map(|entry| match entry {
            ProjectionEntry::Divider(DividerRow::Connector(connector))
                if connector.stack_id == BranchId::new("1") =>
            {
                Some(connector)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(trunk_connector.parent, Some(BranchId::new("main")));
    assert_eq!((trunk_connector.from_lane, trunk_connector.to_lane), (1, 0));
}

#[test]
fn graphite_loads_all_ordered_local_trunks_and_classifies_each_chain() {
    let repository = common::init_repo();
    for branch in ["preview", "staging/root", "staging/tip", "preview-feature"] {
        common::git(repository.path(), &["branch", branch]);
    }
    let git_dir = repository.path().join(".git");
    fs::write(
        git_dir.join(".graphite_repo_config"),
        r#"{"trunk":"main","trunks":[{"name":"main"},{"name":"preview"},{"name":"main"}]}"#,
    )
    .unwrap();
    let connection = Connection::open(git_dir.join(".graphite_metadata.db")).unwrap();
    connection
        .execute_batch(include_str!("fixtures/graphite/supported-schema.sql"))
        .unwrap();
    for (branch, parent) in [
        ("main", None),
        ("preview", None),
        ("staging/root", Some("main")),
        ("staging/tip", Some("staging/root")),
        ("preview-feature", Some("preview")),
    ] {
        connection
            .execute(
                "INSERT INTO branch_metadata (branch_name, parent_branch_name) VALUES (?1, ?2)",
                rusqlite::params![branch, parent],
            )
            .unwrap();
    }
    drop(connection);
    let local = [
        "main",
        "preview",
        "staging/root",
        "staging/tip",
        "preview-feature",
    ]
    .into_iter()
    .map(BranchId::new)
    .collect();

    let topology = read_topology(&git_dir, &local);
    assert_eq!(topology.default_trunk, Some(BranchId::new("main")));
    assert_eq!(
        topology.trunks.as_ref(),
        [BranchId::new("main"), BranchId::new("preview")]
    );
    assert_eq!(
        topology.trunk_by_branch[&BranchId::new("staging/tip")],
        BranchId::new("main")
    );
    assert_eq!(
        topology.trunk_by_branch[&BranchId::new("preview-feature")],
        BranchId::new("preview")
    );
}

#[test]
fn missing_configured_trunk_degrades_its_branches_without_hiding_other_topology() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "valid"]);
    common::git(repository.path(), &["branch", "orphan"]);
    let git_dir = repository.path().join(".git");
    fs::write(
        git_dir.join(".graphite_repo_config"),
        r#"{"trunk":"main","trunks":[{"name":"main"},{"name":"missing"}]}"#,
    )
    .unwrap();
    let connection = Connection::open(git_dir.join(".graphite_metadata.db")).unwrap();
    connection
        .execute_batch(include_str!("fixtures/graphite/supported-schema.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["valid", "main"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["orphan", "missing"],
        )
        .unwrap();
    drop(connection);
    let local = ["main", "valid", "orphan"]
        .into_iter()
        .map(BranchId::new)
        .collect();

    let topology = read_topology(&git_dir, &local);
    assert_eq!(topology.trunks.as_ref(), [BranchId::new("main")]);
    assert_eq!(
        topology.trunk_by_branch.get(&BranchId::new("valid")),
        Some(&BranchId::new("main"))
    );
    assert!(!topology.parents.contains_key(&BranchId::new("orphan")));
    assert!(topology.degraded.contains(&BranchId::new("orphan")));
    assert!(topology.status.contains("missing"));
}

#[test]
fn missing_graphite_metadata_is_explicitly_degraded() {
    let repository = common::init_repo();
    let local = HashSet::from([BranchId::new("main")]);
    let topology = read_topology(&repository.path().join(".git"), &local);
    assert!(topology.parents.is_empty());
    assert!(topology.status.contains("topology unavailable"));
}

#[test]
fn snapshot_rejects_duplicates_and_cycles() {
    let duplicate = common::snapshot(vec![
        common::branch("one", None, "one", false),
        common::branch("one", None, "one", false),
    ]);
    assert!(matches!(
        duplicate.validate(),
        Err(ValidationError::DuplicateBranch(_))
    ));

    let cyclic = common::snapshot(vec![
        common::branch("one", Some("two"), "one", false),
        common::branch("two", Some("one"), "one", false),
    ]);
    assert!(matches!(cyclic.validate(), Err(ValidationError::Cycle(_))));
}

#[test]
fn graphite_quarantines_descendants_of_a_missing_parent() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "feature/root"]);
    common::git(repository.path(), &["branch", "feature/child"]);
    let git_dir = repository.path().join(".git");
    fs::write(git_dir.join(".graphite_repo_config"), r#"{"trunk":"main"}"#).unwrap();
    let connection = Connection::open(git_dir.join(".graphite_metadata.db")).unwrap();
    connection
        .execute_batch(include_str!("fixtures/graphite/supported-schema.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["feature/root", "missing"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO branch_metadata VALUES (?1, ?2)",
            ["feature/child", "feature/root"],
        )
        .unwrap();
    drop(connection);
    let local = HashSet::from([
        BranchId::new("main"),
        BranchId::new("feature/root"),
        BranchId::new("feature/child"),
    ]);
    let topology = read_topology(&git_dir, &local);
    assert!(topology.parents.is_empty());
}

#[test]
fn definitely_untracked_deletion_is_merged_and_expected_oid_atomic() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "merged"]);
    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let inventory = adapter.inventory().unwrap();
    let branch = inventory
        .branches
        .iter()
        .find(|branch| branch.id == BranchId::new("merged"))
        .unwrap();
    let request = DeleteRequest {
        branch: branch.id.clone(),
        expected_oid: branch.oid.clone(),
        expected_provenance: stackmap::model::GraphiteProvenance::DefinitelyUntracked,
    };
    assert_eq!(
        adapter.delete_branch(&request).unwrap(),
        DeleteOutcome::Deleted
    );
    assert!(
        adapter
            .inventory()
            .unwrap()
            .branches
            .iter()
            .all(|branch| branch.id != request.branch)
    );
}

#[test]
fn unmerged_or_stale_untracked_deletion_preserves_the_ref() {
    let repository = common::init_repo();
    common::git(repository.path(), &["switch", "-c", "unmerged"]);
    fs::write(repository.path().join("new.txt"), "new\n").unwrap();
    common::git(repository.path(), &["add", "new.txt"]);
    common::git(repository.path(), &["commit", "-m", "new"]);
    let oid = common::git(repository.path(), &["rev-parse", "HEAD"]);
    common::git(repository.path(), &["switch", "main"]);
    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let request = DeleteRequest {
        branch: BranchId::new("unmerged"),
        expected_oid: Arc::from(oid),
        expected_provenance: stackmap::model::GraphiteProvenance::DefinitelyUntracked,
    };
    assert!(adapter.delete_branch(&request).is_err());
    assert!(
        adapter
            .inventory()
            .unwrap()
            .branches
            .iter()
            .any(|branch| branch.id == request.branch)
    );

    let stale = DeleteRequest {
        expected_oid: Arc::from("0000000000000000000000000000000000000000"),
        ..request
    };
    assert!(adapter.delete_branch(&stale).is_err());
    assert!(
        adapter
            .inventory()
            .unwrap()
            .branches
            .iter()
            .any(|branch| branch.id == stale.branch)
    );
}
