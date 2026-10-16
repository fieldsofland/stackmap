use super::common;
use crate::app::{Action, App, ClipboardScope, MutationState, Overlay};
use crate::events::{Input, Key};
use crate::model::{BranchId, PullRequest, PullRequestLookup, PullRequestMatch, PullRequestStatus};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

fn app() -> App {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
        common::branch("child", Some("feature"), "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    app
}

fn search(app: &mut App, query: &str) {
    app.handle_key(Key::Character('?'));
    for ch in query.chars() {
        assert_eq!(app.handle_key(Key::Character(ch)), Action::None);
    }
}

fn render(app: &mut App, width: u16, height: u16) -> (String, Terminal<TestBackend>) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, app, UNIX_EPOCH))
        .unwrap();
    let text = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, terminal)
}

#[test]
fn action_search_owns_text_and_executes_copy_without_modifier_keys() {
    let mut app = app();
    search(&mut app, "copy stack");
    assert_eq!(app.action_menu_entries().len(), 1);
    let (text, _) = render(&mut app, 40, 14);
    assert!(text.contains("copy stack"));
    assert!(text.contains("Enter run"));
    let Action::Copy(request) = app.handle_key(Key::Enter) else {
        panic!("copy action");
    };
    assert_eq!(request.scope, ClipboardScope::Stack);
    assert_eq!(request.text.as_ref(), "feature\nchild");
    assert_eq!(app.overlay, Overlay::None);
    search(&mut app, "quit");
    assert_eq!(app.overlay, Overlay::Help);
    assert_eq!(app.handle_input(Input::repeat(Key::Enter)), Action::None);
    assert_eq!(app.handle_key(Key::Escape), Action::None);
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn action_menu_delete_always_opens_existing_confirmation() {
    let mut app = app();
    search(&mut app, "delete local");
    assert_eq!(app.action_menu_entries().len(), 1);
    assert_eq!(app.handle_key(Key::Enter), Action::None);
    assert!(matches!(app.mutation, MutationState::ConfirmingDeletion(_)));
    assert_eq!(app.handle_key(Key::Escape), Action::None);
    assert_eq!(app.mutation, MutationState::Idle);
}

#[test]
fn archive_view_and_undo_do_not_delete_or_switch_branches() {
    let mut app = app();
    search(&mut app, "keep Git");
    assert!(matches!(
        app.handle_key(Key::Enter),
        Action::PersistConfig(_)
    ));
    let id = BranchId::new("feature");
    assert!(app.config.is_archived(&id));
    assert!(app.message.as_deref().unwrap().contains("Git branch kept"));
    assert!(matches!(
        app.handle_key(Key::Character('u')),
        Action::PersistConfig(_)
    ));
    assert!(!app.config.is_archived(&id));
    assert_eq!(app.mutation, MutationState::Idle);
    app.handle_key(Key::Character('a'));
    let (text, _) = render(&mut app, 40, 10);
    assert!(text.contains("Active | [Archive]"));
    assert!(text.contains("? Actions"));
    assert!(text.contains("x Restore"));
}

#[test]
fn undo_refuses_to_archive_a_branch_that_became_current() {
    let mut app = app();
    let id = BranchId::new("feature");
    app.config.set_archived_in_memory(&id, true);
    app.handle_key(Key::Character('a'));
    app.selected = Some(id.clone());
    assert!(matches!(
        app.handle_key(Key::Character('x')),
        Action::PersistConfig(_)
    ));
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature", None, "feature", true,
    )]));
    assert_eq!(app.handle_key(Key::Character('u')), Action::None);
    assert!(!app.config.is_archived(&id));
}

#[test]
fn lifecycle_labels_and_icons_remain_distinct_at_all_supported_widths() {
    for (status, icon, color) in [
        (PullRequestStatus::Merged, "⑂", Color::Rgb(187, 154, 247)),
        (PullRequestStatus::Closed, "×", Color::Red),
        (PullRequestStatus::Approved, "✓", Color::Green),
    ] {
        for width in [40, 80, 100, 140, 200] {
            let mut branch = common::branch("feature", None, "feature", false);
            branch.pr_lookup = PullRequestLookup::Ready;
            branch.pr = Some(PullRequest {
                number: 42,
                title: Arc::from("Feature"),
                url: Arc::from("https://example.invalid/42"),
                status,
                head_oid: Some(branch.oid.clone()),
                match_quality: PullRequestMatch::ExactTip,
            });
            let mut app = App::default();
            app.apply_snapshot(common::snapshot(vec![branch]));
            let (text, terminal) = render(&mut app, width, 10);
            assert!(text.contains(&format!("{icon} #42")), "{width}: {text}");
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .any(|cell| cell.symbol() == icon && cell.fg == color)
            );
            if width >= 140 {
                assert!(text.contains(status.label()));
            }
            crate::ui::theme::with_no_color(|| {
                let (text, _) = render(&mut app, width, 10);
                assert!(text.contains(&format!("{icon} #42")));
            });
        }
    }
}

#[test]
fn lookup_states_are_visible_and_narrow_details_show_pr_status() {
    for (lookup, suffix, explanation) in [
        (PullRequestLookup::NotRequested, "?", "Not checked"),
        (PullRequestLookup::Checking, "…", "Checking"),
        (
            PullRequestLookup::Unavailable(Arc::from("offline")),
            "!",
            "Unavailable",
        ),
    ] {
        let mut branch = common::branch("feature", None, "feature", false);
        branch.pr_lookup = lookup;
        let mut app = App::default();
        app.apply_snapshot(common::snapshot(vec![branch]));
        let (text, _) = render(&mut app, 80, 12);
        assert!(text.contains(&format!("local {suffix}")));
        assert!(text.contains(explanation));
        app.handle_key(Key::Character('d'));
        let (text, _) = render(&mut app, 40, 20);
        assert!(text.contains("Branch details"));
        assert!(text.contains("PR lookup:"));
        app.handle_key(Key::Escape);
        assert!(!app.detail_sidebar);
    }
}

#[test]
fn action_menu_keeps_search_result_and_controls_in_short_terminals() {
    let mut app = app();
    search(&mut app, "copy branch");
    let (text, _) = render(&mut app, 40, 8);
    assert!(text.contains("copy branch"), "{text}");
    assert!(text.contains("Cmd+C"), "{text}");
    assert!(text.contains("Enter run"), "{text}");
}
