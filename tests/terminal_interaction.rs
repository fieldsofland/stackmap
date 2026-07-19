mod common;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use stackmap::app::{Action, App, MutationState, Overlay};
use stackmap::events::{Input, Key, KeyPhase};
use stackmap::model::BranchId;

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
            KeyModifiers::ALT,
            KeyEventKind::Repeat
        )),
        Some(Input::repeat(Key::SectionDown))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('K'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::StackUp))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('J'),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::StackDown))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::SectionUp))
    );
    assert_eq!(
        Input::from_event(event(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )),
        Some(Input::press(Key::SectionDown))
    );
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
    assert_eq!(
        app.handle_input(Input::new(Key::Character('s'), KeyPhase::Repeat)),
        Action::None
    );
    assert_eq!(app.separators, separators);

    app.selected = Some(BranchId::new("feature"));
    assert_eq!(app.handle_input(Input::repeat(Key::Enter)), Action::None);
    assert!(matches!(app.mutation, MutationState::Idle));
    assert_eq!(
        app.handle_input(Input::repeat(Key::Character('x'))),
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
    app.handle_input(Input::press(Key::Character('x')));
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
