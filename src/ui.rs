//! Rendering. One draw function per view; popup pickers render on top of the
//! issue list.

use crate::app::{is_epic, App, View};
use crate::jira::Comment;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
    Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};
use ratatui::Frame;
use std::cell::Cell as StdCell;

const ACCENT: Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &App) {
    if let Some(err) = &app.fatal {
        draw_fatal(f, err);
        return;
    }
    let [main, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

    match app.base_view() {
        View::Detail => draw_detail(f, app, main),
        _ => draw_list(f, app, main),
    }
    draw_footer(f, app, footer);

    match app.view {
        View::FilterPicker => draw_filter_picker(f, app),
        View::TransitionPicker => draw_transition_picker(f, app),
        View::AgentPicker => draw_agent_picker(f, app),
        View::NewAgentTypePicker => draw_new_agent_type_picker(f, app),
        View::NewAgentWorkspacePicker => draw_new_agent_workspace_picker(f, app),
        View::NewAgentCwdPicker => draw_new_agent_cwd_picker(f, app),
        View::NewAgentCwdInput => draw_cwd_input(f, app),
        View::WorktreeProjectPicker => draw_worktree_project_picker(f, app),
        View::WorktreeRepoInput => draw_worktree_repo_input(f, app),
        View::WorktreeListPicker => draw_worktree_list_picker(f, app),
        View::WorktreeNameInput => draw_worktree_name_input(f, app),
        View::WorktreeAgentPicker => draw_worktree_agent_picker(f, app),
        View::SearchInput => draw_search(f, app),
        View::JqlInput => draw_jql(f, app),
        View::Help => draw_help(f, app.help_return == View::Detail),
        _ => {}
    }
}

fn status_color(category: &str) -> Color {
    match category {
        "new" => Color::Blue,
        "indeterminate" => Color::Yellow,
        "done" => Color::Green,
        _ => Color::Gray,
    }
}

fn type_style(issue_type: &str) -> Style {
    match issue_type.to_ascii_lowercase().as_str() {
        "epic" => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        "bug" | "defect" => Style::new().fg(Color::Red),
        "story" => Style::new().fg(Color::Green),
        "task" => Style::new().fg(Color::Blue),
        "sub-task" | "subtask" => Style::new().fg(Color::Cyan),
        _ => Style::new().fg(Color::Gray),
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.current_title.is_empty() {
        "Jira".to_string()
    } else {
        format!("Jira — {} ({})", app.current_title, app.issues.len())
    };
    let suffix = if app.loading { "  ⟳ loading…" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::from(vec![
            Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()),
            Span::styled(suffix, Style::new().fg(Color::Yellow)),
        ]))
        .title_alignment(Alignment::Left);

    let visible = app.visible();
    let rows: Vec<Row> = visible
        .iter()
        .map(|&(i, depth)| {
            // Epics carry an expand indicator; their children are indented.
            let (prefix, key_style) = if is_epic(i) {
                let arrow = if app.expanded.contains(&i.key) {
                    "▾ "
                } else {
                    "▸ "
                };
                (
                    arrow.to_string(),
                    Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                )
            } else if depth > 0 {
                (" └ ".to_string(), Style::new().fg(ACCENT))
            } else {
                ("  ".to_string(), Style::new().fg(ACCENT))
            };
            Row::new(vec![
                Cell::from(format!("{prefix}{}", i.key)).style(key_style),
                Cell::from(i.issue_type.clone()).style(type_style(&i.issue_type)),
                Cell::from(i.status.clone())
                    .style(Style::new().fg(status_color(&i.status_category))),
                Cell::from(i.assignee.clone()).style(Style::new().fg(Color::Magenta)),
                Cell::from(i.updated.clone()).style(Style::new().fg(Color::DarkGray)),
                Cell::from(i.summary.clone()),
            ])
        })
        .collect();

    let is_empty = rows.is_empty();
    let table = Table::new(
        rows,
        [
            Constraint::Length(15),
            Constraint::Length(7),
            Constraint::Length(14),
            Constraint::Length(18),
            Constraint::Length(16),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec![
            "KEY", "TYPE", "STATUS", "ASSIGNEE", "UPDATED", "SUMMARY",
        ])
        .style(
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .row_highlight_style(
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .block(block);

    let mut state = TableState::default();
    state.select(if is_empty { None } else { Some(app.selected) });
    f.render_stateful_widget(table, area, &mut state);

    if is_empty && !app.loading {
        let empty =
            Paragraph::new("no issues — press r to refresh, f to pick a filter, / to search")
                .style(Style::new().fg(Color::DarkGray))
                .alignment(Alignment::Center);
        let inner = centered_rect(area, 80, 20);
        f.render_widget(empty, inner);
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(issue) = app.selected_issue() else {
        return;
    };
    let (issue_border, comments_border) = if app.detail_focus_comments {
        (Color::DarkGray, ACCENT)
    } else {
        (ACCENT, Color::DarkGray)
    };
    let area = if area.height >= 12 {
        let [top, bottom] =
            Layout::vertical([Constraint::Min(6), Constraint::Percentage(40)]).areas(area);
        draw_comments(f, app, &issue.key, bottom, comments_border);
        app.detail_comments_area.set(bottom);
        top
    } else {
        app.detail_comments_area.set(Rect::default());
        area
    };
    app.detail_issue_area.set(area);
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(issue.key.clone(), Style::new().fg(ACCENT).bold()),
            Span::raw("  "),
            Span::styled(issue.summary.clone(), Style::new().bold()),
        ]),
        Line::default(),
        meta_line(
            "Status",
            &issue.status,
            status_color(&issue.status_category),
        ),
        meta_line("Type", &issue.issue_type, Color::White),
        meta_line("Priority", &issue.priority, Color::White),
        meta_line("Assignee", &issue.assignee, Color::Magenta),
        meta_line("Reporter", &issue.reporter, Color::Magenta),
        meta_line("Updated", &issue.updated, Color::White),
    ];
    if !issue.labels.is_empty() {
        lines.push(meta_line("Labels", &issue.labels.join(", "), Color::Yellow));
    }
    lines.push(meta_line("Link", &issue.url, Color::Blue));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Description",
        Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    let desc = if issue.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        issue.description.clone()
    };
    for l in desc.lines() {
        lines.push(Line::from(l.to_string()));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(issue_border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(" issue ", Style::new().fg(ACCENT).bold()));
    render_scrolled(
        f,
        lines,
        block,
        area,
        app.detail_scroll,
        &app.detail_scroll_max,
        false,
    );
}

/// Comments pane for `key`: newest first, or its loading / error / empty state.
fn draw_comments(f: &mut Frame, app: &App, key: &str, area: Rect, border: Color) {
    let cached = app.comments.get(key);
    let count = cached
        .map(|cs| format!(" ({})", cs.len()))
        .unwrap_or_default();
    let dim = Style::new().fg(Color::DarkGray);
    let lines: Vec<Line> = match (cached, app.comments_err.get(key)) {
        (Some(cs), _) if cs.is_empty() => vec![Line::styled("(no comments)", dim)],
        (Some(cs), _) => comment_lines(cs),
        (None, Some(e)) => vec![Line::styled(
            format!("comments: {e}"),
            Style::new().fg(Color::Red),
        )],
        (None, None) => vec![Line::styled("loading comments…", dim)],
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" comments · {key}{count} "),
            Style::new().fg(ACCENT).bold(),
        ));
    render_scrolled(
        f,
        lines,
        block,
        area,
        app.comment_scroll,
        &app.comment_scroll_max,
        true,
    );
}

/// Wrapped paragraph scrolled to `scroll`, clamped so the last line can't
/// leave the bottom edge. Publishes that limit to `max` for key handling and
/// draws a scrollbar on the right border when the content overflows. With
/// `mark_cut`, a ` … ` on the bottom border flags content hidden below.
fn render_scrolled(
    f: &mut Frame,
    lines: Vec<Line>,
    mut block: Block,
    area: Rect,
    scroll: u16,
    max: &StdCell<u16>,
    mark_cut: bool,
) {
    let inner = block.inner(area);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    // Counted before attaching the block: `line_count` doesn't subtract the
    // block's horizontal borders/padding from `width`.
    let limit = para
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    max.set(limit);
    let pos = scroll.min(limit);
    if mark_cut && pos < limit {
        block = block.title_bottom(Span::styled(" … ", Style::new().fg(ACCENT).bold()));
    }
    f.render_widget(para.scroll((pos, 0)).block(block), area);
    if limit > 0 {
        let mut state = ScrollbarState::new(limit as usize + 1)
            .position(pos as usize)
            .viewport_content_length(inner.height as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area.inner(Margin::new(0, 1)),
            &mut state,
        );
    }
}

/// Per comment: `author  created` header, the body, then a blank separator.
fn comment_lines(comments: &[Comment]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (n, c) in comments.iter().enumerate() {
        if n > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(vec![
            Span::styled(c.author.clone(), Style::new().fg(Color::Magenta).bold()),
            Span::styled(format!("  {}", c.created), Style::new().fg(Color::DarkGray)),
        ]));
        let body = c.body.trim_end();
        if body.is_empty() {
            lines.push(Line::styled("(empty)", Style::new().fg(Color::DarkGray)));
        } else {
            lines.extend(body.lines().map(|l| Line::from(l.to_string())));
        }
    }
    lines
}

fn meta_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::new().fg(Color::DarkGray)),
        Span::styled(value.to_string(), Style::new().fg(color)),
    ])
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // A toast overrides the hint line for a few seconds.
    if let Some((msg, is_error, at)) = &app.toast {
        if at.elapsed().as_secs() < 5 {
            let style = if *is_error {
                Style::new().fg(Color::White).bg(Color::Red)
            } else {
                Style::new().fg(Color::Black).bg(Color::Green)
            };
            f.render_widget(Paragraph::new(format!(" {msg} ")).style(style), area);
            return;
        }
    }
    let hints = match app.base_view() {
        View::Detail => "j/k scroll  ·  Tab comments  ·  w worktree  ·  d delegate  ·  s status  ·  o browser  ·  z zoom  ·  Esc back  ·  ? help",
        View::SearchInput => "Enter search  ·  Esc cancel",
        View::JqlInput => "Enter run JQL  ·  Ctrl-U clear  ·  Esc cancel",
        View::NewAgentCwdInput => "Enter start  ·  Ctrl-U clear  ·  Esc back",
        View::WorktreeRepoInput | View::WorktreeNameInput => {
            "Enter next  ·  Ctrl-U clear  ·  Esc back"
        }
        View::AgentPicker => {
            "1-9 pick  ·  n new agent  ·  j/k move  ·  Enter select  ·  Esc cancel"
        }
        View::WorktreeProjectPicker => {
            "1-9 pick  ·  j/k move  ·  Enter worktrees  ·  / type path  ·  Esc cancel"
        }
        View::WorktreeListPicker => {
            "1-9 pick  ·  j/k move  ·  Enter new / reopen  ·  Esc back"
        }
        View::NewAgentTypePicker
        | View::NewAgentWorkspacePicker
        | View::NewAgentCwdPicker
        | View::WorktreeAgentPicker => {
            "1-9 pick  ·  j/k move  ·  Enter select  ·  Esc back"
        }
        View::FilterPicker | View::TransitionPicker => {
            "1-9 quick pick  ·  j/k move  ·  Enter select  ·  Esc cancel"
        }
        // Priority order: narrow panes drop hints from the right; `? help` always stays.
        _ => "Enter open  ·  w worktree  ·  d delegate  ·  s status  ·  f filters  ·  / search  ·  →/← epic  ·  r refresh  ·  z zoom  ·  q quit  ·  ? help",
    };
    f.render_widget(
        Paragraph::new(fit_hints(hints, area.width as usize))
            .style(Style::new().fg(Color::DarkGray)),
        area,
    );
}

const HINT_SEP: &str = "  ·  ";

/// Fit a `·`-separated hint line into `width` columns: drop hints from the
/// right, but always keep the last one (the escape hatch, e.g. `? help`).
fn fit_hints(hints: &str, width: usize) -> String {
    if Span::raw(hints).width() <= width {
        return hints.to_string();
    }
    let parts: Vec<&str> = hints.split(HINT_SEP).collect();
    let Some((last, head)) = parts.split_last() else {
        return String::new();
    };
    let sep = Span::raw(HINT_SEP).width();
    let mut used = Span::raw(*last).width();
    let mut kept: Vec<&str> = Vec::new();
    for part in head {
        let w = Span::raw(*part).width() + sep;
        if used + w > width {
            break;
        }
        used += w;
        kept.push(part);
    }
    kept.push(last);
    kept.join(HINT_SEP)
}

fn popup(f: &mut Frame, title: &str, width_pct: u16, height: u16) -> Rect {
    let area = f.area();
    let w = (area.width * width_pct / 100).max(30).min(area.width);
    let h = height.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Span::styled(
                format!(" {title} "),
                Style::new().fg(ACCENT).bold(),
            )),
        rect,
    );
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    }
}

fn render_picker_list(f: &mut Frame, inner: Rect, items: Vec<ListItem>, sel: usize) {
    let mut state = ListState::default();
    state.select(if items.is_empty() { None } else { Some(sel) });
    let list = List::new(items)
        .highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_filter_picker(f: &mut Frame, app: &App) {
    let h = app.cfg.filters.len() as u16 + 2;
    let inner = popup(f, "filters", 50, h.max(3));
    let items: Vec<ListItem> = app
        .cfg
        .filters
        .iter()
        .enumerate()
        .map(|(i, flt)| {
            let marker = if i == app.filter_idx { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{}{}. ", marker, i + 1),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::raw(flt.name.clone()),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_transition_picker(f: &mut Frame, app: &App) {
    let title = format!("set status — {}", app.transitions_for);
    let h = (app.transitions.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 50, h);
    if app.transitions_loading {
        f.render_widget(
            Paragraph::new("loading transitions…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .transitions
        .iter()
        .enumerate()
        .map(|(i, t)| {
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::raw(t.name.clone()),
                Span::styled(
                    format!("  → {}", t.to_status),
                    Style::new().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_agent_picker(f: &mut Frame, app: &App) {
    let key = app
        .selected_issue()
        .map(|i| i.key.clone())
        .unwrap_or_default();
    let title = format!("delegate {key} to agent");
    let h = (app.agent_picker_len() as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h);
    if app.agents_loading {
        f.render_widget(
            Paragraph::new("listing agents…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let mut items: Vec<ListItem> = Vec::with_capacity(app.agent_picker_len());
    let offset = app.agent_list_offset();
    if offset > 0 {
        items.push(ListItem::new(Line::from(vec![
            num_span(0),
            Span::styled(
                "+ start new agent…",
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  (n)", Style::new().fg(Color::DarkGray)),
        ])));
    }
    for (i, a) in app.agents.iter().enumerate() {
        let row = i + offset;
        let status_color = match a.status.as_str() {
            "idle" | "done" => Color::Green,
            "working" => Color::Yellow,
            "blocked" => Color::Red,
            _ => Color::Gray,
        };
        let cwd = short_path(&a.cwd);
        items.push(ListItem::new(Line::from(vec![
            num_span(row),
            Span::styled(format!("{:<10}", a.label), Style::new().fg(ACCENT).bold()),
            Span::styled(format!("{:<9}", a.status), Style::new().fg(status_color)),
            Span::styled(
                format!("{:<8}", a.pane_id),
                Style::new().fg(Color::DarkGray),
            ),
            Span::raw(cwd),
        ])));
    }
    if items.is_empty() {
        f.render_widget(
            Paragraph::new("no agents — add [[delegate.agents]] in config to start new ones")
                .style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_new_agent_type_picker(f: &mut Frame, app: &App) {
    let key = app
        .selected_issue()
        .map(|i| i.key.clone())
        .unwrap_or_default();
    let title = format!("start agent for {key} — pick agent");
    let agents = &app.cfg.delegate.agents;
    let h = (agents.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 55, h);
    let items: Vec<ListItem> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let cmd = a.command.join(" ");
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::styled(format!("{:<12}", a.name), Style::new().fg(ACCENT).bold()),
                Span::styled(cmd, Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_new_agent_workspace_picker(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let place = app.cfg.delegate.placement.trim().to_ascii_lowercase();
    let place_hint = if place == "right" || place == "down" {
        format!("split {place}")
    } else {
        "new tab".into()
    };
    let title = format!("space for {agent_name} ({place_hint})");
    let h = (app.workspaces.len() as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h);
    if app.workspaces_loading {
        f.render_widget(
            Paragraph::new("listing spaces…").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let current = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
    let items: Vec<ListItem> = app
        .workspaces
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let marker = if !current.is_empty() && w.id == current {
                "● "
            } else if w.focused {
                "○ "
            } else {
                "  "
            };
            let status_color = match w.agent_status.as_str() {
                "idle" | "done" => Color::Green,
                "working" => Color::Yellow,
                "blocked" => Color::Red,
                _ => Color::Gray,
            };
            ListItem::new(Line::from(vec![
                num_span(i),
                Span::styled(marker, Style::new().fg(Color::Green)),
                Span::styled(
                    format!("{:<4}", format!("#{}", w.number)),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<24}", truncate_label(&w.label, 24)),
                    Style::new().fg(ACCENT).bold(),
                ),
                Span::styled(
                    format!("{:<8}", w.agent_status),
                    Style::new().fg(status_color),
                ),
                Span::styled(
                    format!("{} tabs · {} panes", w.tab_count, w.pane_count),
                    Style::new().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    render_picker_list(f, inner, items, app.picker_sel);
}

fn truncate_label(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let take = max.saturating_sub(1);
        format!("{}…", s.chars().take(take).collect::<String>())
    }
}

fn draw_new_agent_cwd_picker(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let ws = app
        .pending_workspace
        .as_ref()
        .map(|w| w.label.as_str())
        .unwrap_or("?");
    let title = format!("cwd for {agent_name} in {ws}");
    let n = app.cwd_choices.len() + 1;
    let h = (n as u16 + 2).max(3);
    let inner = popup(f, &title, 70, h.max(4));
    let mut items: Vec<ListItem> = app
        .cwd_choices
        .iter()
        .enumerate()
        .map(|(i, p)| ListItem::new(Line::from(vec![num_span(i), Span::raw(short_path(p))])))
        .collect();
    items.push(ListItem::new(Line::from(vec![
        num_span(app.cwd_choices.len()),
        Span::styled(
            "type path…",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (/)", Style::new().fg(Color::DarkGray)),
    ])));
    render_picker_list(f, inner, items, app.picker_sel);
}

fn draw_cwd_input(f: &mut Frame, app: &App) {
    let agent_name = app
        .pending_spawn
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("agent");
    let title = format!("cwd for {agent_name}");
    let inner = popup(f, &title, 75, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.cwd_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

fn worktree_issue_key(app: &App) -> String {
    app.selected_issue()
        .map(|i| i.key.clone())
        .unwrap_or_default()
}

/// Dim explanatory first line of a picker popup; returns the list area below it.
fn picker_header(f: &mut Frame, inner: Rect, text: &str) -> Rect {
    let [head, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    f.render_widget(
        Paragraph::new(text).style(Style::new().fg(Color::DarkGray)),
        head,
    );
    list
}

fn draw_worktree_project_picker(f: &mut Frame, app: &App) {
    let title = format!("worktree for {} — project", worktree_issue_key(app));
    let projects = &app.wt_projects;
    let h = projects.len() as u16 + 4;
    let inner = popup(f, &title, 75, h);
    let header = if app.wt_projects_loading {
        "listing herdr projects…"
    } else if projects.is_empty() {
        "no git projects open in herdr — type a repo path"
    } else {
        "herdr projects (source checkouts) · ★ current workspace"
    };
    let list = picker_header(f, inner, header);
    let name_w = projects
        .iter()
        .map(|p| p.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(24);
    let mut items: Vec<ListItem> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut spans = vec![
                num_span(i),
                Span::styled(
                    if p.current { "★ " } else { "  " },
                    Style::new().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{:<name_w$}  ", truncate_label(&p.name, name_w)),
                    Style::new().fg(ACCENT).bold(),
                ),
                Span::raw(short_path(&p.root)),
            ];
            if p.open_worktrees > 0 {
                let s = if p.open_worktrees == 1 { "" } else { "s" };
                spans.push(Span::styled(
                    format!("  ({} open worktree{s})", p.open_worktrees),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    items.push(ListItem::new(Line::from(vec![
        num_span(projects.len()),
        Span::styled(
            "type path…",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (/)", Style::new().fg(Color::DarkGray)),
    ])));
    render_picker_list(f, list, items, app.picker_sel);
}

fn draw_worktree_repo_input(f: &mut Frame, app: &App) {
    let title = format!("worktree for {} — repo path", worktree_issue_key(app));
    let inner = popup(f, &title, 75, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.cwd_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

fn draw_worktree_list_picker(f: &mut Frame, app: &App) {
    let title = format!(
        "worktree for {} — {}",
        worktree_issue_key(app),
        app.wt_repo_name
    );
    let worktrees = &app.wt_worktrees;
    let h = worktrees.len() as u16 + 4;
    let inner = popup(f, &title, 75, h);
    let header = if app.wt_worktrees_loading {
        format!("listing worktrees of {}…", short_path(&app.wt_repo))
    } else {
        format!(
            "create a worktree or reopen one of {}",
            short_path(&app.wt_repo)
        )
    };
    let list = picker_header(f, inner, &header);
    let default = if app.wt_default_branch.is_empty() {
        String::new()
    } else {
        format!(" ({})", app.wt_default_branch)
    };
    let mut items = vec![ListItem::new(Line::from(vec![
        num_span(0),
        Span::styled(
            format!("+ new worktree{default}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ]))];
    let branch_w = worktrees
        .iter()
        .map(|w| w.branch.chars().count())
        .max()
        .unwrap_or(0)
        .min(32);
    items.extend(worktrees.iter().enumerate().map(|(i, w)| {
        let mut spans = vec![
            num_span(i + 1),
            Span::styled(
                format!("{:<branch_w$}  ", truncate_label(&w.branch, branch_w)),
                Style::new().fg(ACCENT).bold(),
            ),
            Span::raw(short_path(&w.path)),
        ];
        if w.is_main {
            spans.push(Span::styled("  (source)", Style::new().fg(Color::DarkGray)));
        }
        if w.open {
            spans.push(Span::styled("  [open]", Style::new().fg(Color::Green)));
        }
        ListItem::new(Line::from(spans))
    }));
    render_picker_list(f, list, items, app.picker_sel);
}

fn draw_worktree_name_input(f: &mut Frame, app: &App) {
    let title = format!(
        "new worktree for {} in {}",
        worktree_issue_key(app),
        app.wt_repo_name
    );
    let inner = popup(f, &title, 75, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.wt_name_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

fn draw_worktree_agent_picker(f: &mut Frame, app: &App) {
    let title = format!("worktree {} — start an agent?", app.wt_branch);
    let agents = &app.cfg.delegate.agents;
    let h = (agents.len() as u16 + 3).max(3);
    let inner = popup(f, &title, 60, h);
    let mut items = vec![ListItem::new(Line::from(vec![
        num_span(0),
        Span::styled(
            "no agent — just open the worktree",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ]))];
    items.extend(agents.iter().enumerate().map(|(i, a)| {
        ListItem::new(Line::from(vec![
            num_span(i + 1),
            Span::styled(format!("{:<12}", a.name), Style::new().fg(ACCENT).bold()),
            Span::styled(a.command.join(" "), Style::new().fg(Color::DarkGray)),
        ]))
    }));
    render_picker_list(f, inner, items, app.picker_sel);
}

/// "1. " index prefix shown in pickers — rows past 9 have no hotkey.
fn num_span(i: usize) -> Span<'static> {
    let text = if i < 9 {
        format!("{}. ", i + 1)
    } else {
        "   ".to_string()
    };
    Span::styled(text, Style::new().fg(Color::DarkGray))
}

fn short_path(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

fn draw_search(f: &mut Frame, app: &App) {
    let inner = popup(f, "search (text ~ …)", 60, 3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.search_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        inner,
    );
}

fn draw_jql(f: &mut Frame, app: &App) {
    let inner = popup(f, "custom JQL", 80, 4);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(app.jql_input.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .wrap(Wrap { trim: false }),
        inner,
    );
}

/// Key reference for the list, or for the issue details view when opened
/// from there.
fn draw_help(f: &mut Frame, detail: bool) {
    let rows: &[(&str, &str)] = if detail {
        &[
            ("j/k ↑/↓", "scroll the focused pane"),
            ("PgUp/PgDn", "scroll by 15 lines"),
            ("g/G", "top / bottom of the focused pane"),
            ("Tab/click", "switch focus: issue / comments"),
            ("wheel", "scroll the pane under the cursor"),
            ("s", "change issue status"),
            ("d", "delegate issue to an agent"),
            ("w", "git worktree: project → new/existing worktree → agent"),
            ("o", "open issue in browser"),
            ("z", "zoom pane (fullscreen toggle)"),
            ("?", "this help"),
            ("Esc/q", "back to the issue list"),
        ]
    } else {
        &[
            ("j/k ↑/↓", "move / scroll"),
            ("Enter", "open issue details (? there for its keys)"),
            ("→/l ←/h", "expand / collapse epic"),
            ("f, 1-9", "switch filter"),
            ("/", "search (text ~ query)"),
            ("J", "run custom JQL (prefilled with current)"),
            ("1-9", "quick pick in any popup"),
            ("s", "change issue status"),
            ("d", "delegate issue to an agent"),
            ("n", "in delegate picker: start a new agent"),
            ("w", "git worktree: project → new/existing worktree → agent"),
            ("o", "open issue in browser"),
            ("z", "zoom pane (fullscreen toggle)"),
            ("r", "refresh current filter"),
            ("R", "reload config.toml"),
            ("g/G", "top / bottom"),
            ("Esc", "back / cancel"),
            ("q", "quit"),
        ]
    };
    let title = if detail {
        "help · issue details"
    } else {
        "help"
    };
    let inner = popup(f, title, 60, rows.len() as u16 + 3);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("  {k:<10}"), Style::new().fg(ACCENT)),
                Span::raw(v.to_string()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_fatal(f: &mut Frame, err: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Red))
        .padding(Padding::uniform(1))
        .title(Span::styled(
            " herdr-jira-worktree: configuration ",
            Style::new().fg(Color::Red).bold(),
        ));
    let text = format!("{err}\n\nR — retry after fixing the config, q — quit");
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        f.area(),
    );
}

fn centered_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let w = area.width * width_pct / 100;
    let h = (area.height * height_pct / 100).max(1);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod footer_tests {
    use super::fit_hints;

    #[test]
    fn narrow_footer_drops_middle_hints_but_keeps_the_last() {
        let hints = "Enter open  ·  w worktree  ·  ? help";
        assert_eq!(fit_hints(hints, 80), hints);
        assert_eq!(fit_hints(hints, 25), "Enter open  ·  ? help");
        assert_eq!(fit_hints(hints, 3), "? help");
    }

    #[test]
    fn worktree_hint_survives_a_narrow_split_pane() {
        let list = "Enter open  ·  w worktree  ·  d delegate  ·  s status  ·  f filters  ·  / search  ·  →/← epic  ·  r refresh  ·  z zoom  ·  q quit  ·  ? help";
        let fitted = fit_hints(list, 60);
        assert!(fitted.contains("w worktree"));
        assert!(fitted.ends_with("? help"));
    }
}

#[cfg(test)]
mod scroll_tests {
    use crate::app::{App, View};
    use crate::jira::{Comment, Issue};
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn render(term: &mut Terminal<TestBackend>, app: &App) -> Buffer {
        term.draw(|f| super::draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        buf.content()
            .chunks(buf.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Detail view of one issue with a 10-comment thread that overflows the
    /// comments pane; the oldest (last) comment ends with `END-MARKER`.
    fn detail_app() -> App {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::unloaded(tx);
        app.issues = vec![Issue {
            key: "ABC-1".into(),
            summary: "긴 댓글 스레드".into(),
            status: "In Progress".into(),
            status_category: "indeterminate".into(),
            issue_type: "Task".into(),
            priority: "Medium".into(),
            assignee: "홍길동".into(),
            reporter: "Kim".into(),
            updated: "2026-09-28 10:00".into(),
            labels: vec![],
            description: "설명".into(),
            url: "https://jira.example/browse/ABC-1".into(),
        }];
        // Newest first.
        let comments: Vec<Comment> = (0..10)
            .map(|n| Comment {
                author: format!("작성자 {n}"),
                created: format!("2026-09-{:02} 09:00", 28 - n),
                body: format!(
                    "첫 줄 {n}\n{}\nsecond line {n}{}",
                    "한글 줄바꿈 테스트 ".repeat(12),
                    if n == 9 { "\nEND-MARKER" } else { "" }
                ),
            })
            .collect();
        app.comments.insert("ABC-1".into(), comments);
        app.view = View::Detail;
        app
    }

    #[test]
    fn comments_pane_scroll_stops_with_last_line_visible() {
        let mut app = detail_app();
        app.detail_focus_comments = true;

        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let first = render(&mut term, &app);
        assert!(
            !text(&first).contains("END-MARKER"),
            "thread must overflow the pane"
        );
        assert!(text(&first).contains(" … "), "cut-off marker missing");

        app.on_key(KeyEvent::from(KeyCode::Char('G')));
        let bottom = render(&mut term, &app);
        let max = app.comment_scroll_max.get();
        assert!(max > 0);
        assert_eq!(app.comment_scroll, max);
        assert!(text(&bottom).contains("END-MARKER"), "{}", text(&bottom));
        assert!(!text(&bottom).contains(" … "), "marker shown at the end");

        // Scrolling further must neither move the offset nor blank the pane.
        for _ in 0..200 {
            app.on_key(KeyEvent::from(KeyCode::Char('j')));
        }
        app.on_key(KeyEvent::from(KeyCode::PageDown));
        let after = render(&mut term, &app);
        assert_eq!(app.comment_scroll, max);
        assert_eq!(after, bottom);
    }

    #[test]
    fn mouse_click_focuses_pane_and_wheel_scrolls_hovered_pane() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let at = |kind, r: ratatui::layout::Rect| MouseEvent {
            kind,
            column: r.x + 2,
            row: r.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let mut app = detail_app();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        render(&mut term, &app);
        let (issue, comments) = (app.detail_issue_area.get(), app.detail_comments_area.get());
        assert!(!app.detail_focus_comments);

        app.on_mouse(at(MouseEventKind::Down(MouseButton::Left), comments));
        assert!(app.detail_focus_comments);
        app.on_mouse(at(MouseEventKind::Down(MouseButton::Left), issue));
        assert!(!app.detail_focus_comments);

        // Wheel follows the cursor, not the keyboard focus.
        app.on_mouse(at(MouseEventKind::ScrollDown, comments));
        assert!(app.comment_scroll > 0);
        assert_eq!(app.detail_scroll, 0);
        assert!(!app.detail_focus_comments);
    }

    #[test]
    fn help_from_detail_lists_detail_keys_and_returns_to_detail() {
        let mut app = detail_app();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        let shown = text(&render(&mut term, &app));
        assert!(shown.contains("help · issue details"), "{shown}");
        assert!(shown.contains("switch focus: issue / comments"));
        assert!(
            shown.contains("comments · ABC-1"),
            "detail stays underneath"
        );

        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.view, View::Detail);
    }
}
