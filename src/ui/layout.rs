use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::LanePitch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WidthMode {
    TooNarrow,
    Narrow,
    Medium,
    Wide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColumnRange {
    pub x: usize,
    pub width: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderGeometry {
    pub width: usize,
    pub mode: WidthMode,
    pub effective_pitch: usize,
    pub last_visible_lane: usize,
    pub graph_max_x: usize,
    pub metadata_start: usize,
    pub time: Option<ColumnRange>,
    pub diff: ColumnRange,
    pub worktree: ColumnRange,
    pub remote: Option<ColumnRange>,
    pub pr: Option<ColumnRange>,
    pub stack_health: Option<ColumnRange>,
}

impl RenderGeometry {
    #[cfg(test)]
    pub fn new(width: u16, mode: WidthMode, requested: LanePitch, lane_count: usize) -> Self {
        Self::new_with_status(width, mode, requested, lane_count, true)
    }

    pub fn new_with_status(
        width: u16,
        mode: WidthMode,
        requested: LanePitch,
        lane_count: usize,
        status_visible: bool,
    ) -> Self {
        const COLUMN_GAP: usize = 2;
        let width = width as usize;
        let wide_worktree = mode != WidthMode::Narrow;
        let show_time = width >= 56;
        let show_full_status = width >= 100;
        let time_width = usize::from(show_time) * 6;
        let time_diff_gap = usize::from(show_time) * COLUMN_GAP;
        let diff_width = 10;
        let diff_worktree_gap = COLUMN_GAP;
        let worktree_width = if wide_worktree { 18 } else { 2 };
        let status_width = usize::from(status_visible) * if show_full_status { 12 } else { 8 };
        let status_gap = usize::from(status_visible) * COLUMN_GAP;
        let status_edge_gap = usize::from(status_visible);
        let metadata_width = time_width
            + time_diff_gap
            + diff_width
            + diff_worktree_gap
            + worktree_width
            + status_gap
            + status_width
            + status_edge_gap;
        let metadata_start = width.saturating_sub(metadata_width);
        let mut cursor = metadata_start;
        let time = show_time.then(|| {
            let range = ColumnRange {
                x: cursor,
                width: time_width,
            };
            cursor += time_width + time_diff_gap;
            range
        });
        let diff = ColumnRange {
            x: cursor,
            width: diff_width,
        };
        cursor += diff_width + diff_worktree_gap;
        let worktree = ColumnRange {
            x: cursor,
            width: worktree_width,
        };
        cursor += worktree_width;
        let pr = status_visible.then(|| {
            cursor += COLUMN_GAP;
            let range = ColumnRange {
                x: cursor,
                width: status_width,
            };
            cursor += status_width;
            range
        });
        let remote = None;
        let stack_health = None;

        let automatic = match mode {
            WidthMode::TooNarrow | WidthMode::Narrow => 2,
            WidthMode::Medium => 3,
            WidthMode::Wide => 4,
        };
        const MINIMUM_NAME_WIDTH: usize = 8;
        let maximum_center = metadata_start.saturating_sub(MINIMUM_NAME_WIDTH + 3);
        let maximum_pitch = maximum_center.saturating_sub(3).max(1);
        let effective_pitch = match requested {
            LanePitch::Auto => automatic.min(maximum_pitch),
            LanePitch::Fixed(requested) => (requested as usize).min(maximum_pitch).max(1),
        };
        let maximum_lane = lane_count.saturating_sub(1);
        let last_visible_lane = if maximum_lane == 0 || maximum_center < 3 {
            0
        } else {
            (1 + maximum_center.saturating_sub(3) / effective_pitch).min(maximum_lane)
        };
        let graph_max_x = match last_visible_lane {
            0 => 0,
            lane => 3 + (lane - 1) * effective_pitch,
        };

        Self {
            width,
            mode,
            effective_pitch,
            last_visible_lane,
            graph_max_x,
            metadata_start,
            time,
            diff,
            worktree,
            remote,
            pr,
            stack_health,
        }
    }

    pub fn lane_x(self, lane: usize) -> usize {
        match lane.min(self.last_visible_lane) {
            0 => 0,
            lane => 3 + (lane - 1) * self.effective_pitch,
        }
    }

    pub fn name_x(self, lane: usize) -> usize {
        self.lane_x(lane) + 2
    }

    pub fn name_width(self, lane: usize) -> usize {
        self.metadata_start
            .saturating_sub(self.name_x(lane).saturating_add(1))
    }

    pub fn lane_overflows(self, lane: usize) -> bool {
        lane > self.last_visible_lane
    }

    pub fn overflow_cue_x(self) -> usize {
        self.graph_max_x.saturating_add(1)
    }

    pub fn graph_buffer_width(self) -> usize {
        self.graph_max_x.saturating_add(1)
    }
}

pub struct Areas {
    pub header: Rect,
    pub body: Rect,
    pub detail: Option<Rect>,
    pub footer: Rect,
    pub mode: WidthMode,
}

pub fn width_mode(width: u16) -> WidthMode {
    match width {
        0..=39 => WidthMode::TooNarrow,
        40..=89 => WidthMode::Narrow,
        90..=119 => WidthMode::Medium,
        _ => WidthMode::Wide,
    }
}

pub fn areas(area: Rect, show_detail: bool) -> Areas {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    let mode = width_mode(area.width);
    let (body, detail) = if mode == WidthMode::Wide && show_detail {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(chunks[1]);
        (columns[0], Some(columns[1]))
    } else {
        (chunks[1], None)
    };
    Areas {
        header: chunks[0],
        body,
        detail,
        footer: chunks[2],
        mode,
    }
}
