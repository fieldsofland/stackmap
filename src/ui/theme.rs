use std::hash::{Hash, Hasher};

use ratatui::style::Color;

use crate::config::Config;
use crate::model::BranchId;

#[cfg(test)]
thread_local! {
    static TEST_NO_COLOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn no_color_requested() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return true;
    }
    #[cfg(test)]
    if TEST_NO_COLOR.with(std::cell::Cell::get) {
        return true;
    }
    false
}

#[cfg(test)]
pub(crate) fn with_no_color<T>(operation: impl FnOnce() -> T) -> T {
    TEST_NO_COLOR.with(|flag| {
        let previous = flag.replace(true);
        let result = operation();
        flag.set(previous);
        result
    })
}

#[cfg(test)]
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
    if no_color_requested() {
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
    if no_color_requested() {
        Color::Reset
    } else {
        Color::Rgb(TRUNK_RGB.0, TRUNK_RGB.1, TRUNK_RGB.2)
    }
}

pub fn selected_background() -> Color {
    Color::Rgb(52, 68, 92)
}

pub fn current_background(accent: Color) -> Color {
    match accent {
        Color::Rgb(red, green, blue) => Color::Rgb(
            (red as f32 * 0.4).round() as u8,
            (green as f32 * 0.4).round() as u8,
            (blue as f32 * 0.4).round() as u8,
        ),
        _ => Color::Rgb(31, 38, 52),
    }
}

fn parse_hex(value: &str) -> Option<Color> {
    Some(Color::Rgb(
        u8::from_str_radix(value.get(1..3)?, 16).ok()?,
        u8::from_str_radix(value.get(3..5)?, 16).ok()?,
        u8::from_str_radix(value.get(5..7)?, 16).ok()?,
    ))
}

pub fn visual_section_color(value: &str) -> Color {
    if no_color_requested() {
        Color::Reset
    } else {
        parse_hex(value).unwrap_or(Color::Reset)
    }
}
