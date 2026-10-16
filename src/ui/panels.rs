use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, COLOR_OPTIONS, MutationState, Overlay};
use crate::model::topology::{ArchiveMode, OrderMode};

pub fn header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let title = app
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.root.display().to_string())
        .unwrap_or_else(|| "stackmap".into());
    let status = if app.refresh_error.is_some() {
        "stale"
    } else if app.loading {
        "reading"
    } else if app.checkout_running() {
        "switching"
    } else {
        "live"
    };
    let archived = matches!(app.archive_mode, ArchiveMode::Archive);
    let selected_view = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::raw(" stackmap  "),
        Span::styled(
            if archived { "Active" } else { "[Active]" },
            if archived {
                Style::default()
            } else {
                selected_view
            },
        ),
        Span::raw(" | "),
        Span::styled(
            if archived { "[Archive]" } else { "Archive" },
            if archived {
                selected_view
            } else {
                Style::default()
            },
        ),
    ];
    spans.push(Span::raw(format!(" {title}  ")));
    spans.push(Span::styled(status, Style::default().fg(Color::Cyan)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let archived = matches!(app.archive_mode, ArchiveMode::Archive);
    let base = format!(
        "? Actions  a View {}  x {}",
        if archived { "Active" } else { "Archive" },
        if archived { "Restore" } else { "Archive" }
    );
    let controls = match &app.overlay {
        Overlay::Search => format!("/{}  Enter apply  Esc cancel", app.filter),
        Overlay::StackNameEditor(_) => "Enter save  Ctrl-U clear  Esc cancel".into(),
        Overlay::ArchiveRange(range) => format!(
            "Range {} {} branches  {} → {}  ↑↓ select  Enter {}  Esc cancel",
            if archived { "Restore" } else { "Archive" },
            range.branch_count(),
            range.anchor,
            range.endpoint,
            if archived { "restore" } else { "archive" }
        ),
        Overlay::MovePreview(preview) => format!(
            "Move preview {} → {}  Tab {}  Enter confirm  Esc cancel",
            preview.source,
            preview.target,
            if preview.only {
                "branch only"
            } else {
                "subtree"
            }
        ),
        _ if area.width >= 80 => format!("{base}  / Filter  d Details  R Refresh"),
        _ => base,
    };
    let lookup_detail =
        if let Some(error) = &app.refresh_error {
            format!("Stale: {error}")
        } else if let Some(progress) = app.mutation_progress() {
            progress.to_string()
        } else if let Some(message) = &app.message {
            message.to_string()
        } else if let Some(notice) = app.notice() {
            notice.to_string()
        } else if let Some(branch) = app.selected_branch() {
            match &branch.pr_lookup {
                crate::model::PullRequestLookup::NotRequested => {
                    "PR ? Not checked · p Check selected".into()
                }
                crate::model::PullRequestLookup::Checking => "PR … Checking selected branch".into(),
                crate::model::PullRequestLookup::Unavailable(_) => {
                    "PR ! Unavailable · p Retry · d Details".into()
                }
                crate::model::PullRequestLookup::NoMatch => "PR: no match found".into(),
                crate::model::PullRequestLookup::Ready => {
                    if branch.pr.as_ref().is_some_and(|pr| {
                        pr.match_quality == crate::model::PullRequestMatch::StaleTip
                    }) {
                        "PR ~ Head differs · d Details".into()
                    } else {
                        String::new()
                    }
                }
            }
        } else {
            String::new()
        };
    let mut notices = Vec::new();
    if let Some(error) = &app.refresh_error {
        notices.push(format!("Stale: {error}"));
    }
    if let Some(progress) = app.mutation_progress() {
        notices.push(progress.to_string());
    }
    if let Some(notice) = app.notice() {
        notices.push(notice.to_string());
    }
    if let Some(message) = &app.message {
        notices.push(message.to_string());
    }
    let detail = if notices.is_empty() {
        lookup_detail
    } else {
        notices.join(" · ")
    };
    frame.render_widget(Paragraph::new(format!("{controls}\n{detail}")), area);
}

pub fn detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = app
        .selected_branch()
        .map(super::tree::detail)
        .unwrap_or_else(|| "No branch selected".into());
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(" Branch detail ")
                .borders(Borders::LEFT),
        ),
        area,
    );
}

pub fn help(frame: &mut Frame<'_>, app: &App) {
    let area = centered(frame.area(), 88, 30);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Actions · type to search ")
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if inner.height < 8 { 1 } else { 2 }),
            Constraint::Min(1),
            Constraint::Length(if inner.height < 8 { 1 } else { 4 }),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(format!("> {}", app.action_query)),
        sections[0],
    );
    let entries = app.action_menu_entries();
    let selected = app.action_index.min(entries.len().saturating_sub(1));
    let items: Vec<_> = entries
        .iter()
        .map(|entry| {
            ListItem::new(format!(
                "{} · {}: {}",
                entry.shortcut, entry.group, entry.label
            ))
        })
        .collect();
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching actions. Backspace to edit."),
            sections[1],
        );
    } else {
        let list = List::new(items)
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(
            list,
            sections[1],
            &mut ListState::default().with_selected(Some(selected)),
        );
    }
    let selected_label = entries
        .get(selected)
        .map(|entry| format!("{} [{}]", entry.label, entry.shortcut))
        .unwrap_or_default();
    frame.render_widget(Paragraph::new(format!("↑↓ choose · Enter run · Esc close\n{selected_label}\nPR: ⑂ merged · × closed · ✓ approved" )).wrap(Wrap { trim: false }), sections[2]);
}

pub fn compact_detail(frame: &mut Frame<'_>, app: &App) {
    let area = centered(frame.area(), 88, 22);
    frame.render_widget(Clear, area);
    let text = app
        .selected_branch()
        .map(super::tree::detail)
        .unwrap_or_else(|| "No branch selected".into());
    frame.render_widget(
        Paragraph::new(format!("p Refresh PR · d / Esc close\n{text}"))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(" Branch details ")
                    .borders(Borders::ALL),
            ),
        area,
    );
}

pub fn order_picker(frame: &mut Frame<'_>, app: &App) {
    let Overlay::OrderPicker(picker) = &app.overlay else {
        return;
    };
    let choices = [
        OrderMode::Recent,
        OrderMode::Alphabetical,
        OrderMode::Graphite,
    ]
    .into_iter()
    .map(|mode| {
        let marker = if picker.pending == mode { ">" } else { " " };
        format!("{marker} {}", order_name(mode))
    })
    .collect::<Vec<_>>()
    .join("\n");
    let area = centered(frame.area(), 34, 9);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("{choices}\n\n↑/↓ choose  Enter apply  Esc cancel"))
            .block(Block::default().title(" Order ").borders(Borders::ALL)),
        area,
    );
}

pub fn color_picker(frame: &mut Frame<'_>, app: &App) {
    let Overlay::ColorPicker(picker) = &app.overlay else {
        return;
    };
    let choices = COLOR_OPTIONS
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            let marker = if picker.choice_index == index {
                ">"
            } else {
                " "
            };
            match value {
                Some(value) => format!("{marker} {name:<7} {value}"),
                None => format!("{marker} {name}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let area = centered(frame.area(), 42, 15);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(
            "Stack: {}\n{choices}\n\n↑/↓ preview  Enter save  Esc rollback",
            picker.target
        ))
        .block(
            Block::default()
                .title(" Stack color ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

pub fn checkout_confirmation(frame: &mut Frame<'_>, app: &App) {
    let MutationState::ConfirmingCheckout(target) = &app.mutation else {
        return;
    };
    let linked_worktree = app.selected_branch().and_then(|branch| {
        branch.worktree.as_ref().filter(|path| {
            app.snapshot
                .as_ref()
                .is_some_and(|snapshot| *path != &snapshot.root)
        })
    });
    let (title, prompt, action) = if let Some(path) = linked_worktree.as_ref() {
        (
            " Move worktree ",
            format!(
                "Move this branch to the primary checkout?\n\n{target}\nfrom {}\n\nThe linked worktree must be clean.",
                path.display()
            ),
            "Enter move · Esc cancel",
        )
    } else {
        (
            " Confirm switch ",
            format!("Switch to this branch?\n\n{target}"),
            "Enter switch · Esc cancel",
        )
    };
    let area = centered(
        frame.area(),
        64,
        if linked_worktree.is_some() { 11 } else { 9 },
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("{prompt}\n\n{action}"))
            .wrap(Wrap { trim: false })
            .block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

pub fn deletion_confirmation(frame: &mut Frame<'_>, app: &App) {
    let MutationState::ConfirmingDeletion(confirmation) = &app.mutation else {
        return;
    };
    let area = centered(frame.area(), 76, 13);
    frame.render_widget(Clear, area);
    let provider = match confirmation.request.expected_provenance {
        crate::model::GraphiteProvenance::DefinitelyUntracked => "Git-only",
        crate::model::GraphiteProvenance::Tracked => "Graphite tracked leaf",
        crate::model::GraphiteProvenance::Degraded => "degraded",
    };
    let pr = confirmation
        .pr_number
        .map(|number| format!(" PR #{number} and its remote branch stay open."))
        .unwrap_or_else(|| " Any remote branch stays untouched.".into());
    let text = format!(
        "uppercase X · delete exact local branch?\n\n{}\n\nProvider: {provider}. This never force-deletes or cascades.{pr}\n\n[y] delete  [n/Esc] cancel",
        confirmation.request.branch
    );
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(" Confirm deletion ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

pub fn restack_confirmation(frame: &mut Frame<'_>, app: &App) {
    let MutationState::ConfirmingRestack(request) = &app.mutation else {
        return;
    };
    let descendants = request.affected.len().saturating_sub(1);
    let area = centered(frame.area(), 76, 11);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(
            "Restack this Graphite upstack?\n\n{}\nonto {}\n{descendants} descendants may move\n\nEnter restack · Esc cancel",
            request.source.branch, request.expected_parent
        ))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Confirm restack ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

pub fn move_confirmation(frame: &mut Frame<'_>, app: &App) {
    let MutationState::ConfirmingMove(request, _) = &app.mutation else {
        return;
    };
    let descendants = request.affected.len().saturating_sub(1);
    let mode = if request.only {
        "branch only"
    } else {
        "branch and descendants"
    };
    let area = centered(frame.area(), 76, 11);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(
            "Move {} onto {}?\n\nMode: {mode}\n{descendants} descendants follow\n\nEnter move · Esc cancel",
            request.source.branch, request.target.branch
        ))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Confirm Graphite move ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn order_name(mode: OrderMode) -> &'static str {
    match mode {
        OrderMode::Recent => "Recent",
        OrderMode::Alphabetical => "Alphabetical",
        OrderMode::Graphite => "Graphite",
        OrderMode::Chronological => "Time",
    }
}
