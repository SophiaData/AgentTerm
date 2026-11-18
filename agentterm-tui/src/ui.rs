//! Rendering. Pure function of `App` -> terminal frame, so it stays simple and
//! testable.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, Prompt, Screen};

const GREEN: Color = Color::Rgb(0, 255, 65);
const DIM: Color = Color::Rgb(140, 148, 160);

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], app);
    match app.screen {
        Screen::Projects => render_projects(frame, chunks[1], app),
        Screen::Tools => render_tools(frame, chunks[1], app),
    }
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::Projects => "AgentTerm · 项目",
        Screen::Tools => "AgentTerm · 选择工具",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        ));
    let hint = if app.error.is_some() {
        Line::from(Span::styled(" 出错了", Style::default().fg(Color::Red)))
    } else {
        Line::from(Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(DIM),
        ))
    };
    frame.render_widget(Paragraph::new(hint).block(block), area);
}

fn render_projects(frame: &mut Frame, area: Rect, app: &App) {
    if let Prompt::AddProject(buffer) = &app.prompt {
        render_add_prompt(frame, area, buffer);
        return;
    }

    if app.store.projects.is_empty() {
        frame.render_widget(
            Paragraph::new("还没有项目。按 a 添加一个文件夹。")
                .block(Block::default().borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .store
        .projects
        .iter()
        .map(|project| {
            let last = project
                .last_tool
                .as_deref()
                .map(|id| format!("  · 上次：{id}"))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::raw(project.path.display().to_string()),
                Span::styled(last, Style::default().fg(DIM)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" 项目 "))
        .highlight_style(Style::default().fg(Color::Black).bg(GREEN))
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.project_cursor));
    frame.render_stateful_widget(list, area, &mut state);

    if app.prompt == Prompt::ConfirmRemove {
        render_confirm(frame, area);
    }
}

fn render_add_prompt(frame: &mut Frame, area: Rect, buffer: &str) {
    let body = vec![
        Line::from("输入项目文件夹路径，回车确认，Esc 取消："),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(GREEN)),
            Span::raw(buffer),
            Span::styled("▌", Style::default().fg(GREEN)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "提示：支持 ~/… 写法",
            Style::default().fg(DIM),
        )),
    ];
    frame.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(" 添加项目 ")),
        area,
    );
}

fn render_confirm(frame: &mut Frame, area: Rect) {
    let popup = centered(area, 44, 5);
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(
        Paragraph::new("移除这个项目？只删除记录，文件夹保留。\n\n[y] 确认  [n] 取消")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_tools(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .tools
        .iter()
        .map(|tool| {
            let (status, style) = match (&tool.path, &tool.version) {
                (Some(_), Some(version)) => {
                    (format!("已安装 · {version}"), Style::default().fg(GREEN))
                }
                (Some(_), None) => ("已安装".to_string(), Style::default().fg(GREEN)),
                (None, _) => ("未安装".to_string(), Style::default().fg(DIM)),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<14}", tool.name), Style::default()),
                Span::styled(format!("  {status}"), style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 选择工具后回车启动（Esc 返回） "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(GREEN))
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.tool_cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let text = if let Some(error) = &app.error {
        Line::from(Span::styled(
            format!("⚠ {error}"),
            Style::default().fg(Color::Red),
        ))
    } else {
        match app.screen {
            Screen::Projects => Line::from(Span::styled(
                " ↑↓/jk 选择 · Enter 打开 · a 添加 · d 删除 · r 重扫 · q 退出 ",
                Style::default().fg(DIM),
            )),
            Screen::Tools => Line::from(Span::styled(
                " ↑↓/jk 选择 · Enter 启动 · r 重扫 · Esc 返回 ",
                Style::default().fg(DIM),
            )),
        }
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}
