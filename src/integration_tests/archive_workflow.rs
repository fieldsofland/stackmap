use super::common;

use std::sync::Arc;

use crate::app::{Action, App, Overlay};
use crate::config::ArchiveMutation;
use crate::events::Key;
use crate::model::topology::{ArchiveMode, ProjectionEntry};
use crate::model::{BranchId, GraphiteProvenance};

fn tracked(
    name: &str,
    parent: Option<&str>,
    root: &str,
    trunk: &str,
    current: bool,
    committed_at: i64,
) -> crate::model::Branch {
    let mut branch = common::branch(name, parent, root, current);
    branch.trunk = Some(BranchId::new(trunk));
    branch.graphite = GraphiteProvenance::Tracked;
    branch.committed_at = committed_at;
    branch
}

fn snapshot() -> Arc<crate::model::RepositorySnapshot> {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", true, 1),
        tracked("alpha", None, "alpha", "main", false, 10),
        tracked("alpha-tip", Some("alpha"), "alpha", "main", false, 11),
        tracked("beta", None, "beta", "main", false, 20),
        common::branch("loose", None, "loose", false),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([(
        BranchId::new("main"),
        Arc::from([BranchId::new("alpha"), BranchId::new("beta")]),
    )]);
    Arc::new(snapshot)
}

fn visible(app: &App) -> Vec<&str> {
    app.projection
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectionEntry::Branch(row) => Some(row.branch.0.as_ref()),
            _ => None,
        })
        .collect()
}

#[test]
fn x_archives_immediately_and_a_restores_active_selection_and_scroll() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("alpha"));
    app.scroll = 1;

    let Action::PersistConfig(request) = app.handle_key(Key::Character('x')) else {
        panic!("archive should persist");
    };
    assert_eq!(
        request
            .mutation
            .archive_updates
            .get(&BranchId::new("alpha")),
        Some(&ArchiveMutation::Set(true))
    );
    assert!(!visible(&app).contains(&"alpha"));
    assert_eq!(app.selected, Some(BranchId::new("alpha-tip")));
    assert_eq!(app.archive_mode, ArchiveMode::Active);
    let active_selected = app.selected.clone();
    let active_scroll = app.scroll;

    app.handle_key(Key::Character('a'));
    assert_eq!(app.archive_mode, ArchiveMode::Archive);
    assert!(visible(&app).contains(&"alpha"));
    app.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(request) = app.handle_key(Key::Character('x')) else {
        panic!("restore should persist");
    };
    assert_eq!(
        request
            .mutation
            .archive_updates
            .get(&BranchId::new("alpha")),
        Some(&ArchiveMutation::Set(false))
    );

    app.handle_key(Key::Character('a'));
    assert_eq!(app.archive_mode, ArchiveMode::Active);
    assert_eq!(app.selected, active_selected);
    assert_eq!(app.scroll, active_scroll);
}

#[test]
fn archive_projection_keeps_unarchived_stack_ancestry_as_nonselectable_context() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("alpha-tip"));
    let Action::PersistConfig(_) = app.handle_key(Key::Character('x')) else {
        panic!("archive should persist");
    };
    app.handle_key(Key::Character('a'));

    assert!(visible(&app).contains(&"alpha-tip"));
    assert!(!visible(&app).contains(&"alpha"));
    assert!(app.projection.entries.iter().any(|entry| {
        matches!(
            entry,
            ProjectionEntry::Divider(crate::model::topology::DividerRow::Placeholder(row))
                if row.branch == BranchId::new("alpha")
        )
    }));
    assert!(!app.projection.selectable.contains(&BranchId::new("alpha")));
}

#[test]
fn archive_refuses_current_and_trunk_rows() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("main"));
    assert_eq!(app.handle_key(Key::Character('x')), Action::None);
    assert!(app.message.as_deref().unwrap().contains("current"));
    assert!(matches!(app.mutation, crate::app::MutationState::Idle));
    assert!(!matches!(
        app.handle_key(Key::Character('y')),
        Action::Delete(_)
    ));
}

#[test]
fn range_preview_skips_protected_rows_stays_in_section_and_commits_atomically() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("beta"));
    app.handle_key(Key::Character('v'));
    assert!(matches!(app.overlay, Overlay::ArchiveRange(_)));

    app.handle_key(Key::Down);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range remains open");
    };
    assert!(range.branch_count() >= 2);
    assert!(!range.branches().contains(&BranchId::new("main")));
    assert!(!range.branches().contains(&BranchId::new("loose")));

    let Action::PersistConfig(request) = app.handle_key(Key::Enter) else {
        panic!("range should persist in one request");
    };
    assert!(request.mutation.archive_updates.len() >= 2);
    assert!(
        request
            .mutation
            .archive_updates
            .values()
            .all(|archived| *archived == ArchiveMutation::Set(true))
    );
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn structural_refresh_cancels_range_and_pending_archives_overlay_reloaded_config() {
    let initial = snapshot();
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(_) = app.handle_key(Key::Character('x')) else {
        panic!("archive should persist");
    };
    app.handle_key(Key::Character('a'));
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('v'));

    let mut refreshed = (*initial).clone();
    refreshed.generation += 1;
    app.apply_snapshot(Arc::new(refreshed));
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.config.archived.contains("alpha"));
}

#[test]
fn authoritative_snapshot_prunes_absent_archived_names_through_config_outbox() {
    let directory = tempfile::tempdir().unwrap();
    let config = crate::config::Config::default();
    let mut seed = crate::config::ConfigMutation::default();
    seed.set_archived(BranchId::new("gone"), true);
    seed.set_archived(BranchId::new("alpha"), true);
    crate::config::Config::persist_mutation(directory.path(), &seed, &config).unwrap();
    let mut input = (*snapshot()).clone();
    input.common_dir = directory.path().to_owned();

    let mut app = App::default();
    app.apply_snapshot(Arc::new(input));
    assert!(!app.config.archived.contains("gone"));
    let request = app
        .take_config_write_request()
        .expect("stale archived name should be persisted");
    assert_eq!(
        request.mutation.archive_updates.get(&BranchId::new("gone")),
        Some(&ArchiveMutation::Prune)
    );
}

#[test]
fn structural_snapshot_unarchives_a_branch_that_became_current() {
    let directory = tempfile::tempdir().unwrap();
    let mut seed = crate::config::ConfigMutation::default();
    seed.set_archived(BranchId::new("alpha"), true);
    seed.set_archived(BranchId::new("beta"), true);
    crate::config::Config::persist_mutation(
        directory.path(),
        &seed,
        &crate::config::Config::default(),
    )
    .unwrap();
    let mut input = (*snapshot()).clone();
    input.common_dir = directory.path().to_owned();
    for branch in Arc::make_mut(&mut input.branches) {
        branch.current = branch.id == BranchId::new("alpha");
    }

    let mut app = App::default();
    app.apply_snapshot(Arc::new(input));

    assert!(!app.config.is_archived(&BranchId::new("alpha")));
    assert!(app.config.is_archived(&BranchId::new("beta")));
    assert!(visible(&app).contains(&"alpha"));
    let request = app
        .take_config_write_request()
        .expect("current branch archive cleanup should persist");
    assert_eq!(
        request
            .mutation
            .archive_updates
            .get(&BranchId::new("alpha")),
        Some(&ArchiveMutation::Prune)
    );
}

#[test]
fn structural_snapshot_unarchives_a_branch_promoted_to_configured_trunk() {
    let directory = tempfile::tempdir().unwrap();
    let mut seed = crate::config::ConfigMutation::default();
    seed.set_archived(BranchId::new("loose"), true);
    seed.set_archived(BranchId::new("beta"), true);
    crate::config::Config::persist_mutation(
        directory.path(),
        &seed,
        &crate::config::Config::default(),
    )
    .unwrap();
    let mut input = (*snapshot()).clone();
    input.common_dir = directory.path().to_owned();
    input.configured_trunks = Arc::from([BranchId::new("main"), BranchId::new("loose")]);
    input.trunks = input.configured_trunks.clone();

    let mut app = App::default();
    app.apply_snapshot(Arc::new(input));

    assert!(!app.config.is_archived(&BranchId::new("loose")));
    assert!(app.config.is_archived(&BranchId::new("beta")));
    assert!(visible(&app).contains(&"loose"));
    let request = app
        .take_config_write_request()
        .expect("configured trunk archive cleanup should persist");
    assert_eq!(
        request
            .mutation
            .archive_updates
            .get(&BranchId::new("loose")),
        Some(&ArchiveMutation::Prune)
    );
}

#[test]
fn color_and_archive_writes_coalesce_and_stale_completion_cannot_win() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(color) = app.handle_key(Key::Character('c')) else {
        panic!("color write");
    };
    let Action::PersistConfig(archive) = app.handle_key(Key::Character('x')) else {
        panic!("archive write");
    };
    assert!(
        archive
            .mutation
            .color_updates
            .contains_key(&BranchId::new("alpha"))
    );
    assert_eq!(
        archive
            .mutation
            .archive_updates
            .get(&BranchId::new("alpha")),
        Some(&ArchiveMutation::Set(true))
    );

    let latest_message = app.message.clone();
    app.finish_config_persistence(color.sequence, Err(anyhow::anyhow!("old failure")));
    assert_eq!(app.message, latest_message);
    let retry = app
        .prepare_pending_config_persistence(archive)
        .expect("both dirty fields remain pending after the failed older write");
    assert!(!retry.mutation.color_updates.is_empty());
    assert!(!retry.mutation.archive_updates.is_empty());
}

#[test]
fn range_expands_and_contracts_in_both_directions_with_boundary_feedback() {
    let mut app = App::default();
    app.apply_snapshot(snapshot());
    app.selected = Some(BranchId::new("alpha-tip"));
    app.handle_key(Key::Character('v'));

    app.handle_key(Key::Up);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branch_count(), 2);
    app.handle_key(Key::Down);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branches(), [BranchId::new("alpha-tip")]);

    app.handle_key(Key::StackDown);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branch_count(), 2);
    app.handle_key(Key::StackUp);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branches(), [BranchId::new("alpha-tip")]);

    for _ in 0..10 {
        app.handle_key(Key::Up);
    }
    let before = app.overlay.clone();
    app.handle_key(Key::Up);
    assert_eq!(app.overlay, before);
    assert!(app.message.as_deref().unwrap().contains("boundary"));
}

#[test]
fn large_section_range_extends_and_shrinks_without_rebuilding_selection_rows() {
    let mut input = (*snapshot()).clone();
    let mut branches = input.branches.to_vec();
    for index in 0..2_000 {
        branches.push(common::branch(
            &format!("loose-{index:04}"),
            None,
            &format!("loose-{index:04}"),
            false,
        ));
    }
    input.branch_index = crate::model::RepositorySnapshot::index_branches(&branches);
    input.branches = branches.into();
    let mut app = App::default();
    app.apply_snapshot(Arc::new(input));
    app.selected = Some(BranchId::new("loose-1000"));
    app.handle_key(Key::Character('v'));

    for _ in 0..500 {
        app.handle_key(Key::Down);
    }
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branch_count(), 501);
    assert!(app.archive_range_contains(&BranchId::new("loose-1000")));
    assert!(app.archive_range_contains(&BranchId::new("loose-1500")));

    for _ in 0..500 {
        app.handle_key(Key::Up);
    }
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    assert_eq!(range.branches(), [BranchId::new("loose-1000")]);
    assert!(!app.archive_range_contains(&BranchId::new("loose-1500")));
}

#[test]
fn range_refuses_protected_anchor_and_revalidation_aborts_the_whole_batch() {
    let initial = snapshot();
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("main"));
    app.handle_key(Key::Character('v'));
    assert_eq!(app.overlay, Overlay::None);

    app.selected = Some(BranchId::new("beta"));
    app.handle_key(Key::Character('v'));
    app.handle_key(Key::Down);
    let Overlay::ArchiveRange(range) = &app.overlay else {
        panic!("range preview");
    };
    let captured = range.branches().to_vec();
    assert!(captured.len() >= 2);

    let invalid = captured[1].clone();
    let mut enriched = (*initial).clone();
    enriched.generation += 1;
    Arc::make_mut(&mut enriched.branches)
        .iter_mut()
        .find(|branch| branch.id == invalid)
        .unwrap()
        .current = true;
    app.apply_enriched_snapshot(Arc::new(enriched));
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(
        captured
            .iter()
            .all(|branch| !app.config.is_archived(branch))
    );
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn persisted_archives_survive_restart_and_pending_restore_beats_stale_completion() {
    let directory = tempfile::tempdir().unwrap();
    let mut first_snapshot = (*snapshot()).clone();
    first_snapshot.common_dir = directory.path().to_owned();
    let mut first = App::default();
    first.apply_snapshot(Arc::new(first_snapshot.clone()));
    first.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(archive) = first.handle_key(Key::Character('x')) else {
        panic!("archive request");
    };
    crate::config::Config::persist_mutation(
        &archive.common_dir,
        &archive.mutation,
        &archive.fallback,
    )
    .unwrap();
    first.finish_config_persistence(archive.sequence, Ok(()));

    let mut restarted = App::default();
    first_snapshot.generation += 1;
    restarted.apply_snapshot(Arc::new(first_snapshot));
    assert!(restarted.config.is_archived(&BranchId::new("alpha")));
    assert!(!visible(&restarted).contains(&"alpha"));
    restarted.handle_key(Key::Character('a'));
    assert!(visible(&restarted).contains(&"alpha"));

    let mut racing = App::default();
    racing.apply_snapshot(snapshot());
    racing.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(stale_archive) = racing.handle_key(Key::Character('x')) else {
        panic!("archive request");
    };
    racing.handle_key(Key::Character('a'));
    racing.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(restore) = racing.handle_key(Key::Character('x')) else {
        panic!("restore request");
    };
    let latest_message = racing.message.clone();
    racing.finish_config_persistence(
        stale_archive.sequence,
        Err(anyhow::anyhow!("stale archive failure")),
    );
    assert_eq!(racing.message, latest_message);
    assert!(!racing.config.is_archived(&BranchId::new("alpha")));
    assert_eq!(
        racing
            .prepare_pending_config_persistence(restore)
            .unwrap()
            .mutation
            .archive_updates
            .get(&BranchId::new("alpha")),
        Some(&ArchiveMutation::Set(false))
    );
}
