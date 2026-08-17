use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::path::Path;
use std::time::SystemTime;

use crate::app::{App, ConfigTarget, Overlay};
use crate::model::topology::{
    ArchiveMode, ConnectorRow, DividerRow, Emphasis, ProjectedRow, ProjectionEntry, StackLabelRow,
    VisualSectionDividerRow, VisualSectionLabelRow,
};
use crate::model::{
    Branch, BranchId, ConfiguredUpstream, DiffState, PullRequest, PullRequestStatus,
    RemoteRefEvidence,
};

use super::layout::{ColumnRange, RenderGeometry, WidthMode};
use super::theme::{
    current_background, selected_background, stack_color, trunk_color, visual_section_color,
};

mod connectors;
mod details;

use connectors::{paint_bits, paint_overflow_cue};
pub use details::{detail, evidence_source_footer, local_detail, relative_time};

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

#[derive(Clone, Debug)]
struct RenderCell {
    symbol: String,
    style: Style,
    preserve_foreground: bool,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            symbol: " ".into(),
            style: Style::default(),
            preserve_foreground: false,
        }
    }
}

pub fn render_with_mode(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    now: SystemTime,
    mode: WidthMode,
) {
    if mode == WidthMode::TooNarrow {
        frame.render_widget(
            Paragraph::new("stackmap needs at least 40 columns")
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }
    let Some(snapshot) = &app.snapshot else {
        frame.render_widget(Paragraph::new("Reading local branches…"), area);
        return;
    };
    if app.projection.selectable.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.filter.is_empty() {
                if matches!(app.archive_mode, ArchiveMode::Archive) {
                    "No archived branches · press a to return to Active"
                } else {
                    "No local branches"
                }
            } else {
                "No branches match the filter"
            }),
            area,
        );
        return;
    }

    let geometry = RenderGeometry::new(
        area.width,
        mode,
        app.lane_pitch,
        app.projection.lane_count.max(1),
    );
    let sticky = (area.height >= 2)
        .then(|| app.sticky_visual_row())
        .flatten();
    let scroll_height = area.height.saturating_sub(u16::from(sticky.is_some()));
    let focused_bounds = app.focused_section_bounds();
    let mut lines = Vec::with_capacity(scroll_height as usize);
    let minimum_start = focused_bounds.map(|(start, _)| start).unwrap_or(0);
    let start = app
        .scroll
        .max(minimum_start)
        .min(app.projection.entries.len());
    let maximum_end = focused_bounds
        .map(|(_, end)| end.saturating_add(1))
        .unwrap_or(app.projection.entries.len());
    let maximum_end = sticky.map_or(maximum_end, |row| maximum_end.min(row));
    let end = start
        .saturating_add(scroll_height as usize)
        .min(maximum_end);

    for (offset, entry) in app.projection.entries[start..end].iter().enumerate() {
        let visual_row = start + offset;
        let line = match entry {
            ProjectionEntry::Section(section) => section_line(&section.title, geometry),
            ProjectionEntry::StackLabel(label) => {
                stack_label_line(app, visual_row, label, geometry)
            }
            ProjectionEntry::VisualSectionLabel(label) => {
                visual_section_label_line(app, visual_row, label, geometry)
            }
            ProjectionEntry::VisualSectionDivider(divider) => {
                visual_section_divider_line(app, visual_row, divider, geometry)
            }
            ProjectionEntry::Divider(divider) => divider_line(app, visual_row, divider, geometry),
            ProjectionEntry::Branch(row) => {
                let Some(branch) = snapshot.branch(&row.branch) else {
                    continue;
                };
                branch_line(app, branch, row, visual_row, now, geometry)
            }
        };
        lines.push(line);
    }
    let rendered_height = lines.len() as u16;
    let content_y = if sticky.is_some() && focused_bounds.is_some() {
        area.y + scroll_height.saturating_sub(rendered_height)
    } else {
        area.y
    };
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, content_y, area.width, rendered_height),
    );

    if let Some(sticky_row) = sticky
        && let Some(ProjectionEntry::Branch(row)) = app.projection.entries.get(sticky_row)
        && let Some(branch) = snapshot.branch(&row.branch)
    {
        let mut line = branch_line(app, branch, row, sticky_row, now, geometry);
        if let Some((section_start, _)) = focused_bounds {
            let hidden_above = start > section_start;
            let hidden_below = end < sticky_row;
            let cue = match (hidden_above, hidden_below) {
                (true, true) => Some("↕"),
                (true, false) => Some("↑"),
                (false, true) => Some("↓"),
                (false, false) => None,
            };
            if let (Some(cue), Some(marker)) = (cue, line.spans.first_mut()) {
                *marker = Span::styled(cue, marker.style.fg(Color::DarkGray));
            }
        }
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(
                area.x,
                area.y + area.height.saturating_sub(1),
                area.width,
                1,
            ),
        );
    }
}

fn visual_section_label_line(
    app: &App,
    visual_row: usize,
    label: &VisualSectionLabelRow,
    geometry: RenderGeometry,
) -> Line<'static> {
    let mut cells = blank_cells(geometry.width);
    let (bits, styles) = rail_bits(app, visual_row, geometry);
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);
    let desired_x = geometry
        .name_x(label.lane)
        .saturating_add(label.manual_depth.saturating_mul(2));
    let x = desired_x.min(geometry.metadata_start.saturating_sub(2));
    let text = if x < desired_x {
        format!("{} {}", label.manual_depth, label.text)
    } else {
        label.text.to_string()
    };
    let width = geometry.metadata_start.saturating_sub(x + 1);
    let cursor = match &app.overlay {
        Overlay::StackNameEditor(editor)
            if editor.target == ConfigTarget::VisualSection(label.anchor.clone()) =>
        {
            Some(editor.cursor)
        }
        _ => None,
    };
    let text = inline_editor_text(&text, width, cursor);
    put_text(
        &mut cells,
        x,
        width,
        &text,
        emphasized(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            label.emphasis,
        ),
    );
    let selected = app.selected_label.as_ref()
        == Some(&crate::app::ConfigTarget::VisualSection(
            label.anchor.clone(),
        ));
    if selected {
        set_symbol(
            &mut cells,
            1,
            "›",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
        for cell in &mut cells {
            cell.style = cell.style.fg(Color::White);
        }
    }
    let selected_background = selected.then(|| {
        let color = visual_section_color(&label.color);
        if color == Color::Reset {
            selected_background()
        } else {
            color
        }
    });
    cells_to_line(cells, selected_background)
}

fn visual_section_divider_line(
    app: &App,
    visual_row: usize,
    divider: &VisualSectionDividerRow,
    geometry: RenderGeometry,
) -> Line<'static> {
    let mut cells = blank_cells(geometry.width);
    let (bits, styles) = rail_bits(app, visual_row, geometry);
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);
    let desired_x = geometry
        .name_x(divider.lane)
        .saturating_add(divider.manual_depth.saturating_mul(2));
    let x = desired_x.min(geometry.metadata_start.saturating_sub(2));
    let divider_text = if x < desired_x {
        format!("{}─", divider.manual_depth)
    } else {
        "────────".to_owned()
    };
    put_text(
        &mut cells,
        x,
        geometry.metadata_start.saturating_sub(x + 1),
        &divider_text,
        emphasized(
            Style::default().fg(visual_section_color(&divider.color)),
            divider.emphasis,
        ),
    );
    cells_to_line(cells, None)
}

fn section_line(title: &str, geometry: RenderGeometry) -> Line<'static> {
    let mut cells = blank_cells(geometry.width);
    put_text(
        &mut cells,
        0,
        geometry.width,
        &format!("[{title}]"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    cells_to_line(cells, None)
}

fn stack_label_line(
    app: &App,
    visual_row: usize,
    label: &StackLabelRow,
    geometry: RenderGeometry,
) -> Line<'static> {
    let mut cells = blank_cells(geometry.width);
    let (bits, styles) = rail_bits(app, visual_row, geometry);
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);
    if geometry.lane_overflows(label.lane) {
        paint_overflow_cue(&mut cells, geometry);
    }
    let width = geometry.name_width(label.lane);
    let cursor = match &app.overlay {
        Overlay::StackNameEditor(editor)
            if editor.target == ConfigTarget::Stack(label.stack_id.clone()) =>
        {
            Some(editor.cursor)
        }
        _ => None,
    };
    let count = format!(
        " · {} {}",
        label.branch_count,
        if label.branch_count == 1 {
            "branch"
        } else {
            "branches"
        }
    );
    let name_width = width.saturating_sub(count.chars().count());
    let text = format!(
        "{}{count}",
        inline_editor_text(&label.text, name_width, cursor)
    );
    put_text(
        &mut cells,
        geometry.name_x(label.lane),
        width,
        &text,
        emphasized(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            label.emphasis,
        ),
    );
    if let Some(diff) = app
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.stack_diffs.get(&label.stack_id))
    {
        paint_diff(&mut cells, geometry.diff, diff, label.emphasis);
    }
    let selected = app.selected_label.as_ref()
        == Some(&crate::app::ConfigTarget::Stack(label.stack_id.clone()));
    if selected {
        set_symbol(
            &mut cells,
            1,
            "›",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
        for cell in &mut cells {
            cell.style = cell.style.fg(Color::White);
        }
    }
    let selected_background = selected.then(|| {
        let color = app
            .snapshot
            .as_ref()
            .map(|snapshot| stack_color(&snapshot.repository_id, &label.stack_id, &app.config))
            .unwrap_or(Color::Reset);
        if color == Color::Reset {
            selected_background()
        } else {
            color
        }
    });
    cells_to_line(cells, selected_background)
}

fn divider_line(
    app: &App,
    visual_row: usize,
    divider: &DividerRow,
    geometry: RenderGeometry,
) -> Line<'static> {
    let mut cells = blank_cells(geometry.width);
    let (mut bits, mut styles) = rail_bits(app, visual_row, geometry);
    match divider {
        DividerRow::Connector(connector) => {
            add_connector(app, connector, geometry, &mut bits, &mut styles);
            if geometry.lane_overflows(connector.from_lane)
                || geometry.lane_overflows(connector.to_lane)
            {
                paint_overflow_cue(&mut cells, geometry);
            }
        }
        DividerRow::Placeholder(placeholder) => {
            let x = geometry.lane_x(placeholder.lane);
            if x < geometry.metadata_start {
                bits[x] |= UP | DOWN;
                styles[x] = identity_style(app, &placeholder.stack_id, false, placeholder.emphasis);
            }
            if geometry.lane_overflows(placeholder.lane) {
                paint_overflow_cue(&mut cells, geometry);
            }
            if matches!(app.archive_mode, ArchiveMode::Archive) {
                let style = identity_style(app, &placeholder.stack_id, false, placeholder.emphasis)
                    .add_modifier(Modifier::DIM);
                if x < geometry.metadata_start {
                    set_symbol(&mut cells, x, "○", style);
                }
                put_text(
                    &mut cells,
                    geometry.name_x(placeholder.lane),
                    geometry.name_width(placeholder.lane),
                    placeholder.branch.0.as_ref(),
                    style,
                );
            }
        }
        DividerRow::Spacer { .. } => {}
    }
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);
    cells_to_line(cells, None)
}

fn branch_line(
    app: &App,
    branch: &Branch,
    row: &ProjectedRow,
    visual_row: usize,
    now: SystemTime,
    geometry: RenderGeometry,
) -> Line<'static> {
    let selected = app.selected_label.is_none() && app.selected.as_ref() == Some(&branch.id);
    let background = if selected {
        let accent = if row.is_trunk {
            trunk_color()
        } else {
            let repository_id = app
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.repository_id.as_ref())
                .unwrap_or_default();
            stack_color(repository_id, &row.stack_id, &app.config)
        };
        Some(if accent == Color::Reset {
            selected_background()
        } else {
            accent
        })
    } else if branch.current {
        let accent = if row.is_trunk {
            trunk_color()
        } else {
            let repository_id = app
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.repository_id.as_ref())
                .unwrap_or_default();
            stack_color(repository_id, &row.stack_id, &app.config)
        };
        Some(current_background(accent))
    } else {
        None
    };
    let mut cells = blank_cells(geometry.width);
    let (bits, styles) = rail_bits(app, visual_row, geometry);
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);

    let mut identity = identity_style(app, &row.stack_id, row.is_trunk, row.emphasis);
    if selected {
        identity = if background == Some(selected_background()) {
            identity.add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        };
    }
    if row.context_only {
        identity = identity.add_modifier(Modifier::DIM);
    }

    if row.is_trunk {
        set_symbol(
            &mut cells,
            0,
            if branch.current { "◉" } else { "○" },
            identity.add_modifier(Modifier::BOLD),
        );
    } else {
        if branch.current {
            set_symbol(&mut cells, 0, "●", identity);
        }
        let lane_x = geometry.lane_x(row.lane);
        if lane_x < geometry.metadata_start {
            set_symbol(&mut cells, lane_x, "○", identity);
        }
        if geometry.lane_overflows(row.lane) {
            paint_overflow_cue(&mut cells, geometry);
        }
    }
    if selected {
        set_symbol(&mut cells, 1, "›", identity);
    }
    if app.archive_range_contains(&branch.id) {
        set_symbol(
            &mut cells,
            1,
            "■",
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        );
    }

    let desired_name_x = geometry
        .name_x(row.lane)
        .saturating_add(row.manual_depth.saturating_mul(2));
    let name_x = desired_name_x.min(geometry.metadata_start.saturating_sub(2));
    let name_width = geometry
        .metadata_start
        .saturating_sub(name_x.saturating_add(1));
    let dirty = if branch.dirty { "*" } else { "" };
    let branch_name = if name_x < desired_name_x {
        format!("{} {}{dirty}", row.manual_depth, branch.id)
    } else {
        format!("{}{dirty}", branch.id)
    };
    put_text(
        &mut cells,
        name_x,
        name_width,
        &branch_name,
        if selected {
            identity
        } else if let Some(color) = &row.visual_color {
            emphasized(
                Style::default().fg(visual_section_color(color)),
                row.emphasis,
            )
        } else {
            identity
        },
    );

    let metadata_emphasis = row.emphasis;
    if matches!(app.archive_mode, ArchiveMode::Archive) {
        paint_archive_evidence(&mut cells, branch, geometry, metadata_emphasis);
    } else {
        if let Some(range) = geometry.time {
            put_right(
                &mut cells,
                range,
                &relative_time(branch.committed_at, now),
                emphasized(Style::default().fg(Color::Gray), metadata_emphasis),
            );
        }
        paint_diff(&mut cells, geometry.diff, &branch.diff, metadata_emphasis);
        paint_worktree(
            &mut cells,
            geometry.worktree,
            branch.worktree.as_deref(),
            metadata_emphasis,
        );
        if let Some(range) = geometry.remote {
            paint_remote_status(&mut cells, branch, range, metadata_emphasis);
        }
        if let Some(range) = geometry.pr
            && let Some(pull_request) = &branch.pr
        {
            paint_pull_request(&mut cells, range, pull_request, metadata_emphasis);
        }
    }

    if selected {
        for cell in &mut cells {
            if !cell.preserve_foreground {
                cell.style = cell.style.fg(Color::Black);
            }
        }
    }

    cells_to_line(cells, background)
}

fn paint_archive_evidence(
    cells: &mut [RenderCell],
    branch: &Branch,
    geometry: RenderGeometry,
    emphasis: Emphasis,
) {
    let range = ColumnRange {
        x: geometry.metadata_start,
        width: geometry.width.saturating_sub(geometry.metadata_start),
    };
    let compact = range.width < 20;
    let mut badges = Vec::with_capacity(2);
    if let Some(path) = branch.worktree.as_deref() {
        badges.push(if compact {
            "⎇".to_owned()
        } else {
            let basename = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("worktree");
            format!("⎇ {}", truncate(basename, 10))
        });
    }
    let (remote, color) = remote_status(branch, compact);
    badges.push(remote);
    let label = truncate(&badges.join(" "), range.width);
    put_text(
        cells,
        range.x,
        range.width,
        &label,
        emphasized(Style::default().fg(color), emphasis),
    );
}

fn paint_remote_status(
    cells: &mut [RenderCell],
    branch: &Branch,
    range: ColumnRange,
    emphasis: Emphasis,
) {
    let (label, color) = remote_status(branch, range.width < 10);
    put_right(
        cells,
        range,
        &truncate(&label, range.width),
        emphasized(Style::default().fg(color), emphasis),
    );
}

fn remote_status(branch: &Branch, compact: bool) -> (String, Color) {
    let label = match &branch.configured_upstream {
        ConfiguredUpstream::None => match &branch.remote_ref {
            RemoteRefEvidence::Contained { .. } => {
                if compact {
                    "✓".into()
                } else {
                    "✓ pushed".into()
                }
            }
            RemoteRefEvidence::Checking => {
                if compact {
                    "…".into()
                } else {
                    "… checking".into()
                }
            }
            RemoteRefEvidence::LocalOnly { .. } | RemoteRefEvidence::NotRequested => {
                if compact {
                    "○".into()
                } else {
                    "○ no remote".into()
                }
            }
            RemoteRefEvidence::Unavailable { .. } => {
                if compact {
                    "?".into()
                } else {
                    "? remote".into()
                }
            }
        },
        ConfiguredUpstream::Equal { .. } => {
            if compact {
                "✓".into()
            } else {
                "✓ pushed".into()
            }
        }
        ConfiguredUpstream::Ahead { ahead, .. } => {
            if compact {
                format!("↑{ahead}")
            } else {
                format!("↑{ahead} ahead")
            }
        }
        ConfiguredUpstream::Behind { behind, .. } => {
            if compact {
                format!("↓{behind}")
            } else {
                format!("↓{behind} behind")
            }
        }
        ConfiguredUpstream::Diverged { ahead, behind, .. } => {
            if compact {
                format!("↕{ahead}/{behind}")
            } else {
                format!("↕{ahead}/{behind} div")
            }
        }
        ConfiguredUpstream::Gone { .. } => {
            if compact {
                "×".into()
            } else {
                "× gone".into()
            }
        }
        ConfiguredUpstream::Unavailable { .. } => {
            if compact {
                "?".into()
            } else {
                "? remote".into()
            }
        }
    };
    let color = match &branch.configured_upstream {
        ConfiguredUpstream::Equal { .. } => Color::Green,
        ConfiguredUpstream::Ahead { .. } | ConfiguredUpstream::None => match &branch.remote_ref {
            RemoteRefEvidence::Contained { .. } => Color::Green,
            RemoteRefEvidence::Checking | RemoteRefEvidence::Unavailable { .. } => Color::DarkGray,
            _ => Color::Yellow,
        },
        ConfiguredUpstream::Behind { .. } => Color::Cyan,
        ConfiguredUpstream::Diverged { .. } | ConfiguredUpstream::Gone { .. } => Color::Red,
        ConfiguredUpstream::Unavailable { .. } => Color::DarkGray,
    };
    (label, color)
}

fn rail_bits(app: &App, visual_row: usize, geometry: RenderGeometry) -> (Vec<u8>, Vec<Style>) {
    let mut bits = vec![0; geometry.graph_buffer_width()];
    let mut styles = vec![Style::default(); geometry.graph_buffer_width()];
    for lane in 0..=geometry
        .last_visible_lane
        .min(app.projection.lane_count.saturating_sub(1))
    {
        let x = geometry.lane_x(lane);
        let Some(span) = app.projection.active_lane_span(lane, visual_row) else {
            continue;
        };
        if visual_row > span.start {
            bits[x] |= UP;
        }
        if visual_row < span.end {
            bits[x] |= DOWN;
        }
        styles[x] = identity_style(
            app,
            &span.stack_id,
            app.projection
                .row_for(&span.stack_id)
                .is_some_and(|row| row.is_trunk),
            app.projection.emphasis_for(&span.stack_id),
        );
    }
    (bits, styles)
}

fn add_connector(
    app: &App,
    connector: &ConnectorRow,
    geometry: RenderGeometry,
    bits: &mut [u8],
    styles: &mut [Style],
) {
    let left = geometry.lane_x(connector.to_lane);
    let right = geometry.lane_x(connector.from_lane);
    if bits.is_empty() || left >= bits.len() {
        return;
    }
    let right = right.min(bits.len().saturating_sub(1));
    let child_style = identity_style(app, &connector.stack_id, false, connector.emphasis);
    let parent_style = connector
        .parent
        .as_ref()
        .and_then(|parent| app.projection.row_for(parent))
        .map(|row| identity_style(app, &row.stack_id, row.is_trunk, row.emphasis))
        .unwrap_or(styles[left]);
    for x in left..=right {
        if x > left {
            bits[x] |= LEFT;
        }
        if x < right {
            bits[x] |= RIGHT;
        }
        styles[x] = if x == left { parent_style } else { child_style };
    }
}

fn identity_style(app: &App, stack_id: &BranchId, is_trunk: bool, emphasis: Emphasis) -> Style {
    let repository_id = app
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.repository_id.as_ref())
        .unwrap_or_default();
    let style = if is_trunk {
        Style::default()
            .fg(trunk_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(stack_color(repository_id, stack_id, &app.config))
    };
    emphasized(style, emphasis)
}

fn emphasized(style: Style, emphasis: Emphasis) -> Style {
    match emphasis {
        Emphasis::Full => style,
        Emphasis::Dim | Emphasis::Hidden => style.add_modifier(Modifier::DIM),
    }
}

fn paint_diff(cells: &mut [RenderCell], range: ColumnRange, diff: &DiffState, emphasis: Emphasis) {
    match diff {
        DiffState::Loading => put_right(
            cells,
            range,
            "…loading",
            emphasized(Style::default().fg(Color::DarkGray), emphasis),
        ),
        DiffState::Unavailable(_) => put_right(
            cells,
            range,
            "+? -?",
            emphasized(Style::default().fg(Color::DarkGray), emphasis),
        ),
        DiffState::Ready(stat) => {
            let left_width = range.width / 2;
            let right_width = range.width - left_width;
            put_right(
                cells,
                ColumnRange {
                    x: range.x,
                    width: left_width,
                },
                &format!(
                    "+{}",
                    compact_count(stat.insertions, left_width.saturating_sub(1))
                ),
                emphasized(Style::default().fg(Color::Green), emphasis),
            );
            put_right(
                cells,
                ColumnRange {
                    x: range.x + left_width,
                    width: right_width,
                },
                &format!(
                    "-{}",
                    compact_count(stat.deletions, right_width.saturating_sub(1))
                ),
                emphasized(Style::default().fg(Color::Red), emphasis),
            );
        }
    }
}

fn paint_pull_request(
    cells: &mut [RenderCell],
    range: ColumnRange,
    pull_request: &PullRequest,
    emphasis: Emphasis,
) {
    match pull_request.status {
        PullRequestStatus::Open => {
            put_right(
                cells,
                range,
                &pull_request.status.column_text(pull_request.number),
                emphasized(Style::default().fg(Color::Yellow), emphasis),
            );
        }
        PullRequestStatus::Approved => {
            let combined = pull_request.status.column_text(pull_request.number);
            let visible = truncate(&combined, range.width);
            let visible_count = visible.chars().count();
            let start_x = range.x + range.width.saturating_sub(visible_count);
            let check_style = emphasized(Style::default().fg(Color::Green), emphasis);
            let number_style = emphasized(Style::default().fg(Color::Yellow), emphasis);
            for (offset, character) in visible.chars().enumerate() {
                let position = start_x + offset;
                match character {
                    '✓' => write_symbol(cells, position, "✓", check_style, true),
                    _ => write_symbol(cells, position, &character.to_string(), number_style, false),
                }
            }
        }
        PullRequestStatus::Merged | PullRequestStatus::Closed => {
            put_right(
                cells,
                range,
                &pull_request.status.column_text(pull_request.number),
                emphasized(Style::default().fg(Color::DarkGray), emphasis),
            );
        }
    }
}

fn paint_worktree(
    cells: &mut [RenderCell],
    range: ColumnRange,
    worktree: Option<&Path>,
    emphasis: Emphasis,
) {
    let Some(worktree) = worktree else {
        return;
    };
    let label = if range.width <= 2 {
        "⎇".to_owned()
    } else {
        let basename = worktree
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| worktree.to_str().unwrap_or("worktree"));
        format!("⎇ {basename}")
    };
    put_text(
        cells,
        range.x,
        range.width,
        &label,
        emphasized(Style::default().fg(Color::Cyan), emphasis),
    );
}

fn blank_cells(width: usize) -> Vec<RenderCell> {
    vec![RenderCell::default(); width]
}

fn set_symbol(cells: &mut [RenderCell], x: usize, symbol: &str, style: Style) {
    write_symbol(cells, x, symbol, style, false);
}

fn write_symbol(
    cells: &mut [RenderCell],
    x: usize,
    symbol: &str,
    style: Style,
    preserve_foreground: bool,
) {
    if let Some(cell) = cells.get_mut(x) {
        cell.symbol.clear();
        cell.symbol.push_str(symbol);
        cell.style = style;
        cell.preserve_foreground = preserve_foreground;
    }
}

fn put_text(cells: &mut [RenderCell], x: usize, width: usize, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width).enumerate() {
        set_symbol(cells, x + offset, &character.to_string(), style);
    }
}

fn put_right(cells: &mut [RenderCell], range: ColumnRange, text: &str, style: Style) {
    let text = truncate(text, range.width);
    let count = text.chars().count();
    put_text(
        cells,
        range.x + range.width.saturating_sub(count),
        range.width,
        &text,
        style,
    );
}

fn cells_to_line(cells: Vec<RenderCell>, background: Option<Color>) -> Line<'static> {
    Line::from(
        cells
            .into_iter()
            .map(|cell| {
                let style = background.map_or(cell.style, |background| cell.style.bg(background));
                Span::styled(cell.symbol, style)
            })
            .collect::<Vec<_>>(),
    )
}

fn compact_count(value: u64, width: usize) -> String {
    let raw = value.to_string();
    if value < 1_000 && raw.len() <= width {
        return raw;
    }
    for (divisor, suffix) in [(1_000_000_000, "G"), (1_000_000, "M"), (1_000, "K")] {
        if value >= divisor {
            if value < divisor * 10 {
                let decimal = (value % divisor) / (divisor / 10);
                let compact = format!("{}.{decimal}{suffix}", value / divisor);
                if compact.len() <= width {
                    return compact;
                }
            }
            let compact = format!("{}{suffix}", value / divisor);
            if compact.len() <= width {
                return compact;
            }
        }
    }
    if raw.len() <= width {
        return raw;
    }
    "?".into()
}

fn truncate(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut output: String = value.chars().take(width - 1).collect();
    output.push('…');
    output
}

fn inline_editor_text(value: &str, width: usize, cursor: Option<usize>) -> String {
    let Some(cursor) = cursor else {
        return truncate(value, width);
    };
    if width == 0 {
        return String::new();
    }
    let characters = value.chars().collect::<Vec<_>>();
    let cursor = cursor.min(characters.len());
    let available = width.saturating_sub(1);
    let start = cursor.saturating_sub(available);
    let mut visible = characters[start..cursor].iter().collect::<String>();
    visible.push('▏');
    visible.extend(
        characters[cursor..]
            .iter()
            .take(width.saturating_sub(visible.chars().count())),
    );
    visible
}
