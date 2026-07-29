use super::common;

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapters::git::GitAdapter;
use crate::app::{
    Action, App, LanePitch, MutationState, Overlay, ReconciliationOperation, ViewScope,
};
use crate::config::{ArchiveMutation, Config, ConfigMutation, VisualSection, config_path};
use crate::events::{Input, Key};
use crate::model::BranchId;
use crate::model::topology::{OrderMode, ProjectionEntry};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    branch.graphite = crate::model::GraphiteProvenance::Tracked;
    branch.committed_at = committed_at;
    branch
}

fn view_snapshot() -> Arc<crate::model::RepositorySnapshot> {
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
    let Action::PersistConfig(request) = app.handle_key(Key::Enter) else {
        panic!("color picker commit should persist exactly once");
    };
    assert_eq!(request.mutation.color_updates.len(), 1);
    assert_eq!(
        request.mutation.color_updates.get(&BranchId::new("alpha")),
        Some(&Some(Arc::from("#7aa2f7")))
    );
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.handle_key(Key::Enter), Action::None);
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
fn stack_name_editor_prefills_saves_clears_cancels_and_refuses_trunks() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));

    app.handle_key(Key::Character('n'));
    assert!(matches!(app.overlay, Overlay::StackNameEditor(_)));
    for character in "Release train".chars() {
        app.handle_key(Key::Character(character));
    }
    let Action::PersistConfig(request) = app.handle_key(Key::Enter) else {
        panic!("saving a stack name should persist once");
    };
    assert_eq!(
        request
            .mutation
            .stack_name_updates
            .get(&BranchId::new("alpha")),
        Some(&Some(Arc::from("Release train")))
    );
    assert!(app.projection.entries.iter().any(|entry| {
        matches!(entry, ProjectionEntry::StackLabel(label) if label.text.as_ref() == "Release train")
    }));

    app.handle_key(Key::Character('n'));
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.message.as_deref().unwrap().contains("Enter"));
    app.handle_key(Key::Enter);
    let Overlay::StackNameEditor(editor) = &app.overlay else {
        panic!("Enter on the selected label should reopen the editor");
    };
    assert_eq!(editor.draft, "Release train");
    app.handle_key(Key::Escape);
    assert_eq!(
        app.config.stack_name(&BranchId::new("alpha")),
        Some("Release train")
    );

    app.handle_key(Key::Enter);
    for _ in 0.."Release train".chars().count() {
        app.handle_key(Key::Backspace);
    }
    let Action::PersistConfig(request) = app.handle_key(Key::Enter) else {
        panic!("clearing a stack name should persist once");
    };
    assert_eq!(
        request
            .mutation
            .stack_name_updates
            .get(&BranchId::new("alpha")),
        Some(&None)
    );
    assert_eq!(app.config.stack_name(&BranchId::new("alpha")), None);

    app.selected = Some(BranchId::new("main"));
    app.handle_key(Key::Character('n'));
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.message.as_deref().unwrap().contains("trunk"));
}

#[test]
fn stack_name_editor_supports_cursor_insertion_and_forward_delete() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('n'));
    app.handle_key(Key::Character('a'));
    app.handle_key(Key::Character('c'));
    app.handle_key(Key::Left);
    app.handle_key(Key::Character('b'));
    let Overlay::StackNameEditor(editor) = &app.overlay else {
        panic!("name editor");
    };
    assert_eq!(editor.draft, "abc");
    assert_eq!(editor.cursor, 2);

    app.handle_key(Key::Home);
    app.handle_key(Key::Right);
    app.handle_key(Key::Delete);
    app.handle_key(Key::End);
    app.handle_key(Key::Backspace);
    let Overlay::StackNameEditor(editor) = &app.overlay else {
        panic!("name editor");
    };
    assert_eq!(editor.draft, "a");
    assert_eq!(editor.cursor, 1);
}

#[test]
fn visual_section_toggle_names_inline_and_keeps_git_actions_off_labels() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));

    let Action::PersistConfig(create) = app.handle_key(Key::Character('i')) else {
        panic!("creating a visual section should persist");
    };
    let section = create.mutation.visual_section_updates[&BranchId::new("alpha")]
        .as_ref()
        .unwrap();
    assert!(section.name.is_none());
    assert!(
        app.projection
            .row_for(&BranchId::new("alpha"))
            .unwrap()
            .manual_depth
            > 0
    );

    app.handle_key(Key::Character('n'));
    for character in "jkgGJK section".chars() {
        app.handle_key(Key::Character(character));
    }
    assert!(matches!(app.handle_key(Key::Quit), Action::None));
    let Action::PersistConfig(named) = app.handle_key(Key::Enter) else {
        panic!("section name should persist");
    };
    assert_eq!(
        named.mutation.visual_section_updates[&BranchId::new("alpha")]
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("jkgGJK section")
    );
    assert!(
        app.selected_branch().is_none(),
        "a selected label must not masquerade as a branch"
    );
    assert!(matches!(app.handle_key(Key::Enter), Action::None));
    app.handle_key(Key::Escape);

    app.selected_label = None;
    app.selected = Some(BranchId::new("alpha"));
    let Action::PersistConfig(remove) = app.handle_key(Key::Character('i')) else {
        panic!("removing a visual section should persist");
    };
    assert_eq!(
        remove.mutation.visual_section_updates[&BranchId::new("alpha")],
        None
    );
}

#[test]
fn section_recolor_avoids_effective_neighbors_after_legacy_color_conflicts() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    for anchor in ["alpha", "alpha-tip"] {
        app.config
            .set_visual_section_in_memory(
                &BranchId::new(anchor),
                Some(VisualSection {
                    color: "#7aa2f7".into(),
                    name: None,
                }),
            )
            .unwrap();
    }
    app.handle_key(Key::Character('t'));
    let lower_effective = app
        .projection
        .row_for(&BranchId::new("alpha"))
        .unwrap()
        .visual_color
        .clone()
        .unwrap();
    let upper_effective = app
        .projection
        .row_for(&BranchId::new("alpha-tip"))
        .unwrap()
        .visual_color
        .clone()
        .unwrap();
    assert_ne!(lower_effective, upper_effective);

    app.selected = Some(BranchId::new("alpha-tip"));
    let Action::PersistConfig(request) = app.handle_key(Key::Character('c')) else {
        panic!("recolor should remain available after conflict resolution");
    };
    let saved = request.mutation.visual_section_updates[&BranchId::new("alpha-tip")]
        .as_ref()
        .unwrap();
    assert_ne!(saved.color, lower_effective.as_ref());
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
fn stack_down_from_lowest_stack_lands_on_its_trunk() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    let lowest_stack = app
        .projection
        .stack_heads
        .iter()
        .filter(|head| app.projection.is_true_stack(&head.branch))
        .max_by_key(|head| head.visual_row)
        .unwrap()
        .branch
        .clone();
    app.selected = Some(lowest_stack);

    app.handle_key(Key::StackDown);

    assert_eq!(app.selected, Some(BranchId::new("main")));
}

#[test]
fn stack_down_to_sticky_trunk_scrolls_focused_stacks_to_the_bottom() {
    let mut app = App::default();
    app.apply_snapshot(view_snapshot());
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(Key::Character('H'));
    app.set_viewport_height(3);

    let sticky = app
        .sticky_visual_row()
        .expect("focused trunk should be sticky");
    let start = app.focused_section_bounds().expect("focused section").0;
    let lowest_stack = app
        .projection
        .stack_heads
        .iter()
        .filter(|head| app.projection.is_true_stack(&head.branch))
        .max_by_key(|head| head.visual_row)
        .expect("true stack")
        .branch
        .clone();
    app.selected = Some(lowest_stack);
    app.scroll = start;

    app.handle_key(Key::StackDown);

    assert_eq!(app.selected, Some(BranchId::new("main")));
    assert_eq!(
        app.scroll,
        sticky.saturating_sub(app.viewport_height).max(start)
    );
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
        crate::model::topology::Emphasis::Full
    );
    assert_eq!(
        app.projection.emphasis_for(&BranchId::new("beta")),
        crate::model::topology::Emphasis::Dim
    );
    assert_eq!(
        app.projection.emphasis_for(&BranchId::new("loose")),
        crate::model::topology::Emphasis::Hidden
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
    detached.state = crate::model::RepositoryState::Detached;
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
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(matches!(
        app.mutation,
        MutationState::ConfirmingCheckout(ref target) if target == &BranchId::new("feature")
    ));
    assert_eq!(
        app.handle_key(Key::Enter),
        Action::Checkout(BranchId::new("feature"))
    );
    assert_eq!(app.handle_key(Key::Enter), Action::None);
}

#[test]
fn checkout_confirmation_cancels_on_escape_or_navigation() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));

    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert_eq!(app.handle_key(Key::Escape), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));

    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert_eq!(app.handle_key(Key::Up), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(app.selected, Some(BranchId::new("main")));
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
    app.begin_delete_confirmation();
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
    assert_eq!(app.handle_key(Key::Character('o')), Action::None);
    assert_eq!(app.handle_key(Key::Character('n')), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));

    app.begin_delete_confirmation();
    let action = app.handle_key(Key::Character('y'));
    assert!(matches!(action, Action::Delete(_)));
    assert!(matches!(app.mutation, MutationState::Deleting(_)));
}

#[test]
fn lowercase_x_is_only_reversible_and_uppercase_x_requires_press_confirmation_in_both_modes() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("merged", None, "merged", false),
    ]));
    app.selected = Some(BranchId::new("merged"));

    let active_archive = app.handle_input(Input::press(Key::Character('x')));
    assert!(matches!(active_archive, Action::PersistConfig(_)));
    assert!(!matches!(active_archive, Action::Delete(_)));
    assert!(matches!(app.mutation, MutationState::Idle));

    app.handle_key(Key::Character('a'));
    app.selected = Some(BranchId::new("merged"));
    let archive_restore = app.handle_input(Input::press(Key::Character('x')));
    assert!(matches!(archive_restore, Action::PersistConfig(_)));
    assert!(!matches!(archive_restore, Action::Delete(_)));
    assert!(matches!(app.mutation, MutationState::Idle));

    app.handle_key(Key::Character('a'));
    app.selected = Some(BranchId::new("merged"));
    assert!(matches!(
        app.handle_key(Key::Character('x')),
        Action::PersistConfig(_)
    ));
    app.handle_key(Key::Character('a'));
    app.selected = Some(BranchId::new("merged"));
    assert_eq!(
        app.handle_input(Input::repeat(Key::Character('X'))),
        Action::None
    );
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(
        app.handle_input(Input::press(Key::Character('X'))),
        Action::None
    );
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
    assert_eq!(app.handle_key(Key::Character('n')), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));

    app.handle_key(Key::Character('a'));
    app.selected = Some(BranchId::new("merged"));
    assert_eq!(app.handle_key(Key::Character('X')), Action::None);
    let active_delete = app.handle_key(Key::Character('y'));
    assert!(matches!(active_delete, Action::Delete(_)));
}

#[test]
fn uppercase_delete_preserves_current_trunk_worktree_degraded_and_nonleaf_refusals() {
    let current = common::branch("current", None, "current", true);
    let mut trunk = common::branch("main", None, "main", false);
    trunk.trunk = Some(BranchId::new("main"));
    trunk.graphite = crate::model::GraphiteProvenance::Tracked;
    let mut linked = common::branch("linked", None, "linked", false);
    linked.worktree = Some("/tmp/linked".into());
    let mut degraded = common::branch("degraded", None, "degraded", false);
    degraded.graphite = crate::model::GraphiteProvenance::Degraded;
    let mut parent = common::branch("parent", None, "parent", false);
    parent.graphite = crate::model::GraphiteProvenance::Tracked;
    let mut child = common::branch("child", Some("parent"), "parent", false);
    child.graphite = crate::model::GraphiteProvenance::Tracked;
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        current, trunk, linked, degraded, parent, child,
    ]));

    for branch in ["current", "main", "linked", "degraded", "parent"] {
        app.selected = Some(BranchId::new(branch));
        assert_eq!(app.handle_key(Key::Character('X')), Action::None);
        assert!(matches!(app.mutation, MutationState::Idle));
        assert!(app.message.is_some());
    }

    app.selected = Some(BranchId::new("child"));
    app.mark_stale(Arc::from("inventory refresh failed"));
    assert_eq!(app.handle_key(Key::Character('X')), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert!(app.message.as_deref().unwrap().contains("stale"));
    assert!(app.message.as_deref().unwrap().contains("press r"));

    app.apply_snapshot(app.snapshot.clone().unwrap());
    app.selected = Some(BranchId::new("child"));
    assert_eq!(app.handle_key(Key::Character('X')), Action::None);
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
    app.mark_stale(Arc::from("refresh failed during confirmation"));
    assert_eq!(app.handle_key(Key::Character('y')), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert!(app.message.as_deref().unwrap().contains("stale"));
}

fn with_generation(
    snapshot: &Arc<crate::model::RepositorySnapshot>,
    generation: u64,
) -> Arc<crate::model::RepositorySnapshot> {
    let mut snapshot = (**snapshot).clone();
    snapshot.generation = generation;
    Arc::new(snapshot)
}

fn deletion_cleanup_fixture() -> (
    tempfile::TempDir,
    App,
    Arc<crate::model::RepositorySnapshot>,
    Option<BranchId>,
) {
    let directory = tempfile::tempdir().unwrap();
    let target = BranchId::new("delete-me");
    let neighbor = BranchId::new("keep-me");
    let mut seed = ConfigMutation::default();
    seed.set_archived(target.clone(), true);
    seed.set_archived(neighbor.clone(), true);
    seed.set_archived(BranchId::new("other"), true);
    seed.set_color(target.clone(), Some(Arc::from("#7aa2f7")));
    seed.set_color(neighbor.clone(), Some(Arc::from("#bb9af7")));
    Config::persist_mutation(directory.path(), &seed, &Config::default()).unwrap();

    let mut snapshot = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("delete-me", None, "delete-me", false),
        common::branch("keep-me", None, "keep-me", false),
        common::branch("other", None, "other", false),
    ]))
    .clone();
    snapshot.generation = 20;
    snapshot.common_dir = directory.path().to_owned();
    let snapshot = Arc::new(snapshot);

    let mut app = App::default();
    app.apply_snapshot(snapshot.clone());
    app.handle_key(Key::Character('a'));
    app.selected = Some(target.clone());
    let target_index = app.projection.branch_to_selectable[&target];
    let nearby = app
        .projection
        .selectable
        .get(target_index + 1)
        .or_else(|| {
            target_index
                .checked_sub(1)
                .and_then(|index| app.projection.selectable.get(index))
        })
        .cloned();
    assert_eq!(app.handle_key(Key::Character('X')), Action::None);
    assert!(matches!(
        app.handle_key(Key::Character('y')),
        Action::Delete(_)
    ));
    (directory, app, snapshot, nearby)
}

fn deletion_snapshot_without(
    snapshot: &Arc<crate::model::RepositorySnapshot>,
    target: &BranchId,
    generation: u64,
) -> Arc<crate::model::RepositorySnapshot> {
    let mut changed = (**snapshot).clone();
    changed.generation = generation;
    let branches = changed
        .branches
        .iter()
        .filter(|branch| &branch.id != target)
        .cloned()
        .collect::<Vec<_>>();
    changed.branch_index = crate::model::RepositorySnapshot::index_branches(&branches);
    changed.branches = Arc::from(branches);
    Arc::new(changed)
}

#[test]
fn checkout_reconciliation_ignores_a_pre_mutation_refresh_that_arrives_late() {
    let snapshot = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]);
    let now = Instant::now();
    let mut app = App::with_mutation_timing(Duration::from_secs(10), Duration::from_secs(2));
    app.apply_snapshot(with_generation(&snapshot, 5));
    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(matches!(app.handle_key(Key::Enter), Action::Checkout(_)));
    app.finish_checkout_at(Ok(()), 42, now);
    app.apply_structural_snapshot_at(with_generation(&snapshot, 6), 41, now);
    assert!(matches!(
        app.mutation,
        MutationState::Reconciling {
            operation: ReconciliationOperation::Checkout { ref target, .. },
            request_epoch: 42,
            ..
        } if target == &BranchId::new("feature")
    ));

    let matching = common::snapshot(vec![
        common::branch("main", None, "main", false),
        common::branch("feature", None, "feature", true),
    ]);
    app.apply_structural_snapshot_at(with_generation(&matching, 7), 42, now);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(
        app.message, None,
        "successful reconciliation must not leave the footer stuck on progress"
    );
    assert_eq!(app.notice(), Some("checked out feature"));
    assert!(!app.tick(now + Duration::from_secs(1)));
    assert!(app.tick(now + Duration::from_secs(2)));
    assert_eq!(app.notice(), None);
}

#[test]
fn causal_checkout_mismatch_unlocks_with_targeted_refresh_guidance() {
    let now = Instant::now();
    let snapshot = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]);
    let mut app = App::with_mutation_timing(Duration::from_secs(10), Duration::from_secs(5));
    app.apply_snapshot(with_generation(&snapshot, 5));
    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert_eq!(
        app.handle_key(Key::Enter),
        Action::Checkout(BranchId::new("feature"))
    );
    app.finish_checkout_at(Ok(()), 42, now);

    app.selected = Some(BranchId::new("main"));
    app.apply_structural_snapshot_at(with_generation(&snapshot, 6), 42, now);

    assert!(matches!(app.mutation, MutationState::Idle));
    let notice = app.notice().expect("bounded mismatch notice");
    assert!(notice.contains("feature"));
    assert!(notice.contains("main"));
    assert!(notice.contains("press r"));
}

#[test]
fn checkout_reconciliation_deadline_unlocks_and_expires_deterministically() {
    let now = Instant::now();
    let snapshot = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]);
    let mut app = App::with_mutation_timing(Duration::from_secs(3), Duration::from_secs(2));
    app.apply_snapshot(snapshot);
    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(matches!(app.handle_key(Key::Enter), Action::Checkout(_)));
    app.finish_checkout_at(Ok(()), 9, now);
    app.mark_stale(Arc::from("refresh failed"));

    assert!(!app.tick(now + Duration::from_secs(2)));
    assert!(matches!(app.mutation, MutationState::Reconciling { .. }));
    assert!(app.tick(now + Duration::from_secs(3)));
    assert!(matches!(app.mutation, MutationState::Idle));
    assert!(app.notice().unwrap().contains("press r"));
    assert_eq!(app.refresh_error.as_deref(), Some("refresh failed"));
    assert!(!app.tick(now + Duration::from_secs(4)));
    assert!(app.tick(now + Duration::from_secs(5)));
    assert_eq!(app.notice(), None);
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
    app.begin_delete_confirmation();
    assert!(matches!(
        app.handle_key(Key::Character('y')),
        Action::Delete(_)
    ));
    app.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Unchanged), 18);
    app.apply_structural_snapshot(with_generation(&snapshot, 9), 17);
    assert!(matches!(
        app.mutation,
        MutationState::Reconciling {
            operation: ReconciliationOperation::Deletion { ref request, .. },
            request_epoch: 18,
            ..
        } if request.branch == BranchId::new("merged") && request.expected_oid.as_ref() == "oid-merged"
    ));
    app.apply_structural_snapshot(with_generation(&snapshot, 10), 18);
    assert!(matches!(app.mutation, MutationState::Idle));
}

#[test]
fn successful_deletion_cleans_exact_config_identity_only_after_authoritative_absence() {
    let (directory, mut app, snapshot, nearby) = deletion_cleanup_fixture();
    let target = BranchId::new("delete-me");
    app.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Deleted), 50);

    app.apply_structural_snapshot(deletion_snapshot_without(&snapshot, &target, 21), 49);
    assert!(app.config.is_archived(&target));
    assert_eq!(app.config.color(&target), Some("#7aa2f7"));
    assert!(app.take_config_write_request().is_none());
    assert!(matches!(app.mutation, MutationState::Reconciling { .. }));

    app.apply_structural_snapshot(deletion_snapshot_without(&snapshot, &target, 22), 50);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert!(!app.config.is_archived(&target));
    assert_eq!(app.config.color(&target), None);
    assert!(app.config.is_archived(&BranchId::new("keep-me")));
    assert_eq!(app.config.color(&BranchId::new("keep-me")), Some("#bb9af7"));
    assert_eq!(app.selected, nearby);
    let cleanup = app
        .take_config_write_request()
        .expect("verified deletion should enter the config outbox");
    assert_eq!(
        cleanup.mutation.archive_updates.get(&target),
        Some(&ArchiveMutation::Prune)
    );
    assert_eq!(cleanup.mutation.color_updates.get(&target), Some(&None));
    Config::persist_mutation(&cleanup.common_dir, &cleanup.mutation, &cleanup.fallback).unwrap();
    app.finish_config_persistence(cleanup.sequence, Ok(()));
    let saved = Config::load(directory.path()).unwrap();
    assert!(!saved.is_archived(&target));
    assert_eq!(saved.color(&target), None);
    assert!(saved.is_archived(&BranchId::new("keep-me")));
    assert_eq!(saved.color(&BranchId::new("keep-me")), Some("#bb9af7"));
}

#[test]
fn deletion_does_not_clean_config_on_unchanged_error_or_same_name_new_oid() {
    let target = BranchId::new("delete-me");

    let (_directory, mut unchanged, snapshot, _) = deletion_cleanup_fixture();
    unchanged.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Unchanged), 60);
    unchanged.apply_structural_snapshot(deletion_snapshot_without(&snapshot, &target, 21), 60);
    assert!(unchanged.config.is_archived(&target));
    assert_eq!(unchanged.config.color(&target), Some("#7aa2f7"));
    assert!(unchanged.take_config_write_request().is_none());

    let (_directory, mut failed, snapshot, _) = deletion_cleanup_fixture();
    failed.finish_deletion(Err(anyhow::anyhow!("provider refused")), 70);
    failed.apply_structural_snapshot(deletion_snapshot_without(&snapshot, &target, 21), 70);
    assert!(failed.config.is_archived(&target));
    assert_eq!(failed.config.color(&target), Some("#7aa2f7"));
    assert!(failed.take_config_write_request().is_none());

    let (_directory, mut replaced, snapshot, _) = deletion_cleanup_fixture();
    replaced.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Deleted), 80);
    let mut replacement = (*snapshot).clone();
    replacement.generation = 21;
    let branches = Arc::make_mut(&mut replacement.branches);
    branches
        .iter_mut()
        .find(|branch| branch.id == target)
        .unwrap()
        .oid = Arc::from("new-oid");
    replaced.apply_structural_snapshot(Arc::new(replacement), 80);
    assert!(replaced.config.is_archived(&target));
    assert_eq!(replaced.config.color(&target), Some("#7aa2f7"));
    assert!(replaced.take_config_write_request().is_none());
    assert!(replaced.notice().unwrap().contains("changed"));
}

#[test]
fn deletion_reconciliation_preserves_user_navigation_away_from_the_target() {
    let (_directory, mut app, snapshot, nearby) = deletion_cleanup_fixture();
    let target = BranchId::new("delete-me");
    let user_choice = app
        .projection
        .selectable
        .iter()
        .find(|branch| **branch != target && Some((*branch).clone()) != nearby)
        .cloned()
        .expect("fixture has a non-nearby branch");
    let target_index = app.projection.branch_to_selectable[&target];
    let choice_index = app.projection.branch_to_selectable[&user_choice];
    let key = if choice_index < target_index {
        Key::Up
    } else {
        Key::Down
    };
    for _ in 0..target_index.abs_diff(choice_index) {
        app.handle_key(key.clone());
    }
    assert_eq!(app.selected, Some(user_choice.clone()));

    app.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Deleted), 90);
    app.apply_structural_snapshot(deletion_snapshot_without(&snapshot, &target, 21), 90);

    assert_eq!(app.selected, Some(user_choice));
    assert!(matches!(app.mutation, MutationState::Idle));
}

#[test]
fn inconsistent_provider_deletion_blocks_further_mutation() {
    let mut app = App::default();
    app.finish_deletion(Ok(crate::adapters::git::DeleteOutcome::Inconsistent), 18);
    assert!(matches!(app.mutation, MutationState::DeletionBlocked(_)));
    assert!(app.message.as_deref().unwrap().contains("inconsistent"));
    assert!(!app.tick(Instant::now() + Duration::from_secs(60)));
    assert!(matches!(app.mutation, MutationState::DeletionBlocked(_)));
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
    let Action::PersistConfig(request) = app.handle_key(Key::Character('c')) else {
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
    let Action::PersistConfig(first) = app.handle_key(Key::Character('c')) else {
        panic!("first color action");
    };
    let Action::PersistConfig(second) = app.handle_key(Key::Character('c')) else {
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
        retry.mutation.color_updates.get(&BranchId::new("feature")),
        Some(&Some(Arc::from("#bb9af7")))
    );
    app.finish_color_persistence(second.sequence, Ok(()));
    assert_eq!(app.message.as_deref(), Some("configuration saved"));
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
    let Action::PersistConfig(first) = app.handle_key(Key::Character('c')) else {
        panic!("first color action");
    };
    app.selected = Some(BranchId::new("beta"));
    let Action::PersistConfig(second) = app.handle_key(Key::Character('c')) else {
        panic!("second color action");
    };
    assert_eq!(second.mutation.color_updates.len(), 2);

    Config::persist_mutation(&first.common_dir, &first.mutation, &first.fallback).unwrap();
    app.finish_color_persistence(first.sequence, Ok(()));
    let mut external = Config::load(directory.path()).unwrap();
    external
        .set_color(directory.path(), &BranchId::new("alpha"), Some("#f7768e"))
        .unwrap();

    let filtered = app.prepare_pending_color_persistence(second).unwrap();
    assert_eq!(
        filtered.mutation.color_updates.keys().collect::<Vec<_>>(),
        vec![&BranchId::new("beta")]
    );
    Config::persist_mutation(&filtered.common_dir, &filtered.mutation, &filtered.fallback).unwrap();
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
    alpha.graphite = crate::model::GraphiteProvenance::DefinitelyUntracked;
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
    app.begin_delete_confirmation();
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
}
