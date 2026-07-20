mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use stackmap::app::{App, LanePitch};
use stackmap::events::Key;
use stackmap::model::{BranchId, ConfiguredUpstream, DiffStat, DiffState, RemoteRefEvidence};
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
    assert_eq!(clamped.effective_pitch, 6);
    assert!(clamped.last_visible_lane < 29);
    assert!(clamped.name_width(29) >= 8);
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
    assert_eq!(line.chars().next(), Some('●'));
    assert_eq!(line.chars().nth(1), Some('›'));
    assert_eq!(line.chars().nth(3), Some('○'));
    assert_eq!(char_column(line, "current"), Some(5));
    let accent = stack_color("test-repository", &BranchId::new("current"), &app.config);
    assert_eq!(
        terminal.backend().buffer()[(0, y as u16)].bg,
        if accent == Color::Reset {
            selected_background()
        } else {
            accent
        }
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
    let (y, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| char_column(line, "main") == Some(2))
        .map(|(y, line)| (y, line.as_str()))
        .expect("trunk branch row");
    assert_eq!(line.chars().next(), Some('◉'));
    assert_eq!(char_column(line, "main"), Some(2));
    let marker = &terminal.backend().buffer()[(0, y as u16)];
    assert_eq!(marker.fg, trunk_color());
    assert!(marker.modifier.contains(Modifier::BOLD));

    app.config
        .set_color_in_memory(&BranchId::new("conflict"), Some(TRUNK_COLOR_HEX))
        .unwrap();
    assert!(
        stackmap::app::COLOR_OPTIONS
            .iter()
            .all(|(_, value)| *value != Some(TRUNK_COLOR_HEX))
    );
    if trunk_color() != Color::Reset {
        assert_ne!(
            stack_color("repository", &BranchId::new("conflict"), &app.config),
            trunk_color()
        );
    }
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
    let selected_accent = stack_color("test-repository", &BranchId::new("selected"), &app.config);
    let selected_accent = if selected_accent == Color::Reset {
        selected_background()
    } else {
        selected_accent
    };
    for x in 0..80 {
        assert_eq!(
            terminal.backend().buffer()[(x, current_y as u16)].bg,
            current_background()
        );
        assert_eq!(
            terminal.backend().buffer()[(x, selected_y as u16)].bg,
            selected_accent
        );
        assert!(
            !terminal.backend().buffer()[(x, selected_y as u16)]
                .modifier
                .contains(Modifier::REVERSED)
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
fn named_stack_renders_a_white_nonselectable_label_above_its_head() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("root", None, "root", false),
        common::branch("tip", Some("root"), "root", false),
    ]));
    app.selected = Some(BranchId::new("root"));
    app.handle_key(Key::Character('n'));
    for character in "Release train".chars() {
        app.handle_key(Key::Character(character));
    }
    app.handle_key(Key::Enter);

    let mut terminal = Terminal::new(TestBackend::new(90, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (label_y, label_line) = line_with(&lines, "Release train");
    let (tip_y, _) = line_with(&lines, "tip");
    assert_eq!(label_y + 1, tip_y);
    let label_x = char_column(label_line, "Release train").unwrap() as u16;
    let cell = &terminal.backend().buffer()[(label_x, label_y as u16)];
    assert_eq!(cell.fg, Color::White);
    assert!(cell.modifier.contains(Modifier::BOLD));
    assert_eq!(app.projection.selectable.len(), 2);
}

#[test]
fn archive_view_renders_unarchived_ancestry_dimmed_and_nonselectable() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("parent", None, "parent", false),
        common::branch("archived-tip", Some("parent"), "parent", false),
    ]));
    app.selected = Some(BranchId::new("archived-tip"));
    app.handle_key(Key::Character('x'));
    app.handle_key(Key::Character('a'));

    let mut terminal = Terminal::new(TestBackend::new(90, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (parent_y, parent_line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("parent") && !line.contains("archived-tip"))
        .map(|(y, line)| (y, line.as_str()))
        .expect("dim parent context");
    let parent_x = char_column(parent_line, "parent").unwrap() as u16;
    assert!(
        terminal.backend().buffer()[(parent_x, parent_y as u16)]
            .modifier
            .contains(Modifier::DIM)
    );
    assert!(!app.projection.selectable.contains(&BranchId::new("parent")));
}

#[test]
fn footer_labels_a_as_view_archive() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "branch", None, "branch", false,
    )]));
    let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    assert!(
        rendered_lines(&terminal)
            .iter()
            .any(|line| line.contains("a View Archive"))
    );
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
        let lines: Vec<String> = (0..12)
            .map(|y| {
                (0..body_width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
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
        let selected_lines: Vec<String> = (0..12)
            .map(|y| {
                (0..body_width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
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
    let side_connector = connectors
        .iter()
        .find(|(_, connector)| connector.parent.as_ref() == Some(&BranchId::new("root")))
        .map(|(_, connector)| connector)
        .expect("side-stack connector");
    let parent_stack = app
        .projection
        .row_for(&BranchId::new("root"))
        .expect("parent projection row")
        .stack_id
        .clone();
    app.config
        .set_color_in_memory(&parent_stack, Some("#123456"))
        .unwrap();
    app.config
        .set_color_in_memory(&side_connector.stack_id, Some("#abcdef"))
        .unwrap();
    let repository_id = app.snapshot.as_ref().unwrap().repository_id.as_ref();
    let parent_color = stack_color(repository_id, &parent_stack, &app.config);
    let child_color = stack_color(repository_id, &side_connector.stack_id, &app.config);
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
        if connector.parent.as_ref() == Some(&BranchId::new("root")) {
            assert_eq!(buffer[(left, y)].fg, parent_color);
            for x in left + 1..=right {
                assert_eq!(buffer[(x, y)].fg, child_color);
            }
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
fn focused_sibling_rows_keep_geometry_and_use_dim_modifier() {
    let tracked = |name: &str, parent: Option<&str>, root: &str, current: bool| {
        let mut branch = common::branch(name, parent, root, current);
        branch.trunk = Some(BranchId::new("main"));
        branch.graphite = stackmap::model::GraphiteProvenance::Tracked;
        branch
    };
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", false),
        tracked("alpha", None, "alpha", true),
        tracked("alpha-tip", Some("alpha"), "alpha", false),
        tracked("beta", None, "beta", false),
    ]))
    .clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.selected = Some(BranchId::new("alpha"));
    app.handle_key(stackmap::events::Key::Character('h'));
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&terminal);
    let (alpha_y, alpha_line) = line_with(&lines, "alpha-tip");
    let (beta_y, beta_line) = line_with(&lines, "beta");
    let alpha_x = char_column(alpha_line, "alpha-tip").unwrap() as u16;
    let beta_x = char_column(beta_line, "beta").unwrap() as u16;
    assert!(
        !terminal.backend().buffer()[(alpha_x, alpha_y as u16)]
            .modifier
            .contains(Modifier::DIM)
    );
    assert!(
        terminal.backend().buffer()[(beta_x, beta_y as u16)]
            .modifier
            .contains(Modifier::DIM)
    );
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
    app.begin_delete_confirmation();
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
    assert!(rendered.contains("uppercase X"));
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
fn archive_mode_is_unmistakably_framed_and_empty_state_explains_return() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![common::branch(
        "main", None, "main", true,
    )]));
    app.handle_key(stackmap::events::Key::Character('a'));
    app.message = Some(Arc::from("configuration saved"));
    let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("ARCHIVE · local refs only · no fetch"));
    assert!(rendered.contains("No archived branches · press a to return to Active"));
    assert!(rendered.contains("x restore"));
    assert!(rendered.contains("X delete"));
}

#[test]
fn archive_badge_stays_visible_at_minimum_width_with_a_long_repository_path() {
    let mut snapshot =
        (*common::snapshot(vec![common::branch("main", None, "main", true)])).clone();
    snapshot.root = "/a/very/long/repository/path/that/must/not/cover/archive".into();
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    app.handle_key(stackmap::events::Key::Character('a'));
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let header = rendered_lines(&terminal)[0].clone();
    assert!(header.contains("ARCHIVE · local refs only · no fetch"));
}

#[test]
fn archive_range_uses_a_non_color_marker_and_exposes_action_count_and_endpoints() {
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]));
    app.selected = Some(BranchId::new("feature"));
    app.handle_key(stackmap::events::Key::Character('v'));
    let mut terminal = Terminal::new(TestBackend::new(120, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("■"));
    assert!(rendered.contains("RANGE ARCHIVE 1 branches"));
    assert!(rendered.contains("feature → feature"));
    assert!(rendered.contains("Enter archive"));
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
fn footer_keeps_controls_progress_notices_and_stale_health_independent() {
    let now = Instant::now();
    let initial = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("feature", None, "feature", false),
    ]);
    let mut app = App::with_mutation_timing(Duration::from_secs(10), Duration::from_secs(5));
    app.apply_snapshot(initial);
    app.selected = Some(BranchId::new("feature"));
    app.message = Some(Arc::from("ordinary message"));
    app.mark_stale(Arc::from("refresh failed"));
    assert!(matches!(
        app.handle_key(stackmap::events::Key::Enter),
        stackmap::app::Action::Checkout(_)
    ));
    app.finish_checkout_at(Ok(()), 42, now);

    let mut terminal = Terminal::new(TestBackend::new(180, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let progress = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(progress.contains("? help"));
    assert!(progress.contains("ordinary message"));
    assert!(progress.contains("verifying checkout of feature"));
    assert!(progress.contains("STALE: refresh failed"));

    let mut matching = (*common::snapshot(vec![
        common::branch("main", None, "main", false),
        common::branch("feature", None, "feature", true),
    ]))
    .clone();
    matching.generation = 2;
    app.apply_structural_snapshot_at(Arc::new(matching), 42, now);
    app.mark_stale(Arc::from("later refresh failed"));
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let reconciled = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(reconciled.contains("? help"));
    assert!(reconciled.contains("ordinary message"));
    assert!(reconciled.contains("checked out feature"));
    assert!(reconciled.contains("STALE: later refresh failed"));
    assert!(!reconciled.contains("verifying checkout"));
}

#[test]
fn verified_deletion_returns_to_controls_without_stuck_progress() {
    let now = Instant::now();
    let initial = common::snapshot(vec![
        common::branch("main", None, "main", true),
        common::branch("merged", None, "merged", false),
    ]);
    let mut app = App::default();
    app.apply_snapshot(initial);
    app.selected = Some(BranchId::new("merged"));
    assert_eq!(
        app.handle_key(stackmap::events::Key::Character('X')),
        stackmap::app::Action::None
    );
    assert!(matches!(
        app.handle_key(stackmap::events::Key::Character('y')),
        stackmap::app::Action::Delete(_)
    ));
    app.finish_deletion_at(Ok(stackmap::adapters::git::DeleteOutcome::Deleted), 51, now);
    let mut matching =
        (*common::snapshot(vec![common::branch("main", None, "main", true)])).clone();
    matching.generation = 2;
    app.apply_structural_snapshot_at(Arc::new(matching), 51, now);

    let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("X delete"));
    assert!(rendered.contains("? help"));
    assert!(rendered.contains("deleted merged locally"));
    assert!(!rendered.contains("verifying deletion"));
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
fn nested_side_stacks_clamp_to_visible_graph_edge_with_stable_overflow_cue() {
    let mut branches = vec![common::branch("root", None, "root", true)];
    let mut graphite_children = Vec::new();
    let mut parent = "root".to_owned();
    for depth in 1..=12 {
        let primary = format!("primary-{depth:02}");
        let side = format!("side-{depth:02}");
        branches.push(common::branch(&primary, Some(&parent), "root", false));
        branches.push(common::branch(&side, Some(&parent), "root", false));
        graphite_children.push((
            BranchId::new(parent.clone()),
            Arc::from([BranchId::new(primary), BranchId::new(side.clone())]),
        ));
        parent = side;
    }
    let mut snapshot = (*common::snapshot(branches)).clone();
    snapshot.graphite_children = Arc::from(graphite_children);
    let mut app = App::default();
    app.apply_snapshot(Arc::new(snapshot));
    let deepest = BranchId::new("side-12");
    let logical_lane = app.projection.row_for(&deepest).unwrap().lane;
    let geometry = RenderGeometry::new(
        40,
        stackmap::ui::layout::WidthMode::Narrow,
        app.lane_pitch,
        app.projection.lane_count,
    );
    assert!(logical_lane > geometry.last_visible_lane);
    assert!(geometry.graph_buffer_width() < geometry.metadata_start);
    assert!(geometry.name_width(logical_lane) >= 8);

    for _ in 0..3 {
        app.handle_key(stackmap::events::Key::Character('+'));
    }
    let expanded = RenderGeometry::new(
        40,
        stackmap::ui::layout::WidthMode::Narrow,
        app.lane_pitch,
        app.projection.lane_count,
    );
    assert_eq!(expanded.effective_pitch, 6);
    assert!(expanded.name_width(logical_lane) >= 8);
    app.handle_key(stackmap::events::Key::Character('0'));

    let mut terminal = Terminal::new(TestBackend::new(40, 100)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let initial_lines = rendered_lines(&terminal);
    let (_, initial) = line_with(&initial_lines, "side-12");
    assert_eq!(
        char_column(initial, "side-12"),
        Some(geometry.name_x(logical_lane))
    );
    assert_eq!(initial.chars().nth(geometry.graph_max_x), Some('○'));
    assert_eq!(initial.chars().nth(geometry.overflow_cue_x()), Some('»'));
    assert_eq!(initial.chars().count(), 40);

    app.selected = Some(deepest);
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let selected_lines = rendered_lines(&terminal);
    let (_, selected) = line_with(&selected_lines, "side-12");
    assert_eq!(
        char_column(selected, "side-12"),
        char_column(initial, "side-12")
    );
    assert_eq!(selected.chars().nth(geometry.overflow_cue_x()), Some('»'));
}

#[test]
fn forty_column_archive_row_composes_worktree_divergence_and_containment() {
    let mut hidden = common::branch("useful-hidden-name", None, "useful-hidden-name", false);
    hidden.worktree = Some(PathBuf::from("/tmp/worktrees/hidden-worktree"));
    hidden.configured_upstream = ConfiguredUpstream::Diverged {
        reference: Arc::from("refs/remotes/origin/main"),
        ahead: 2,
        behind: 3,
    };
    hidden.remote_ref = RemoteRefEvidence::Contained {
        reference: Arc::from("origin/backup"),
        source_token: 44,
        checked_at: SystemTime::UNIX_EPOCH,
    };
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        hidden,
    ]));
    app.config
        .set_archived_in_memory(&BranchId::new("useful-hidden-name"), true);
    app.handle_key(stackmap::events::Key::Character('a'));

    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("useful-hidden"));
    assert!(rendered.contains('⎇'));
    assert!(rendered.contains("↑2↓3"));
    assert!(rendered.contains("r✓"));
    assert!(rendered.contains("ARCHIVE"));
    assert!(rendered.contains("000000000000002c"));

    let mut wide = Terminal::new(TestBackend::new(140, 10)).unwrap();
    wide.draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let lines = rendered_lines(&wide);
    let (_, branch_line) = line_with(&lines, "useful-hidden-name");
    assert!(branch_line.contains('⎇'));
    assert!(branch_line.contains("up ↑2↓3"));
    assert!(branch_line.contains("remote-ref ✓"));
}

#[test]
fn wide_archive_detail_reports_canonical_upstream_source_token_time_and_no_fetch() {
    let mut hidden = common::branch("hidden", None, "hidden", false);
    hidden.configured_upstream = ConfiguredUpstream::Behind {
        reference: Arc::from("refs/remotes/origin/main"),
        behind: 4,
    };
    hidden.remote_ref = RemoteRefEvidence::Contained {
        reference: Arc::from("origin/release"),
        source_token: 0x2a,
        checked_at: SystemTime::UNIX_EPOCH,
    };
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        hidden,
    ]));
    app.config
        .set_archived_in_memory(&BranchId::new("hidden"), true);
    app.handle_key(stackmap::events::Key::Character('a'));

    let mut terminal = Terminal::new(TestBackend::new(180, 12)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("refs/remotes/origin/main"));
    assert!(rendered.contains("origin/release"));
    assert!(rendered.contains("000000000000002a"));
    assert!(rendered.contains("1969-12-31") || rendered.contains("1970-01-01"));
    assert!(rendered.contains("no fetch"));
}

#[test]
fn unavailable_archive_evidence_never_renders_as_local_only() {
    let mut hidden = common::branch("uncertain", None, "uncertain", false);
    hidden.remote_ref = RemoteRefEvidence::Unavailable {
        reason: Arc::from("bounded command timed out"),
        source_token: None,
        checked_at: SystemTime::UNIX_EPOCH,
    };
    let mut app = App::default();
    app.apply_snapshot(common::snapshot(vec![
        common::branch("main", None, "main", true),
        hidden,
    ]));
    app.config
        .set_archived_in_memory(&BranchId::new("uncertain"), true);
    app.handle_key(stackmap::events::Key::Character('a'));

    let mut terminal = Terminal::new(TestBackend::new(140, 10)).unwrap();
    terminal
        .draw(|frame| stackmap::ui::render(frame, &mut app, UNIX_EPOCH))
        .unwrap();
    let rendered = rendered_lines(&terminal).join("\n");
    assert!(rendered.contains("remote ?"));
    assert!(rendered.contains("unavailable"));
    assert!(!rendered.contains("local only"));
}

#[test]
fn help_documents_archive_range_and_picker_keys() {
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
    assert!(rendered.contains("archive / restore selected local branch"));
    assert!(rendered.contains("guarded delete exact local branch"));
    assert!(rendered.contains("preview contiguous archive/restore range"));
    assert!(rendered.contains("order picker"));
    assert!(rendered.contains("color picker"));
    assert!(rendered.contains("› selected"));
    assert!(rendered.contains("○ branch"));
    assert!(rendered.contains("● current"));
    assert!(rendered.contains("◉ trunk"));
    assert!(rendered.contains("■ range"));
    assert!(rendered.contains("* dirty"));
    assert!(rendered.contains("⎇ worktree"));
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
