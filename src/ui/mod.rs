pub mod layout;
pub mod panels;
pub mod theme;
pub mod tree;

use std::time::SystemTime;

use ratatui::Frame;

use crate::app::{App, MutationState, Overlay};

pub fn render(frame: &mut Frame<'_>, app: &mut App, now: SystemTime) {
    let areas = layout::areas(frame.area(), app.detail_sidebar);
    panels::header(frame, areas.header, app);
    app.set_viewport_height(areas.body.height as usize);
    tree::render_with_mode(frame, areas.body, app, now, areas.mode);
    if let Some(detail) = areas.detail {
        panels::detail(frame, detail, app);
    }
    panels::footer(frame, areas.footer, app);
    if matches!(app.mutation, MutationState::ConfirmingCheckout(_)) {
        panels::checkout_confirmation(frame, app);
        return;
    }
    if matches!(app.mutation, MutationState::ConfirmingDeletion(_)) {
        panels::deletion_confirmation(frame, app);
        return;
    }
    if matches!(app.mutation, MutationState::ConfirmingRestack(_)) {
        panels::restack_confirmation(frame, app);
        return;
    }
    if matches!(app.mutation, MutationState::ConfirmingMove(_)) {
        panels::move_confirmation(frame, app);
        return;
    }
    match &app.overlay {
        Overlay::Help => panels::help(frame, app),
        Overlay::OrderPicker(_) => panels::order_picker(frame, app),
        Overlay::ColorPicker(_) => panels::color_picker(frame, app),
        Overlay::None
        | Overlay::Search
        | Overlay::StackNameEditor(_)
        | Overlay::ArchiveRange(_)
        | Overlay::MovePreview(_) => {}
    }
}
