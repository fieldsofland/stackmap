use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::model::topology::{ConnectorRow, DividerRow, Emphasis, ProjectedRow, ProjectionEntry};
use crate::model::{Branch, BranchId, DiffState};

use super::layout::{ColumnRange, RenderGeometry, WidthMode, width_mode};
use super::theme::{current_background, selected_background, stack_color, trunk_color};

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

#[derive(Clone, Debug)]
struct RenderCell {
    symbol: String,
    style: Style,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            symbol: " ".into(),
            style: Style::default(),
        }
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &App, now: SystemTime) {
    render_with_mode(frame, area, app, now, width_mode(area.width));
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
                "No local branches"
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
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y, area.width, scroll_height),
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
        }
        DividerRow::Placeholder(placeholder) => {
            let x = geometry.lane_x(placeholder.lane);
            if x < geometry.metadata_start {
                bits[x] |= UP | DOWN;
                styles[x] = identity_style(app, &placeholder.stack_id, false, placeholder.emphasis);
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
    let selected = app.selected.as_ref() == Some(&branch.id);
    let background = if selected {
        Some(selected_background())
    } else if branch.current {
        Some(current_background())
    } else {
        None
    };
    let mut cells = blank_cells(geometry.width);
    let (bits, styles) = rail_bits(app, visual_row, geometry);
    paint_bits(&mut cells, &bits, &styles, geometry.metadata_start);

    let mut identity = identity_style(app, &row.stack_id, row.is_trunk, row.emphasis);
    if selected {
        identity = identity.add_modifier(Modifier::BOLD);
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
    }
    if selected {
        set_symbol(&mut cells, 1, "›", identity);
    }

    let name_x = geometry.name_x(row.lane);
    let name_width = geometry.name_width(row.lane);
    let dirty = if branch.dirty { "*" } else { "" };
    put_text(
        &mut cells,
        name_x,
        name_width,
        &format!("{}{dirty}", branch.id),
        identity,
    );

    let metadata_emphasis = row.emphasis;
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
    if let Some(range) = geometry.pr
        && let Some(pr) = &branch.pr
    {
        put_right(
            &mut cells,
            range,
            &format!("#{}", pr.number),
            emphasized(Style::default().fg(Color::Yellow), metadata_emphasis),
        );
    }

    cells_to_line(cells, background)
}

fn rail_bits(app: &App, visual_row: usize, geometry: RenderGeometry) -> (Vec<u8>, Vec<Style>) {
    let mut bits = vec![0; geometry.metadata_start];
    let mut styles = vec![Style::default(); geometry.metadata_start];
    for lane in 0..app.projection.lane_count {
        let x = geometry.lane_x(lane);
        if x >= geometry.metadata_start {
            break;
        }
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
    if left >= geometry.metadata_start {
        return;
    }
    let right = right.min(geometry.metadata_start.saturating_sub(1));
    let style = identity_style(app, &connector.stack_id, false, connector.emphasis);
    for x in left..=right {
        if x > left {
            bits[x] |= LEFT;
        }
        if x < right {
            bits[x] |= RIGHT;
        }
        styles[x] = style;
    }
}

fn paint_bits(cells: &mut [RenderCell], bits: &[u8], styles: &[Style], limit: usize) {
    for x in 0..limit.min(cells.len()).min(bits.len()) {
        if let Some(glyph) = bit_glyph(bits[x]) {
            set_symbol(cells, x, glyph, styles[x]);
        }
    }
}

fn bit_glyph(bits: u8) -> Option<&'static str> {
    match bits & 15 {
        0 => None,
        15 => Some("┼"),
        11 => Some("├"),
        7 => Some("┤"),
        14 => Some("┬"),
        13 => Some("┴"),
        10 => Some("┌"),
        6 => Some("┐"),
        9 => Some("└"),
        5 => Some("┘"),
        12 | 4 | 8 => Some("─"),
        3 | 1 | 2 => Some("│"),
        _ => Some("┼"),
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
    if let Some(cell) = cells.get_mut(x) {
        cell.symbol.clear();
        cell.symbol.push_str(symbol);
        cell.style = style;
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
    if raw.len() <= width {
        return raw;
    }
    for (divisor, suffix) in [(1_000_000_000, "G"), (1_000_000, "M"), (1_000, "K")] {
        if value >= divisor {
            let compact = format!("{}{suffix}", value / divisor);
            if compact.len() <= width {
                return compact;
            }
        }
    }
    "?".into()
}

pub fn relative_time(timestamp: i64, now: SystemTime) -> String {
    let now = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let seconds = now - timestamp;
    if seconds < -60 {
        return "clock?".into();
    }
    if seconds < 60 {
        "now".into()
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 172_800 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

pub fn exact_time(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|value| {
            value
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        })
        .unwrap_or_else(|| "unavailable".into())
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

pub fn detail(branch: &Branch) -> String {
    let pr = branch
        .pr
        .as_ref()
        .map(|pr| format!("PR #{} {}", pr.number, pr.title))
        .unwrap_or_else(|| "No matching open PR".into());
    let worktree = branch
        .worktree
        .as_ref()
        .map(|path| format!("Worktree: {}", path.display()))
        .unwrap_or_else(|| "Worktree: not checked out".into());
    format!(
        "{}\nLast edited: {}\n{}\n{}",
        branch.id,
        exact_time(branch.committed_at),
        worktree,
        pr
    )
}
