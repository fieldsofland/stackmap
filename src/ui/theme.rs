use std::hash::{Hash, Hasher};

use ratatui::style::Color;

use crate::config::Config;
use crate::model::BranchId;

pub const TRUNK_COLOR_HEX: &str = "#e0af68";
const TRUNK_RGB: (u8, u8, u8) = (224, 175, 104);

const PALETTE: &[(u8, u8, u8)] = &[
    (122, 162, 247),
    (187, 154, 247),
    (125, 207, 255),
    (255, 158, 100),
    (158, 206, 106),
    (247, 118, 142),
    (42, 195, 222),
    (192, 202, 245),
];

pub fn stack_color(repository_id: &str, root: &BranchId, config: &Config) -> Color {
    if std::env::var_os("NO_COLOR").is_some() {
        return Color::Reset;
    }
    if let Some(value) = config.color(root).and_then(parse_hex)
        && value != trunk_color()
    {
        return value;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    repository_id.hash(&mut hasher);
    root.hash(&mut hasher);
    let (red, green, blue) = PALETTE[hasher.finish() as usize % PALETTE.len()];
    Color::Rgb(red, green, blue)
}

pub fn trunk_color() -> Color {
    if std::env::var_os("NO_COLOR").is_some() {
        Color::Reset
    } else {
        Color::Rgb(TRUNK_RGB.0, TRUNK_RGB.1, TRUNK_RGB.2)
    }
}

pub fn selected_background() -> Color {
    Color::Rgb(52, 68, 92)
}

pub fn current_background() -> Color {
    Color::Rgb(31, 38, 52)
}

fn parse_hex(value: &str) -> Option<Color> {
    Some(Color::Rgb(
        u8::from_str_radix(value.get(1..3)?, 16).ok()?,
        u8::from_str_radix(value.get(3..5)?, 16).ok()?,
        u8::from_str_radix(value.get(5..7)?, 16).ok()?,
    ))
}
