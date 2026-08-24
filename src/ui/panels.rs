use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, COLOR_OPTIONS, GitHubState, LanePitch, MutationState, Overlay};
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
    let mut spans = if matches!(app.archive_mode, ArchiveMode::Archive) {
        vec![Span::styled(
            " ARCHIVE · local refs only · no fetch ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::styled(
            " stackmap ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]
    };
    spans.push(Span::raw(format!(" {title}  ")));
    spans.push(Span::styled(status, Style::default().fg(Color::Cyan)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let position = app
        .projection
        .navigation_visual_rows
        .iter()
        .position(|row| Some(*row) == app.selected_visual_row_for_ui())
        .map(|index| index + 1)
        .unwrap_or(0);
    let progress = app.mutation_progress();
    let has_supplementary = app.message.is_some()
        || app.notice().is_some()
        || progress.is_some()
        || app.refresh_error.is_some();
    let mut state = if app.overlay == Overlay::Search {
        format!(" /{}", app.filter)
    } else if let Overlay::StackNameEditor(_) = &app.overlay {
        " NAME  Enter save · empty clears · Esc cancel".to_owned()
    } else if let Overlay::ArchiveRange(range) = &app.overlay {
        format!(
            " RANGE {} {} branches  {} → {}  ↑↓ resize  Enter {}  Esc cancel",
            if matches!(range.mode, ArchiveMode::Archive) {
                "RESTORE"
            } else {
                "ARCHIVE"
            },
            range.branch_count(),
            range.anchor,
            range.endpoint,
            if matches!(range.mode, ArchiveMode::Archive) {
                "restore"
            } else {
                "archive"
            }
        )
    } else if let Overlay::MovePreview(preview) = &app.overlay {
        format!(
            " MOVE PREVIEW  {} → {}  {}  ↑↓ target  Tab mode  Enter confirm  Esc cancel",
            preview.source,
            preview.target,
            if preview.only {
                "branch only"
            } else {
                "subtree"
            }
        )
    } else {
        let order = match app.order_mode {
            OrderMode::Recent => "recent",
            OrderMode::Alphabetical => "alphabetical",
            OrderMode::Graphite => "graphite",
            OrderMode::Chronological => "time",
        };
        let pitch = match app.lane_pitch {
            LanePitch::Auto => "pitch:auto".to_owned(),
            LanePitch::Fixed(value) => format!("pitch:{value}"),
        };
        let stack_navigation = if app
            .selected
            .as_ref()
            .is_some_and(|selected| app.projection.is_true_stack(selected))
        {
            "J/K stacks"
        } else {
            "J/K ±10"
        };
        let archive_target = if matches!(app.archive_mode, ArchiveMode::Archive) {
            "Active"
        } else {
            "Archive"
        };
        let archive_action = if matches!(app.archive_mode, ArchiveMode::Archive) {
            "restore"
        } else {
            "archive"
        };
        let pr_actions = if app
            .selected_branch()
            .is_some_and(|branch| branch.pr.is_some())
        {
            "  o/O/y PR"
        } else {
            ""
        };
        if has_supplementary {
            format!(" a View {archive_target}  x {archive_action}  X delete{pr_actions}  ? help")
        } else {
            format!(
                " {position}/{}  {order}  {}  {pitch}  s status  S spacing  ↑↓ {stack_navigation}  a View {archive_target}  x {archive_action}  r restack  m move  R refresh{pr_actions}  ? help",
                app.projection.navigation.len(),
                app.scope_label(),
            )
        }
    };
    if let Some(error) = &app.refresh_error {
        state.push_str(&format!("  STALE: {error}"));
    }
    if app.overlay != Overlay::Search {
        if let Some(message) = &app.message
            && !matches!(app.mutation, MutationState::DeletionBlocked(_))
        {
            state.push_str(&format!("  {message}"));
        }
        if let Some(notice) = app.notice() {
            state.push_str(&format!("  {notice}"));
        }
        if let Some(progress) = progress {
            state.push_str(&format!("  {progress}"));
        }
    }
    if matches!(app.archive_mode, ArchiveMode::Archive)
        && let Some(branch) = app.selected_branch()
    {
        state.push_str(&format!(
            "\n {}",
            super::tree::evidence_source_footer(branch)
        ));
    }
    frame.render_widget(
        Paragraph::new(state).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

pub fn detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = app
        .selected_branch()
        .map(|branch| {
            if matches!(app.archive_mode, ArchiveMode::Archive) {
                super::tree::local_detail(branch)
            } else {
                super::tree::detail(branch)
            }
        })
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
    let area = centered(frame.area(), 72, 41);
    frame.render_widget(Clear, area);
    let graphite = app
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.graphite_status.as_ref())
        .unwrap_or("loading");
    let github = match &app.github_state {
        GitHubState::Idle => "not requested".to_owned(),
        GitHubState::Loading => "loading".to_owned(),
        GitHubState::Ready => "loaded".to_owned(),
        GitHubState::Unavailable(error) => format!("unavailable: {error}"),
    };
    let text = format!(
        "Markers: › selected  ○ branch  ● current  ◉ trunk\n         ■ range  * dirty  ⎇ worktree\nStatus:  local gray  pushed white  # open yellow\n         ✓ # approved/merged green  X # closed red\n         PR replaces pushed; pushed replaces local\n\n↑/↓ or j/k          previous/next branch\nShift/Cmd+↑/↓ J/K  adjacent stack head, otherwise ±10 rows\nAlt+↑/↓ g/G         top/bottom branch of current section\nt / T               Recent/Graphite toggle / order picker\n+ / - / 0           adjust / reset lane pitch\nh                   focus selected stack; repeat exits\nH                   focus trunk or all Untrunked; repeat exits\ns / S               toggle status / stack spacing\na                   toggle Active / Archive view\nv + arrows          preview contiguous archive/restore range\n/                   filter branch names\nEnter ×2 / Enter    switch or move clean worktree / edit label\nc / C               color active indent / color picker\nCmd+C / +Shift / +Opt+Shift  copy branch / section / stack\nd                   toggle wide detail sidebar\ni                   toggle visual section boundary\nn                   edit deepest section / real stack name\nShift+Backspace / Ctrl-U     clear complete name draft\nx                   archive / restore selected local branch\nX                   guarded delete exact local branch\nr / R               restack selected / full reconciliation\nm                   preview Graphite move; Tab toggles --only\no / O / y           open selected PR / stack PRs / copy URL\nEsc                 close/cancel\nq or Ctrl-C         quit\n\nGraphite actions require confirmation and never push or fetch.\nArchive view shows dim, nonselectable ancestry for stack context.\nFocused sections pin their trunk/bottom row.\nPushed evidence uses exact local-tip equality only; no fetch.\nGraphite and provider diagnostics remain in branch details.\nActive: {} / {} / {} / {}\n\nGraphite: {graphite}\nGitHub: {github}",
        match app.order_mode {
            OrderMode::Recent => "recent order",
            OrderMode::Alphabetical => "alphabetical order",
            OrderMode::Graphite => "Graphite order",
            OrderMode::Chronological => "time order",
        },
        app.scope_label(),
        if app.separators {
            "separators on"
        } else {
            "separators off"
        },
        if app.status_visible {
            "status on"
        } else {
            "status off"
        },
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left)
            .block(Block::default().title(" Help ").borders(Borders::ALL)),
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
    let MutationState::ConfirmingMove(request) = &app.mutation else {
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
