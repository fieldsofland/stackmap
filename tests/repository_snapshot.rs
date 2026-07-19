mod common;

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use rusqlite::Connection;
use stackmap::adapters::git::{DeleteOutcome, DeleteRequest, GitAdapter};
use stackmap::adapters::graphite::read_topology;
use stackmap::model::{BranchId, ValidationError};

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
