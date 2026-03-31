//! ratatui rendering for the chat TUI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{nick_color, App};

const CHAT_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Map nick color index to ratatui Color
fn color_for(idx: u8) -> Color {
    match idx {
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        _ => Color::White,
    }
}

/// Render the main chat interface
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    // Main layout: header, body, input
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(5),    // body
            Constraint::Length(3), // input
        ])
        .split(area);

    // Header bar
    render_header(f, app, main_chunks[0]);

    // Body: channel list + messages
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(16), // channel list
            Constraint::Min(20),    // messages
        ])
        .split(main_chunks[1]);

    render_channel_list(f, app, body_chunks[0]);
    render_messages(f, app, body_chunks[1]);

    // Input bar
    render_input(f, app, main_chunks[2]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let channel = app.active_channel_name();
    let peer_span = if app.p2p_connecting && app.peer_count == 0 {
        Span::styled(
            "connecting...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{} peers", app.peer_count),
            Style::default().fg(Color::Green),
        )
    };
    let header = Line::from(vec![
        Span::styled(
            format!(" ElastOS Chat v{} ", CHAT_VERSION),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            &app.nickname,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(&channel, Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        peer_span,
        Span::raw(" "),
        if !app.status.is_empty() {
            Span::styled(&app.status, Style::default().fg(Color::Red))
        } else {
            Span::raw("")
        },
    ]);
    f.render_widget(Paragraph::new(header), area);
}

fn render_channel_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .channels
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let style = if i == app.active_channel {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if ch.name.starts_with('@') {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prefix = if i == app.active_channel { ">" } else { " " };
            ListItem::new(format!("{}{}", prefix, ch.name)).style(style)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::RIGHT).title("Channels"));
    f.render_widget(list, area);
}

fn render_messages(f: &mut Frame, app: &App, area: Rect) {
    let channel = match app.channels.get(app.active_channel) {
        Some(c) => c,
        None => {
            let empty = Paragraph::new("No channels. Use /join #channel to start.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(empty, area);
            return;
        }
    };

    // Show recent messages that fit in the area (wrap-aware)
    let height = area.height as usize;
    let width = area.width.max(1) as usize;
    let mut visual_lines = 0;
    let mut start = 0;
    for (i, msg) in channel.messages.iter().enumerate().rev() {
        // Estimate rendered width: "[HH:MM] [ok] <nick> content" or "[HH:MM] * content"
        let text_len = 8 + if msg.sender_nick == "*" {
            2 + msg.content.len()
        } else {
            2 + msg.sender_nick.len() + 3 + msg.content.len() // 2 = "✓ " badge
        };
        let lines_needed = text_len.div_ceil(width).max(1);
        visual_lines += lines_needed;
        if visual_lines > height {
            start = i + 1;
            break;
        }
    }

    let lines: Vec<Line> = channel
        .messages
        .iter()
        .skip(start)
        .map(|msg| {
            let ts = format_time(msg.ts);
            if msg.sender_nick == "*" {
                // System message
                Line::from(vec![
                    Span::styled(format!("[{}] ", ts), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("* {}", msg.content),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            } else {
                let nick_col = color_for(nick_color(&msg.sender_nick));
                let verified_badge = match msg.verified {
                    Some(true) => Span::styled("✓", Style::default().fg(Color::Green)),
                    Some(false) => Span::styled("✗", Style::default().fg(Color::Red)),
                    None => Span::styled("·", Style::default().fg(Color::DarkGray)),
                };
                Line::from(vec![
                    Span::styled(format!("[{}] ", ts), Style::default().fg(Color::DarkGray)),
                    verified_badge,
                    Span::raw(" "),
                    Span::styled(
                        format!("<{}>", msg.sender_nick),
                        Style::default().fg(nick_col),
                    ),
                    Span::raw(format!(" {}", msg.content)),
                ])
            }
        })
        .collect();

    let messages = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(messages, area);
}

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let input = Paragraph::new(app.input.as_str())
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("> "));
    f.render_widget(input, area);

    // Place cursor
    let cursor_cols = app.input[..app.cursor].chars().count() as u16;
    f.set_cursor_position((area.x + cursor_cols + 1, area.y + 1));
}

/// Format unix timestamp as HH:MM in local time
fn format_time(ts: u64) -> String {
    let ts_i64 = ts as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&ts_i64, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}
