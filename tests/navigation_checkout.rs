mod common;

use std::fs;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use stackmap::adapters::git::GitAdapter;
use stackmap::app::{Action, App, LanePitch, MutationState, Overlay, ViewScope};
use stackmap::config::{Config, config_path};
use stackmap::events::{Input, Key};
use stackmap::model::BranchId;
use stackmap::model::topology::{OrderMode, ProjectionEntry};

fn tracked(
    name: &str,
    parent: Option<&str>,
    root: &str,
    trunk: &str,
    current: bool,
    committed_at: i64,
) -> stackmap::model::Branch {
    let mut branch = common::branch(name, parent, root, current);
    branch.trunk = Some(BranchId::new(trunk));
    branch.graphite = stackmap::model::GraphiteProvenance::Tracked;
    branch.committed_at = committed_at;
    branch
}

fn view_snapshot() -> Arc<stackmap::model::RepositorySnapshot> {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", true, 1),
        tracked("alpha", None, "alpha", "main", false, 100),
        tracked("alpha-tip", Some("alpha"), "alpha", "main", false, 101),
        tracked("beta", None, "beta", "main", false, 10),
        common::branch("loose", None, "loose", false),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([(
        BranchId::new("main"),
        Arc::from([BranchId::new("alpha"), BranchId::new("beta")]),
    )]);
    Arc::new(snapshot)
}

fn visible_names(app: &App) -> Vec<&str> {
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
fn shifted_arrow_events_map_to_stack_navigation() {
    assert_eq!(
        Input::from_event(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
        Some(Input::press(Key::StackUp))
    );
    assert_eq!(
        Input::from_event(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
        Some(Input::press(Key::StackDown))
    );
}

#[test]
fn view_modes_toggle_without_losing_selection() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha-tip"));

    app.handle_key(Key::Character('t'));
    assert_eq!(app.order_mode, OrderMode::Graphite);
    app.handle_key(Key::Character('t'));
    assert_eq!(app.order_mode, OrderMode::Recent);
    assert_eq!(app.selected, Some(BranchId::new("alpha-tip")));

    app.handle_key(Key::Character('h'));
    assert!(matches!(app.scope, ViewScope::Stack { .. }));
    assert_eq!(visible_names(&app), ["beta", "alpha-tip", "alpha", "main"]);
    app.handle_key(Key::Character('h'));
    assert!(matches!(app.scope, ViewScope::All));

    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::Trunk { .. }));
    assert!(!visible_names(&app).contains(&"loose"));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::All));

    let with_separators = app.projection.entries.len();
    app.handle_key(Key::Character('s'));
    assert!(!app.separators);
    assert!(app.projection.entries.len() < with_separators);
    assert_eq!(app.selected, Some(BranchId::new("alpha-tip")));
}

#[test]
fn recent_is_default_and_order_picker_commits_or_rolls_back_atomically() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    assert_eq!(app.order_mode, OrderMode::Recent);

    app.order_mode = OrderMode::Alphabetical;
    app.handle_key(Key::Character('t'));
    assert_eq!(app.order_mode, OrderMode::Recent);

    app.handle_key(Key::Character('T'));
    assert!(matches!(app.overlay, Overlay::OrderPicker(_)));
    app.handle_key(Key::Down);
    assert!(matches!(
        app.overlay,
        Overlay::OrderPicker(ref picker) if picker.pending == OrderMode::Alphabetical
    ));
    assert_eq!(app.order_mode, OrderMode::Recent);
    app.handle_key(Key::Escape);
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.order_mode, OrderMode::Recent);

    app.handle_key(Key::Character('T'));
    app.handle_key(Key::Down);
    app.handle_key(Key::Enter);
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.order_mode, OrderMode::Alphabetical);
}

#[test]
fn lane_pitch_is_global_session_state_with_bounded_no_ops_and_auto_reset() {
    let mut app = App::default();
    assert_eq!(app.lane_pitch, LanePitch::Auto);
    app.handle_key(Key::Character('+'));
    assert_eq!(app.lane_pitch, LanePitch::Fixed(4));
    for _ in 0..10 {
        app.handle_key(Key::Character('+'));
    }
    assert_eq!(app.lane_pitch, LanePitch::Fixed(6));
    let boundary_message = app.message.clone();
    app.handle_key(Key::Character('+'));
    assert_eq!(app.message, boundary_message);

    app.handle_key(Key::Character('-'));
    assert_eq!(app.lane_pitch, LanePitch::Fixed(5));
    app.handle_key(Key::Character('0'));
    assert_eq!(app.lane_pitch, LanePitch::Auto);
    app.handle_key(Key::Character('0'));
    assert_eq!(app.lane_pitch, LanePitch::Auto);
}

#[test]
fn color_picker_previews_rolls_back_and_commits_one_write() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));

    app.handle_key(Key::Character('C'));
    assert!(matches!(app.overlay, Overlay::ColorPicker(_)));
    app.handle_key(Key::Down);
    assert_eq!(app.config.color(&BranchId::new("alpha")), Some("#7aa2f7"));
    app.handle_key(Key::Escape);
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.config.color(&BranchId::new("alpha")), None);

    app.handle_key(Key::Character('C'));
    app.handle_key(Key::Down);
    let Action::PersistColor(request) = app.handle_key(Key::Enter) else {
        panic!("color picker commit should persist exactly once");
    };
    assert_eq!(request.updates.len(), 1);
    assert_eq!(
        request.updates.get(&BranchId::new("alpha")),
        Some(&Some(Arc::from("#7aa2f7")))
    );
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(
        app.handle_key(Key::Enter),
        Action::Checkout(BranchId::new("alpha"))
    );
}

#[test]
fn color_picker_refuses_trunks_and_closes_if_its_target_disappears() {
    let initial = view_snapshot();
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("main"));
    app.handle_key(Key::Character('C'));
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.message.as_deref().unwrap().contains("trunk"));

    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('C'));
    app.handle_key(Key::Down);
    let mut changed = (*initial).clone();
    changed.generation = 2;
    changed.branches = Arc::from(
        changed
            .branches
            .iter()
            .filter(|branch| {
                branch.id != BranchId::new("alpha") && branch.id != BranchId::new("alpha-tip")
            })
            .cloned()
            .collect::<Vec<_>>(),
    );
    app.apply_snapshot(Arc::new(changed));
    assert_eq!(app.overlay, Overlay::None);
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("color picker closed")
    );
}

#[test]
fn geometric_stack_jumps_use_visible_heads() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("main"));
    app.handle_key(Key::Character('K'));
    assert_eq!(app.selected, Some(BranchId::new("beta")));
    app.handle_key(Key::Character('K'));
    assert_eq!(app.selected, Some(BranchId::new("beta")));
    app.handle_key(Key::Character('J'));
    assert_eq!(app.selected, Some(BranchId::new("loose")));
}

#[test]
fn stack_keys_jump_true_stacks_but_move_ten_rows_from_trunks_and_one_offs() {
    let mut branches = (0..25)
        .map(|index| {
            let name = format!("one-off-{index:02}");
            common::branch(&name, None, &name, false)
        })
        .collect::<Vec<_>>();
    branches.push(common::branch("stack", None, "stack", false));
    branches.push(common::branch("stack-tip", Some("stack"), "stack", true));
    branches.push(common::branch("other", None, "other", false));
    branches.push(common::branch("other-tip", Some("other"), "other", false));
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(branches));

    app.selected = Some(app.projection.selectable[12].clone());
    let start = app
        .projection
        .branch_to_selectable
        .get(app.selected.as_ref().unwrap())
        .copied()
        .unwrap();
    app.handle_key(Key::StackUp);
    assert_eq!(
        app.selected,
        Some(app.projection.selectable[start.saturating_sub(10)].clone())
    );
    app.handle_key(Key::StackDown);
    assert_eq!(app.selected, Some(app.projection.selectable[start].clone()));

    let true_heads = app
        .projection
        .stack_heads
        .iter()
        .filter(|head| app.projection.is_true_stack(&head.branch))
        .map(|head| head.branch.clone())
        .collect::<Vec<_>>();
    assert!(true_heads.len() >= 2);
    app.selected = true_heads.last().cloned();
    let current_row = app
        .projection
        .branch_to_visual
        .get(app.selected.as_ref().unwrap())
        .copied()
        .unwrap();
    let expected = app
        .projection
        .stack_heads
        .iter()
        .rev()
        .find(|head| head.visual_row < current_row)
        .unwrap()
        .branch
        .clone();
    app.handle_key(Key::StackUp);
    assert_eq!(app.selected, Some(expected));

    let current_head = true_heads.last().unwrap();
    let current_stack = app
        .projection
        .row_for(current_head)
        .unwrap()
        .stack_id
        .clone();
    let non_head = app
        .projection
        .rows
        .iter()
        .find(|row| row.stack_id == current_stack && row.branch != *current_head)
        .unwrap()
        .branch
        .clone();
    let non_head_row = app.projection.branch_to_visual[&non_head];
    let expected_other_stack = app
        .projection
        .stack_heads
        .iter()
        .rev()
        .find(|head| head.visual_row < non_head_row && head.stack_id != current_stack)
        .unwrap()
        .branch
        .clone();
    app.selected = Some(non_head);
    app.handle_key(Key::StackUp);
    assert_eq!(app.selected, Some(expected_other_stack));
}

#[test]
fn section_keys_jump_to_current_section_edges_without_crossing_sections() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::SectionUp);
    assert_eq!(app.selected, Some(BranchId::new("beta")));
    app.handle_key(Key::SectionDown);
    assert_eq!(app.selected, Some(BranchId::new("main")));

    app.selected = Some(BranchId::new("loose"));
    app.handle_key(Key::SectionUp);
    assert_eq!(app.selected, Some(BranchId::new("loose")));
    app.handle_key(Key::SectionDown);
    assert_eq!(app.selected, Some(BranchId::new("loose")));
}

#[test]
fn switching_focus_replaces_scope_and_same_scope_restores_all_view_position() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha-tip"));
    app.set_viewport_height(2);
    app.scroll = 1;

    app.handle_key(Key::Character('h'));
    assert!(matches!(app.scope, ViewScope::Stack { .. }));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::Trunk { .. }));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::All));
    assert_eq!(app.selected, Some(BranchId::new("alpha-tip")));
    assert_eq!(app.scroll, 1);
}

#[test]
fn trunk_focus_accepts_untrunked_and_stack_focus_keeps_dim_context() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('h'));
    assert_eq!(
        app.projection.emphasis_for(&BranchId::new("alpha")),
        stackmap::model::topology::Emphasis::Full
    );
    assert_eq!(
        app.projection.emphasis_for(&BranchId::new("beta")),
        stackmap::model::topology::Emphasis::Dim
    );
    assert_eq!(
        app.projection.emphasis_for(&BranchId::new("loose")),
        stackmap::model::topology::Emphasis::Hidden
    );

    app.handle_key(Key::Character('h'));
    app.selected = Some(BranchId::new("loose"));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::Untrunked));
    assert_eq!(
        app.projection
            .selectable
            .iter()
            .map(|branch| branch.0.as_ref())
            .collect::<Vec<_>>(),
        ["loose"]
    );
}

#[test]
fn untrunked_section_focus_includes_every_untrunked_component() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("loose-a", None, "loose-a", false),
        common::branch("loose-b", None, "loose-b", false),
    ]));
    app.selected = Some(BranchId::new("loose-a"));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::Untrunked));
    assert_eq!(
        app.projection
            .selectable
            .iter()
            .map(|branch| branch.0.as_ref())
            .collect::<Vec<_>>(),
        ["loose-a", "loose-b"]
    );
}

#[test]
fn current_startup_maps_branches_trunks_and_missing_current_safely() {
    let mut branch_app = App::default();
    let mut branch_snapshot = (*view_snapshot()).clone();
    for branch in Arc::make_mut(&mut branch_snapshot.branches) {
        branch.current = branch.id == BranchId::new("alpha-tip");
    }
    branch_app.request_current_startup();
    branch_app.apply_snapshot(Arc::new(branch_snapshot));
    assert!(matches!(branch_app.scope, ViewScope::Stack { .. }));

    let mut trunk_app = App::default();
    trunk_app.request_current_startup();
    trunk_app.apply_snapshot(view_snapshot());
    assert!(matches!(trunk_app.scope, ViewScope::Trunk { .. }));

    let mut missing_app = App::default();
    let mut missing = (*view_snapshot()).clone();
    for branch in Arc::make_mut(&mut missing.branches) {
        branch.current = false;
    }
    missing_app.request_current_startup();
    missing_app.apply_snapshot(Arc::new(missing));
    assert!(matches!(missing_app.scope, ViewScope::All));
    assert!(
        missing_app
            .message
            .as_deref()
            .unwrap()
            .contains("--current")
    );

    let mut detached_app = App::default();
    let mut detached = (*view_snapshot()).clone();
    detached.state = stackmap::model::RepositoryState::Detached;
    detached_app.request_current_startup();
    detached_app.apply_snapshot(Arc::new(detached));
    assert!(matches!(detached_app.scope, ViewScope::All));
    assert!(
        detached_app
            .message
            .as_deref()
            .unwrap()
            .contains("detached")
    );
}

#[test]
fn navigation_filtering_and_stack_jumps_preserve_context() {
    let snapshot = common::snapshot(vec![
        common::branch("alpha", None, "alpha", true),
        common::branch("alpha/one", Some("alpha"), "alpha", false),
        common::branch("alpha/two", Some("alpha"), "alpha", false),
        common::branch("beta", None, "beta", false),
        common::branch("beta/one", Some("beta"), "beta", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(snapshot);
    assert_eq!(app.selected, Some(BranchId::new("alpha")));
    app.handle_key(Key::Up);
    assert_eq!(app.selected, Some(BranchId::new("alpha/two")));
    app.handle_key(Key::Character('J'));
    assert_eq!(app.selected, Some(BranchId::new("beta")));

    app.handle_key(Key::Character('/'));
    app.handle_key(Key::Character('t'));
    app.handle_key(Key::Character('w'));
    app.handle_key(Key::Character('o'));
    assert_eq!(app.projection.rows.len(), 2);
    assert_eq!(app.selected, Some(BranchId::new("alpha/two")));
    app.handle_key(Key::Escape);
    assert!(app.filter.is_empty());
    assert_eq!(app.selected, Some(BranchId::new("beta")));
}

#[test]
fn checkout_uses_git_protection_and_never_discards_dirty_work() {
    let repository = common::init_repo();
    common::git(repository.path(), &["switch", "-c", "feature"]);
    fs::write(repository.path().join("file.txt"), "feature\n").unwrap();
    common::git(repository.path(), &["add", "file.txt"]);
    common::git(repository.path(), &["commit", "-m", "feature"]);
    common::git(repository.path(), &["switch", "main"]);
    fs::write(
        repository.path().join("file.txt"),
        "uncommitted main work\n",
    )
    .unwrap();
    let adapter = GitAdapter::discover(repository.path()).unwrap();

    let error = adapter.checkout(&BranchId::new("feature")).unwrap_err();
    assert!(error.to_string().contains("could not switch branch"));
    assert_eq!(
        common::git(repository.path(), &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("file.txt")).unwrap(),
        "uncommitted main work\n"
    );
}

#[test]
fn enter_emits_one_checkout_while_checkout_is_running() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    assert_eq!(
        app.handle_key(Key::Enter),
        Action::Checkout(BranchId::new("feature"))
    );
    assert_eq!(app.handle_key(Key::Enter), Action::None);
}

#[test]
fn checkout_is_disabled_for_current_branch_with_explanation() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(app.message.as_deref().unwrap().contains("already current"));
}

#[test]
fn ctrl_c_quits_while_search_is_active() {
    let mut app = App::default();
    app.overlay = Overlay::Search;
    assert_eq!(app.handle_key(Key::Quit), Action::Quit);
}

#[test]
fn checkout_refuses_branch_owned_by_linked_worktree() {
    let repository = common::init_repo();
    common::git(repository.path(), &["branch", "feature"]);
    let linked_parent = tempfile::tempdir().unwrap();
    let linked = linked_parent.path().join("linked");
    common::git(
        repository.path(),
        &["worktree", "add", linked.to_str().unwrap(), "feature"],
    );
    let adapter = GitAdapter::discover(repository.path()).unwrap();
    let error = adapter.checkout(&BranchId::new("feature")).unwrap_err();
    assert!(error.to_string().contains("checked out at"));
    common::git(
        repository.path(),
        &["worktree", "remove", linked.to_str().unwrap()],
    );
}

#[test]
fn deletion_requires_exact_blocking_confirmation() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("merged", None, "merged", false),
    ]));
    app.selected = Some(BranchId::new("merged"));
    assert_eq!(app.handle_key(Key::Character('x')), Action::None);
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
    assert_eq!(app.handle_key(Key::Character('n')), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));

    app.handle_key(Key::Character('x'));
    let action = app.handle_key(Key::Character('y'));
    assert!(matches!(action, Action::Delete(_)));
    assert!(matches!(app.mutation, MutationState::Deleting(_)));
}

#[test]
fn deletion_refuses_current_trunk_worktree_and_degraded_branches() {
    let mut current = common::branch("main", None, "main", true);
    current.trunk = Some(BranchId::new("main"));
    current.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut linked = common::branch("linked", None, "linked", false);
    linked.worktree = Some("/tmp/linked".into());
    let mut degraded = common::branch("degraded", None, "degraded", false);
    degraded.graphite = stackmap::model::GraphiteProvenance::Degraded;
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![current, linked, degraded]));

    for branch in ["main", "linked", "degraded"] {
        app.selected = Some(BranchId::new(branch));
        assert_eq!(app.handle_key(Key::Character('x')), Action::None);
        assert!(matches!(app.mutation, MutationState::Idle));
        assert!(app.message.is_some());
    }
}

fn with_generation(
    snapshot: &Arc<stackmap::model::RepositorySnapshot>,
    generation: u64,
) -> Arc<stackmap::model::RepositorySnapshot> {
    let mut snapshot = (**snapshot).clone();
    snapshot.generation = generation;
    Arc::new(snapshot)
}

#[test]
fn checkout_reconciliation_ignores_a_pre_mutation_refresh_that_arrives_late() {
    let snapshot = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(with_generation(&snapshot, 5));
    app.selected = Some(BranchId::new("feature"));
    assert!(matches!(app.handle_key(Key::Enter), Action::Checkout(_)));
    app.finish_checkout(Ok(()), 42);
    app.apply_structural_snapshot(with_generation(&snapshot, 6), 41);
    assert!(matches!(
        app.mutation,
        MutationState::Reconciling { request_epoch: 42 }
    ));
    app.apply_structural_snapshot(with_generation(&snapshot, 7), 42);
    assert!(matches!(app.mutation, MutationState::Idle));
}

#[test]
fn deletion_reconciliation_ignores_a_pre_mutation_refresh_that_arrives_late() {
    let snapshot = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("merged", None, "merged", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(with_generation(&snapshot, 8));
    app.selected = Some(BranchId::new("merged"));
    app.handle_key(Key::Character('x'));
    assert!(matches!(
        app.handle_key(Key::Character('y')),
        Action::Delete(_)
    ));
    app.finish_deletion(Ok(stackmap::adapters::git::DeleteOutcome::Unchanged), 18);
    app.apply_structural_snapshot(with_generation(&snapshot, 9), 17);
    assert!(matches!(
        app.mutation,
        MutationState::Reconciling { request_epoch: 18 }
    ));
    app.apply_structural_snapshot(with_generation(&snapshot, 10), 18);
    assert!(matches!(app.mutation, MutationState::Idle));
}

#[test]
fn inconsistent_provider_deletion_blocks_further_mutation() {
    let mut app = App::default();
    app.finish_deletion(Ok(stackmap::adapters::git::DeleteOutcome::Inconsistent), 18);
    assert!(matches!(app.mutation, MutationState::DeletionBlocked(_)));
    assert!(app.message.as_deref().unwrap().contains("inconsistent"));
}

#[test]
fn color_input_updates_memory_without_waiting_for_a_contended_disk_lock() {
    let directory = tempfile::tempdir().unwrap();
    let mut snapshot = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]))
    .clone();
    snapshot.common_dir = directory.path().to_owned();
    let lock = config_path(directory.path()).with_extension("lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, "held").unwrap();

    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.selected = Some(BranchId::new("feature"));
    let Action::PersistColor(request) = app.handle_key(Key::Character('c')) else {
        panic!("color persistence action");
    };
    assert_eq!(app.config.color(&BranchId::new("feature")), Some("#7aa2f7"));
    assert_eq!(request.sequence, 1);
    let current = app.snapshot.clone().unwrap();
    app.apply_snapshot(with_generation(&current, 2));
    assert_eq!(app.config.color(&BranchId::new("feature")), Some("#7aa2f7"));
}

#[test]
fn stale_color_results_cannot_overwrite_the_latest_choice_or_message() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    let Action::PersistColor(first) = app.handle_key(Key::Character('c')) else {
        panic!("first color action");
    };
    let Action::PersistColor(second) = app.handle_key(Key::Character('c')) else {
        panic!("second color action");
    };
    let latest_message = app.message.clone();
    app.finish_color_persistence(first.sequence, Err(anyhow::anyhow!("old failure")));
    assert_eq!(app.message, latest_message);
    assert_eq!(app.config.color(&BranchId::new("feature")), Some("#bb9af7"));
    let retry = app
        .prepare_pending_color_persistence(second.clone())
        .expect("failed and newer color remains pending");
    assert_eq!(
        retry.updates.get(&BranchId::new("feature")),
        Some(&Some(Arc::from("#bb9af7")))
    );
    app.finish_color_persistence(second.sequence, Ok(()));
    assert_eq!(app.message.as_deref(), Some("stack color saved"));
}

#[test]
fn completed_color_root_is_not_replayed_over_an_external_writer() {
    let directory = tempfile::tempdir().unwrap();
    let mut snapshot = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("alpha", None, "alpha", false),
        common::branch("beta", None, "beta", false),
    ]))
    .clone();
    snapshot.common_dir = directory.path().to_owned();

    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.selected = Some(BranchId::new("alpha"));
    let Action::PersistColor(first) = app.handle_key(Key::Character('c')) else {
        panic!("first color action");
    };
    app.selected = Some(BranchId::new("beta"));
    let Action::PersistColor(second) = app.handle_key(Key::Character('c')) else {
        panic!("second color action");
    };
    assert_eq!(second.updates.len(), 2);

    Config::persist_colors(&first.common_dir, &first.updates, &first.fallback).unwrap();
    app.finish_color_persistence(first.sequence, Ok(()));
    let mut external = Config::load(directory.path()).unwrap();
    external
        .set_color(directory.path(), &BranchId::new("alpha"), Some("#f7768e"))
        .unwrap();

    let filtered = app.prepare_pending_color_persistence(second).unwrap();
    assert_eq!(
        filtered.updates.keys().collect::<Vec<_>>(),
        vec![&BranchId::new("beta")]
    );
    Config::persist_colors(&filtered.common_dir, &filtered.updates, &filtered.fallback).unwrap();
    app.finish_color_persistence(filtered.sequence, Ok(()));

    let saved = Config::load(directory.path()).unwrap();
    assert_eq!(saved.color(&BranchId::new("alpha")), Some("#f7768e"));
    assert_eq!(saved.color(&BranchId::new("beta")), Some("#7aa2f7"));
}

#[test]
fn trunk_scope_survives_when_a_selected_branch_leaves_the_still_valid_section() {
    let initial = view_snapshot();
    let mut app = App::default();
    app.apply_snapshot(initial.clone());
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('H'));
    assert!(matches!(app.scope, ViewScope::Trunk { .. }));

    let mut changed = (*initial).clone();
    changed.generation = 2;
    let branches = Arc::make_mut(&mut changed.branches);
    let alpha = branches
        .iter_mut()
        .find(|branch| branch.id == BranchId::new("alpha"))
        .unwrap();
    alpha.trunk = None;
    alpha.graphite = stackmap::model::GraphiteProvenance::DefinitelyUntracked;
    app.apply_snapshot(Arc::new(changed));
    assert!(matches!(app.scope, ViewScope::Trunk { .. }));
}

#[test]
fn option_shaped_untracked_branch_is_deletion_eligible() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("--force", None, "--force", false),
    ]));
    app.selected = Some(BranchId::new("--force"));
    assert_eq!(app.handle_key(Key::Character('x')), Action::None);
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
}
