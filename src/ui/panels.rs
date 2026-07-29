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
        if has_supplementary {
            format!(" a View {archive_target}  x {archive_action}  X delete  ? help")
        } else {
            format!(
                " {position}/{}  {order}  {}  {pitch}  a View {archive_target}  x {archive_action}  v range  X delete  ↑↓ {stack_navigation} ? help",
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
    let area = centered(frame.area(), 72, 25);
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
        "Markers: › selected  ○ branch  ● current  ◉ trunk\n         ■ range  * dirty  ⎇ worktree\nRemote:  ✓ pushed  ↑ ahead  ↓ behind  ↕ diverged\n         × gone  ○ no remote  ? unknown\n\n↑/↓ or j/k          previous/next branch\nShift/Cmd+↑/↓ J/K  adjacent stack head, otherwise ±10 rows\nAlt+↑/↓ g/G         top/bottom branch of current section\nt / T               Recent/Graphite toggle / order picker\n+ / - / 0           adjust / reset lane pitch\nh                   focus selected stack; repeat exits\nH                   focus trunk or all Untrunked; repeat exits\ns                   toggle stack spacing\na                   toggle Active / Archive view\nv + arrows          preview contiguous archive/restore range\n/                   filter branch names\nEnter ×2            arm / confirm protected git switch\nc / C               cycle color / color picker\nn                   name / clear selected stack\nx                   archive / restore selected local branch\nX                   guarded delete exact local branch\nr                   full reconciliation\no / y               open / copy PR URL\nEsc                 close message/help\nq or Ctrl-C         quit\n\nLowercase x/v change local config only; uppercase X can delete one exact local ref after confirmation. No remote changes or fetch.\nArchive view shows dim, nonselectable ancestry for stack context.\nFocused sections pin their trunk/bottom row.\nRemote evidence uses local remote-tracking refs only; no fetch.\nColors: stack identity; yellow PR; green/red diff\nActive: {} / {} / {}\n\nGraphite: {graphite}\nGitHub: {github}",
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
        }
    );
    let text = text.replace(
        "Enter ×2            arm / confirm protected git switch\nc / C               cycle color / color picker\nn                   name / clear selected stack",
        "Enter ×2 / Enter    git switch / edit selected label\nc / C               contextual color / color picker\nd                   toggle wide detail sidebar\ni                   toggle visual section boundary\nn                   create missing stack/section label",
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
    let area = centered(frame.area(), 64, 9);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(
            "Switch to this branch?\n\n{target}\n\nEnter switch · Esc cancel"
        ))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Confirm switch ")
                .borders(Borders::ALL),
        ),
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
