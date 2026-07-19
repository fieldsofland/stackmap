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
    pub metadata_start: usize,
    pub time: Option<ColumnRange>,
    pub diff: ColumnRange,
    pub worktree: ColumnRange,
    pub pr: Option<ColumnRange>,
}

impl RenderGeometry {
    pub fn new(width: u16, mode: WidthMode, requested: LanePitch, lane_count: usize) -> Self {
        let width = width as usize;
        let wide_worktree = mode != WidthMode::Narrow;
        let show_time = width >= 56;
        let show_pr = width >= 72;
        let time_width = usize::from(show_time) * 6;
        let diff_width = 9;
        let worktree_width = if wide_worktree { 18 } else { 2 };
        let pr_width = usize::from(show_pr) * 8;
        let metadata_width = time_width + diff_width + worktree_width + pr_width;
        let metadata_start = width.saturating_sub(metadata_width);
        let mut cursor = metadata_start;
        let time = show_time.then(|| {
            let range = ColumnRange {
                x: cursor,
                width: time_width,
            };
            cursor += time_width;
            range
        });
        let diff = ColumnRange {
            x: cursor,
            width: diff_width,
        };
        cursor += diff_width;
        let worktree = ColumnRange {
            x: cursor,
            width: worktree_width,
        };
        cursor += worktree_width;
        let pr = show_pr.then_some(ColumnRange {
            x: cursor,
            width: pr_width,
        });

        let automatic = match mode {
            WidthMode::TooNarrow | WidthMode::Narrow => 2,
            WidthMode::Medium => 3,
            WidthMode::Wide => 4,
        };
        let effective_pitch = match requested {
            LanePitch::Auto => automatic,
            LanePitch::Fixed(requested) => {
                let denominator = lane_count.saturating_sub(2);
                let maximum = if denominator == 0 {
                    requested as usize
                } else {
                    metadata_start
                        .saturating_sub(6)
                        .checked_div(denominator)
                        .unwrap_or(1)
                        .max(1)
                };
                (requested as usize).min(maximum).max(1)
            }
        };

        Self {
            width,
            mode,
            effective_pitch,
            metadata_start,
            time,
            diff,
            worktree,
            pr,
        }
    }

    pub fn lane_x(self, lane: usize) -> usize {
        match lane {
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

pub fn areas(area: Rect) -> Areas {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    let mode = width_mode(area.width);
    let (body, detail) = if mode == WidthMode::Wide {
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
