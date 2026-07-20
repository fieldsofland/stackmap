use ratatui::style::{Color, Modifier, Style};

use super::super::layout::RenderGeometry;
use super::{RenderCell, set_symbol};

pub(super) fn paint_bits(cells: &mut [RenderCell], bits: &[u8], styles: &[Style], limit: usize) {
    for x in 0..limit.min(cells.len()).min(bits.len()) {
        if let Some(glyph) = bit_glyph(bits[x]) {
            set_symbol(cells, x, glyph, styles[x]);
        }
    }
}

pub(super) fn paint_overflow_cue(cells: &mut [RenderCell], geometry: RenderGeometry) {
    set_symbol(
        cells,
        geometry.overflow_cue_x(),
        "»",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
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
        1..=3 => Some("│"),
        _ => Some("┼"),
    }
}
