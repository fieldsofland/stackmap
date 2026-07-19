mod common;

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use stackmap::app::{App, LanePitch};
use stackmap::model::{BranchId, DiffStat, DiffState};
use stackmap::ui::layout::{RenderGeometry, areas};
use stackmap::ui::theme::{
    TRUNK_COLOR_HEX, current_background, selected_background, stack_color, trunk_color,
};

fn rendered_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let area = terminal.backend().buffer().area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn line_with<'a>(lines: &'a [String], needle: &str) -> (usize, &'a str) {
    lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .map(|(index, line)| (index, line.as_str()))
        .unwrap_or_else(|| panic!("missing {needle:?} in {lines:#?}"))
}

fn char_column(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|byte| line[..byte].chars().count())
}

#[test]
fn narrow_renderer_contains_every_fixed_semantic_field() {
    let mut app = App::default();
    let mut current = common::branch("feature/current", None, "feature/current", true);
    current.dirty = true;
    app.apply_snapshot(common::snapshot(vec![current]));
    let backend = TestBackend::new(80, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            stackmap::ui::render(
                frame,
                &mut app,
                UNIX_EPOCH + Duration::from_secs(1_700_003_600),
            )
        })
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("feature/current"));
    assert!(rendered.contains("1h"));
    assert!(rendered.contains("…loading"));
    assert!(rendered.contains("●"));
}

#[test]
fn additions_and_deletions_use_independent_semantic_colors() {
    let mut app = App::default();
    let mut branch = common::branch("feature/colors", None, "feature/colors", true);
    branch.diff = DiffState::Ready(DiffStat {
        insertions: 12,
        deletions: 7,
        ..DiffStat::default()
    });
    app.apply_snapshot(common::snapshot(vec![branch]));
    let backend = TestBackend::new(80, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let addition = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "+")
        .expect("addition cell");
    let deletion = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "-")
        .expect("deletion cell");
    assert_eq!(addition.fg, Color::Green);
    assert_eq!(deletion.fg, Color::Red);
}

#[test]
fn render_geometry_is_left_anchored_and_selection_independent() {
    let narrow = RenderGeometry::new(
        80,
        stackmap::ui::layout::WidthMode::Narrow,
        LanePitch::Auto,
        4,
    );
    assert_eq!(narrow.effective_pitch, 2);
    assert_eq!(narrow.lane_x(0), 0);
    assert_eq!(narrow.lane_x(1), 3);
    assert_eq!(narrow.lane_x(2), 5);
    assert_eq!(narrow.name_x(1), 5);
    assert_eq!(narrow.name_x(2), 7);

    let medium = RenderGeometry::new(
        100,
        stackmap::ui::layout::WidthMode::Medium,
        LanePitch::Auto,
        4,
    );
    let wide = RenderGeometry::new(
        130,
        stackmap::ui::layout::WidthMode::Wide,
        LanePitch::Auto,
        4,
    );
    assert_eq!(medium.effective_pitch, 3);
    assert_eq!(wide.effective_pitch, 4);

    let clamped = RenderGeometry::new(
        40,
        stackmap::ui::layout::WidthMode::Narrow,
        LanePitch::Fixed(6),
        30,
    );
    assert_eq!(clamped.effective_pitch, 1);
}

#[test]
fn current_nontrunk_has_status_and_topology_circles_in_fixed_columns() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "current", None, "current", true,
    )]));
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (y, line) = line_with(&lines, "current");
    assert_eq!(line.chars().nth(0), Some('●'));
    assert_eq!(line.chars().nth(1), Some('›'));
    assert_eq!(line.chars().nth(3), Some('○'));
    assert_eq!(char_column(line, "current"), Some(5));
    assert_eq!(
        terminal.backend().buffer()[(0, y as u16)].bg,
        selected_background()
    );
}

#[test]
fn trunk_uses_reserved_bold_hue_and_checked_out_marker() {
    let mut trunk = common::branch("main", None, "main", true);
    trunk.trunk = Some(BranchId::new("main"));
    trunk.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut snapshot = (*common::snapshot(vec![trunk])).clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (y, line) = line_with(&lines, "main");
    assert_eq!(line.chars().nth(0), Some('◉'));
    assert_eq!(char_column(line, "main"), Some(2));
    let marker = &terminal.backend().buffer()[(0, y as u16)];
    assert_eq!(marker.fg, trunk_color());
    assert!(marker.modifier.contains(Modifier::BOLD));

    app.config
        .set_color_in_memory(&BranchId::new("conflict"), Some(TRUNK_COLOR_HEX))
        .unwrap();
    assert_ne!(
        stack_color("repository", &BranchId::new("conflict"), &app.config),
        trunk_color()
    );
}

#[test]
fn selected_and_current_backgrounds_fill_rows_without_destroying_diff_colors() {
    let mut current = common::branch("current", None, "current", true);
    current.diff = DiffState::Ready(DiffStat {
        insertions: 12,
        deletions: 7,
        ..DiffStat::default()
    });
    let mut selected = common::branch("selected", None, "selected", false);
    selected.diff = current.diff.clone();
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![current, selected]));
    app.selected = Some(BranchId::new("selected"));
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (current_y, _) = line_with(&lines, "current");
    let (selected_y, _) = line_with(&lines, "selected");
    for x in 0..80 {
        assert_eq!(
            terminal.backend().buffer()[(x, current_y as u16)].bg,
            current_background()
        );
        assert_eq!(
            terminal.backend().buffer()[(x, selected_y as u16)].bg,
            selected_background()
        );
    }
    let mut selected_cells = (0..80).map(|x| &terminal.backend().buffer()[(x, selected_y as u16)]);
    assert!(
        selected_cells
            .clone()
            .any(|cell| cell.symbol() == "+" && cell.fg == Color::Green)
    );
    assert!(selected_cells.any(|cell| cell.symbol() == "-" && cell.fg == Color::Red));
}

#[test]
fn worktree_indicator_is_fixed_and_wide_detail_shows_path() {
    let mut app = App::default();
    let mut current = common::branch("current", None, "current", true);
    current.worktree = Some("/repo".into());
    let mut linked = common::branch("linked", None, "linked", false);
    linked.worktree = Some("/tmp/linked-worktree".into());
    app.apply_snapshot(common::snapshot(vec![current, linked]));
    let backend = TestBackend::new(140, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let branch_lines: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("current") || line.contains("linked"))
        .collect();
    assert_eq!(branch_lines.len(), 2);
    let wt_columns: Vec<_> = branch_lines
        .iter()
        .map(|line| {
            line.chars()
                .position(|character| character == '⎇')
                .expect("worktree indicator")
        })
        .collect();
    assert_eq!(wt_columns[0], wt_columns[1]);
    assert!(lines.iter().any(|line| line.contains("⎇ linked-worktree")));

    app.selected = Some(stackmap::model::BranchId::new("linked"));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("/tmp/linked-worktree"));

    let mut narrow = Terminal::new(TestBackend::new(40, 10)).unwrap();
    narrow
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let narrow_lines = rendered_lines(&narrow);
    let (_, linked_line) = line_with(&narrow_lines, "linked");
    assert!(linked_line.contains('⎇'));
    assert!(!linked_line.contains("linked-worktree"));
}

#[test]
fn stack_local_name_columns_are_stable_across_selection_and_child_lanes() {
    for width in [40, 200] {
        let mut app = App::default();
        app.apply_snapshot(common::snapshot(vec![
            common::branch("root", None, "root", true),
            common::branch("primary", Some("root"), "root", false),
            common::branch("side", Some("root"), "root", false),
        ]));
        let backend = TestBackend::new(width, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
            .unwrap();
        let body_width = stackmap::ui::layout::areas(ratatui::layout::Rect::new(0, 0, width, 12))
            .body
            .width;
        let lines = rendered_lines(&terminal);
        let columns: Vec<_> = ["root", "primary", "side"]
            .into_iter()
            .map(|name| {
                lines
                    .iter()
                    .find_map(|line| line.find(name).map(|byte| line[..byte].chars().count()))
                    .expect("branch label")
            })
            .collect();
        assert_eq!(columns[0], columns[1], "same stack at width {width}");
        let expected_pitch = if width == 40 { 2 } else { 4 };
        assert_eq!(
            columns[2] - columns[1],
            expected_pitch,
            "child stack at width {width}: {columns:?}"
        );
        assert!(body_width >= 40);

        app.selected = Some(BranchId::new("side"));
        terminal
            .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
            .unwrap();
        let selected_lines = rendered_lines(&terminal);
        for (name, expected) in ["root", "primary", "side"].into_iter().zip(columns) {
            let (_, line) = line_with(&selected_lines, name);
            assert_eq!(char_column(line, name), Some(expected));
        }
    }
}

#[test]
fn connectors_draw_exact_lane_endpoints_and_root_contact() {
    let mut main = common::branch("main", None, "main", false);
    main.trunk = Some(BranchId::new("main"));
    main.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut root = common::branch("root", None, "root", true);
    root.trunk = Some(BranchId::new("main"));
    root.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut primary = common::branch("primary", Some("root"), "root", false);
    primary.trunk = Some(BranchId::new("main"));
    primary.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut side = common::branch("side", Some("root"), "root", false);
    side.trunk = Some(BranchId::new("main"));
    side.graphite = stackmap::model::GraphiteProvenance::Tracked;
    let mut snapshot = (*common::snapshot(vec![main, root, primary, side])).clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("main"), Arc::from([BranchId::new("root")])),
        (
            BranchId::new("root"),
            Arc::from([BranchId::new("primary"), BranchId::new("side")]),
        ),
    ]);
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    let terminal_area = ratatui::layout::Rect::new(0, 0, 80, 20);
    let body = areas(terminal_area).body;
    let geometry = RenderGeometry::new(
        body.width,
        areas(terminal_area).mode,
        app.lane_pitch,
        app.projection.lane_count,
    );
    let connectors: Vec<_> = app
        .projection
        .entries
        .iter()
        .enumerate()
        .filter_map(|(row, entry)| match entry {
            stackmap::model::topology::ProjectionEntry::Divider(
                stackmap::model::topology::DividerRow::Connector(connector),
            ) => Some((row, connector.clone())),
            _ => None,
        })
        .collect();
    let spacers: Vec<_> = app
        .projection
        .entries
        .iter()
        .enumerate()
        .filter_map(|(row, entry)| {
            matches!(
                entry,
                stackmap::model::topology::ProjectionEntry::Divider(
                    stackmap::model::topology::DividerRow::Spacer { .. }
                )
            )
            .then_some(row)
        })
        .collect();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let buffer = terminal.backend().buffer();
    for (visual_row, connector) in connectors {
        let y = body.y + visual_row as u16;
        let left = geometry.lane_x(connector.to_lane) as u16;
        let right = geometry.lane_x(connector.from_lane) as u16;
        assert!(matches!(buffer[(left, y)].symbol(), "┌" | "├"));
        assert_eq!(buffer[(right, y)].symbol(), "┘");
        for x in left + 1..right {
            assert!(matches!(buffer[(x, y)].symbol(), "─" | "┼" | "┬" | "┴"));
        }
        if connector.parent.as_ref() == Some(&BranchId::new("main")) {
            assert_eq!(buffer[(0, y)].symbol(), "┌");
            assert_eq!(buffer[(0, y + 1)].symbol(), "○");
        }
    }
    for visual_row in spacers {
        let y = body.y + visual_row as u16;
        for x in geometry.metadata_start as u16..body.width {
            assert_eq!(buffer[(x, y)].symbol(), " ");
        }
    }
}

#[test]
fn plus_minus_and_zero_change_global_geometry_without_selection_recentering() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("root", None, "root", true),
        common::branch("primary", Some("root"), "root", false),
        common::branch("side", Some("root"), "root", false),
    ]));
    let area = areas(ratatui::layout::Rect::new(0, 0, 80, 12));
    let automatic = RenderGeometry::new(
        area.body.width,
        area.mode,
        app.lane_pitch,
        app.projection.lane_count,
    );
    assert_eq!(automatic.effective_pitch, 2);
    app.handle_key(stackmap::events::Key::Character('+'));
    let expanded = RenderGeometry::new(
        area.body.width,
        area.mode,
        app.lane_pitch,
        app.projection.lane_count,
    );
    assert_eq!(expanded.effective_pitch, 4);
    app.selected = Some(BranchId::new("side"));
    let selected = RenderGeometry::new(
        area.body.width,
        area.mode,
        app.lane_pitch,
        app.projection.lane_count,
    );
    assert_eq!(selected, expanded);
    app.handle_key(stackmap::events::Key::Character('-'));
    assert_eq!(app.lane_pitch, LanePitch::Fixed(3));
    app.handle_key(stackmap::events::Key::Character('0'));
    assert_eq!(app.lane_pitch, LanePitch::Auto);
}

#[test]
fn deletion_confirmation_keeps_choices_visible_at_minimum_width() {
    let mut app = App::default();
    let target = "feature/a-long-but-valid-branch-name";
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch(target, None, target, false),
    ]));
    app.selected = Some(stackmap::model::BranchId::new(target));
    let backend = TestBackend::new(40, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    app.handle_key(stackmap::events::Key::Character('x'));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Confirm deletion"));
    assert!(rendered.contains("[y] delete"));
    assert!(rendered.contains("[n/Esc] cancel"));
}

#[test]
fn too_narrow_terminal_has_explicit_state() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    let backend = TestBackend::new(39, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("at least 40 columns"));
}

#[test]
fn forty_column_terminal_renders_branches() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    let backend = TestBackend::new(40, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("main"));
    assert!(!rendered.contains("at least"));
    let lines = rendered_lines(&terminal);
    let (_, branch_line) = line_with(&lines, "main");
    assert_eq!(branch_line.chars().count(), 40);
    let geometry = RenderGeometry::new(
        40,
        stackmap::ui::layout::WidthMode::Narrow,
        LanePitch::Auto,
        app.projection.lane_count,
    );
    assert!(geometry.time.is_none());
    assert!(geometry.pr.is_none());
}

#[test]
fn relative_time_boundaries_are_deterministic() {
    let now = UNIX_EPOCH + Duration::from_secs(200_000);
    assert_eq!(stackmap::ui::tree::relative_time(199_950, now), "now");
    assert_eq!(stackmap::ui::tree::relative_time(199_940, now), "1m");
    assert_eq!(stackmap::ui::tree::relative_time(196_400, now), "1h");
    assert_eq!(stackmap::ui::tree::relative_time(27_200, now), "2d");
    assert_eq!(stackmap::ui::tree::relative_time(200_061, now), "clock?");
}

#[test]
fn wide_renderer_includes_selected_branch_detail() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "feature/detail",
        None,
        "feature/detail",
        true,
    )]));
    let backend = TestBackend::new(140, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            stackmap::ui::render(
                frame,
                &mut app,
                UNIX_EPOCH + Duration::from_secs(1_700_003_600),
            )
        })
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Branch detail"));
    assert!(rendered.contains("Last edited:"));
}

#[test]
fn stale_health_remains_visible_alongside_messages() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    app.message = Some(Arc::from("another message"));
    app.mark_stale(Arc::from("refresh failed"));
    let backend = TestBackend::new(80, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("stale"));
    assert!(rendered.contains("STALE: refresh failed"));
}

#[test]
fn forty_columns_keeps_deep_stack_rows_on_one_line() {
    let mut branches = Vec::new();
    for index in 0..30 {
        let name = format!("b{index:02}");
        let parent = (index > 0).then(|| format!("b{:02}", index - 1));
        branches.push(common::branch(&name, parent.as_deref(), "b00", index == 29));
    }
    let mut app = App::default();
    let mut snapshot = (*common::snapshot(branches)).clone();
    snapshot.graphite_children = Arc::from([(
        stackmap::model::BranchId::new("main"),
        Arc::from([stackmap::model::BranchId::new("branch-00")]),
    )]);
    app.apply_snapshot(Arc::new(snapshot));
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    for index in 25..30 {
        assert!(rendered.contains(&format!("b{index:02}")));
    }
}

#[test]
fn help_documents_deletion_entry_and_confirmation_keys() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    app.overlay = stackmap::app::Overlay::Help;
    let backend = TestBackend::new(90, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("guarded local branch deletion"));
    assert!(rendered.contains("confirm / cancel deletion"));
    assert!(rendered.contains("order picker"));
    assert!(rendered.contains("color picker"));
}

#[test]
fn picker_modals_render_textual_choices_and_commit_hints() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.handle_key(stackmap::events::Key::Character('T'));
    let backend = TestBackend::new(90, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let order = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(order.contains("Recent"));
    assert!(order.contains("Alphabetical"));
    assert!(order.contains("Graphite"));
    assert!(order.contains("Enter apply"));

    app.handle_key(stackmap::events::Key::Escape);
    app.selected = Some(stackmap::model::BranchId::new("feature"));
    app.handle_key(stackmap::events::Key::Character('C'));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let color = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(color.contains("Auto"));
    assert!(color.contains("Blue"));
    assert!(color.contains("Enter save"));
}

#[test]
fn focused_section_reserves_one_sticky_bottom_row_with_continuation_cue() {
    let mut branches = Vec::new();
    let mut trunk = common::branch("main", None, "main", true);
    trunk.trunk = Some(stackmap::model::BranchId::new("main"));
    trunk.graphite = stackmap::model::GraphiteProvenance::Tracked;
    branches.push(trunk);
    for index in 0..14 {
        let name = format!("branch-{index:02}");
        let parent = if index > 0 {
            format!("branch-{:02}", index - 1)
        } else {
            String::new()
        };
        let mut branch = common::branch(
            &name,
            (!parent.is_empty()).then_some(parent.as_str()),
            "branch-00",
            false,
        );
        branch.trunk = Some(stackmap::model::BranchId::new("main"));
        branch.graphite = stackmap::model::GraphiteProvenance::Tracked;
        branches.push(branch);
    }
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(branches));
    app.selected = Some(stackmap::model::BranchId::new("branch-00"));
    app.handle_key(stackmap::events::Key::Character('H'));

    let backend = TestBackend::new(80, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let body_last = (0..80).map(|x| buffer[(x, 7)].symbol()).collect::<String>();
    assert!(body_last.contains("main"));
    assert!(
        body_last.contains('↑') || body_last.contains('↓') || body_last.contains('↕'),
        "sticky row lacks continuation cue: {body_last:?}, scroll={}, bounds={:?}, sticky={:?}",
        app.scroll,
        app.focused_section_bounds(),
        app.sticky_visual_row()
    );
    for y in 2..7 {
        let row = (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>();
        assert!(!row.contains("main"), "sticky branch duplicated at row {y}");
    }

    app.handle_key(stackmap::events::Key::Character('t'));
    app.handle_key(stackmap::events::Key::Character('s'));
    let backend = TestBackend::new(80, 8);
    let mut resized = Terminal::new(backend).unwrap();
    resized
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = resized
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("branch-00"));
    let sticky = (0..80)
        .map(|x| resized.backend().buffer()[(x, 5)].symbol())
        .collect::<String>();
    assert!(sticky.contains("main"));
}

#[test]
fn footer_describes_contextual_stack_or_ten_row_navigation() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("one-off", None, "one-off", false),
        common::branch("stack", None, "stack", false),
        common::branch("stack-tip", Some("stack"), "stack", true),
    ]));
    let backend = TestBackend::new(90, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    app.selected = Some(stackmap::model::BranchId::new("one-off"));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let one_off = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(one_off.contains("J/K ±10"));

    app.selected = Some(stackmap::model::BranchId::new("stack-tip"));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let stack = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(stack.contains("J/K stacks"));
}

#[test]
fn five_thousand_stack_projection_renders_only_the_visible_window() {
    let branches = (0..5_000)
        .map(|index| {
            let name = format!("stack-{index:04}");
            common::branch(&name, None, &name, index == 0)
        })
        .collect();
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(branches));
    assert_eq!(app.projection.lane_count, 2);
    assert_eq!(app.projection.lane_spans.len(), 5_000);
    let backend = TestBackend::new(40, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    assert_eq!(terminal.backend().buffer().area.width, 40);
}
