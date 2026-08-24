use super::common;

use std::sync::Arc;

use crate::app::{Action, App, MutationState, Overlay};
use crate::config::VisualSection;
use crate::events::{Input, Key, KeyPhase};
use crate::model::BranchId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

fn event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent::new_with_kind(code, modifiers, kind)
}

#[test]
fn key_events_preserve_phase_and_normalize_portable_navigation_fallbacks() {
    assert_eq!(
        Input::from_event(event(KeyCode::Up, KeyModifiers::SHIFT, KeyEventKind::Press)),
        Some(Input::press(Key::StackUp))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Down,
            KeyModifiers::SUPER,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::StackDown))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Down,
            KeyModifiers::ALT,
            KeyEventKind::Repeat
        )),
        Some(Input::repeat(Key::SectionDown))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Left,
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Left))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Delete,
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Delete))
    );
    assert_eq!(
        Input::from_event(event(KeyCode::Tab, KeyModifiers::NONE, KeyEventKind::Press)),
        Some(Input::press(Key::Tab))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('K'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('K')))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('J'),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('J')))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('g')))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('G')))
    );
}

#[test]
fn command_copy_chords_do_not_fall_through_to_plain_color_keys() {
    for (code, modifiers, copy_key) in [
        (KeyCode::Char('c'), KeyModifiers::SUPER, Key::CopyBranch),
        (
            KeyCode::Char('C'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
            Key::CopySection,
        ),
        (
            KeyCode::Char('C'),
            KeyModifiers::SUPER | KeyModifiers::ALT | KeyModifiers::SHIFT,
            Key::CopyStack,
        ),
    ] {
        assert_eq!(
            Input::from_event(event(code, modifiers, KeyEventKind::Press)),
            Some(Input::press(copy_key))
        );
    }

    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('c')))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('C'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Character('C')))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('c'),
            KeyModifiers::SUPER | KeyModifiers::ALT | KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::CopyStack))
    );
}

#[test]
fn name_clear_chords_are_normalized_before_printable_text() {
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        )),
        Some(Input::press(Key::ClearNameDraft))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Backspace,
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        )),
        Some(Input::press(Key::ClearNameDraft))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Backspace,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Some(Input::press(Key::Backspace))
    );
}

#[test]
fn ctrl_u_and_shift_backspace_clear_editor_text_instead_of_inserting_u() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature", None, "feature", false,
    )]));
    app.handle_key(Key::Character('n'));
    for character in "é🙂draft".chars() {
        app.handle_key(Key::Character(character));
    }
    let ctrl_u = Input::from_event(event(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    ))
    .unwrap();
    app.handle_input(ctrl_u);
    let Overlay::StackNameEditor(editor) = &app.overlay else {
        panic!("name editor");
    };
    assert_eq!((editor.draft.as_str(), editor.cursor), ("", 0));

    app.handle_key(Key::Character('x'));
    let shifted_backspace = Input::from_event(event(
        KeyCode::Backspace,
        KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ))
    .unwrap();
    app.handle_input(shifted_backspace);
    let Overlay::StackNameEditor(editor) = &app.overlay else {
        panic!("name editor");
    };
    assert_eq!((editor.draft.as_str(), editor.cursor), ("", 0));
}

fn copy_fixture() -> App {
    let mut snapshot = (*common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("base", None, "base", false),
        common::branch("middle", Some("base"), "base", false),
        common::branch("tip", Some("middle"), "base", false),
        common::branch("side", Some("middle"), "side", false),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("base"), Arc::from([BranchId::new("middle")])),
        (
            BranchId::new("middle"),
            Arc::from([BranchId::new("tip"), BranchId::new("side")]),
        ),
    ]);
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    for anchor in ["base", "middle"] {
        app.config
            .set_visual_section_in_memory(
                &BranchId::new(anchor),
                Some(VisualSection {
                    color: "#7aa2f7".into(),
                    name: Some(anchor.into()),
                }),
            )
            .unwrap();
    }
    app.handle_key(Key::Character('t'));
    app.selected = Some(BranchId::new("tip"));
    app.selected_label = None;
    app
}

fn copied(action: Action) -> crate::app::ClipboardRequest {
    let Action::Copy(request) = action else {
        panic!("expected a clipboard action");
    };
    request
}

#[test]
fn semantic_copy_scopes_use_complete_real_topology() {
    let mut app = copy_fixture();

    let branch = copied(app.handle_key(Key::CopyBranch));
    assert_eq!(branch.text.as_ref(), "tip");
    assert_eq!(branch.branch_count, 1);

    let section = copied(app.handle_key(Key::CopySection));
    assert_eq!(section.text.as_ref(), "middle\ntip");
    assert_eq!(section.branch_count, 2);

    let stack = copied(app.handle_key(Key::CopyStack));
    assert_eq!(stack.text.as_ref(), "base\nmiddle\ntip");
    assert_eq!(stack.branch_count, 3);
    assert!(!stack.text.contains("main"));
    assert!(!stack.text.contains("side"));
}

#[test]
fn semantic_copy_is_projection_invariant_and_captures_at_keypress() {
    let mut app = copy_fixture();
    let before = copied(app.handle_key(Key::CopyStack));

    app.handle_key(Key::Character('/'));
    for character in "tip".chars() {
        app.handle_key(Key::Character(character));
    }
    app.handle_key(Key::Enter);
    app.selected = Some(BranchId::new("tip"));
    let filtered = copied(app.handle_key(Key::CopyStack));
    assert_eq!(filtered, before);

    let mut changed = (*app.snapshot.as_ref().unwrap().as_ref()).clone();
    changed.branches = changed
        .branches
        .iter()
        .filter(|branch| branch.id != BranchId::new("middle"))
        .cloned()
        .collect::<Vec<_>>()
        .into();
    changed.branch_index = crate::model::RepositorySnapshot::index_branches(&changed.branches);
    app.apply_snapshot(Arc::new(changed));
    assert_eq!(before.text.as_ref(), "base\nmiddle\ntip");
}

#[test]
fn semantic_labels_copy_their_identity_and_context_rows_are_ineligible() {
    let mut app = copy_fixture();
    app.selected_label = Some(crate::app::ConfigTarget::VisualSection(BranchId::new(
        "base",
    )));
    assert_eq!(
        copied(app.handle_key(Key::CopyBranch)).text.as_ref(),
        "base"
    );
    assert_eq!(
        copied(app.handle_key(Key::CopySection)).text.as_ref(),
        "base\nmiddle\ntip"
    );

    app.selected_label = None;
    app.selected = Some(BranchId::new("tip"));
    app.handle_key(Key::Character('h'));
    app.selected = Some(BranchId::new("main"));
    assert_eq!(app.handle_key(Key::CopyBranch), Action::None);
}

#[test]
fn ctrl_c_quits_unknown_keys_are_ignored_and_release_is_dropped() {
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        )),
        Some(Input::press(Key::Quit))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::F(12),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::Ignored))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Release
        )),
        None
    );
}

#[test]
fn repeats_move_but_cannot_toggle_or_start_mutations() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    let first = app.selected.clone();
    assert_eq!(app.handle_input(Input::repeat(Key::Down)), Action::None);
    assert_ne!(app.selected, first);

    let separators = app.separators;
    let status_visible = app.status_visible;
    assert_eq!(
        app.handle_input(Input::new(Key::Character('s'), KeyPhase::Repeat)),
        Action::None
    );
    assert_eq!(app.separators, separators);
    assert_eq!(app.status_visible, status_visible);

    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_input(Input::repeat(Key::Enter)), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(
        app.handle_input(Input::repeat(Key::Character('x'))),
        Action::None
    );
    assert!(matches!(app.mutation, MutationState::Idle));
    assert!(!app.config.is_archived(&BranchId::new("feature")));
    assert_eq!(
        app.handle_input(Input::repeat(Key::Character('X'))),
        Action::None
    );
    assert!(matches!(app.mutation, MutationState::Idle));
}

#[test]
fn repeat_events_cannot_open_or_drive_pickers() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    app.handle_input(Input::repeat(Key::Character('T')));
    assert_eq!(app.overlay, Overlay::None);
    app.handle_input(Input::press(Key::Character('T')));
    let before = app.overlay.clone();
    app.handle_input(Input::repeat(Key::Down));
    assert_eq!(app.overlay, before);
    app.handle_input(Input::press(Key::Escape));

    app.handle_input(Input::repeat(Key::Character('C')));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn overlays_are_exclusive_and_own_input_but_ctrl_c_still_wins() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    app.handle_input(Input::press(Key::Character('/')));
    assert_eq!(app.overlay, Overlay::Search);

    app.handle_input(Input::press(Key::Character('?')));
    assert_eq!(app.overlay, Overlay::Search);
    assert_eq!(app.filter, "?");
    assert_eq!(app.handle_input(Input::press(Key::Quit)), Action::Quit);

    app.handle_input(Input::press(Key::Escape));
    app.handle_input(Input::press(Key::Character('?')));
    assert_eq!(app.overlay, Overlay::Help);
    let order = app.order_mode;
    app.handle_input(Input::press(Key::Character('t')));
    assert_eq!(app.overlay, Overlay::Help);
    assert_eq!(app.order_mode, order);
}

#[test]
fn deletion_confirmation_has_precedence_over_overlay_owner() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("merged", None, "merged", false),
    ]));
    app.selected = Some(BranchId::new("merged"));
    app.begin_delete_confirmation();
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));

    app.overlay = Overlay::Search;
    assert_eq!(
        app.handle_input(Input::press(Key::Character('n'))),
        Action::None
    );
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(app.overlay, Overlay::Search);
    assert!(app.filter.is_empty());
}
