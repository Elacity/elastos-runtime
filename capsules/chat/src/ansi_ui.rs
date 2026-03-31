use std::io::{self, Stderr, Write};

use anyhow::Result;

use crate::app::{nick_color, App, Message};

const CHAT_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const CHANNEL_WIDTH: usize = 16;

pub struct AnsiUi {
    stderr: Stderr,
    cols: u16,
    rows: u16,
    last_frame: String,
}

impl AnsiUi {
    pub fn enter() -> Result<Self> {
        let mut stderr = io::stderr();
        write!(stderr, "\x1b[?1049h\x1b[?25h\x1b[2J\x1b[H")?;
        stderr.flush()?;
        let (cols, rows) = terminal_size();
        Ok(Self {
            stderr,
            cols,
            rows,
            last_frame: String::new(),
        })
    }

    pub fn render(&mut self, app: &App) -> Result<()> {
        self.cols = terminal_size().0.max(40);
        self.rows = terminal_size().1.max(12);

        let cols = self.cols as usize;
        let rows = self.rows as usize;
        let input_height = 3usize;
        let body_height = rows.saturating_sub(1 + input_height);
        let channel_width = CHANNEL_WIDTH.min(cols.saturating_sub(20).max(8));
        let message_width = cols.saturating_sub(channel_width + 1);
        let mut frame = String::new();
        frame.push_str("\x1b[H\x1b[2J");

        push_line(
            &mut frame,
            1,
            &truncate(
                &format!(
                    " ElastOS IRC v{} | {} | {} | {}",
                    CHAT_VERSION,
                    app.nickname,
                    app.active_channel_name(),
                    header_peer_status(app)
                ),
                cols,
            ),
        );

        let channel_lines = render_channel_lines(app, channel_width, body_height);
        let message_lines = render_message_lines(app, message_width, body_height);

        for row in 0..body_height {
            let left = channel_lines.get(row).cloned().unwrap_or_default();
            let right = message_lines.get(row).cloned().unwrap_or_default();
            let line = format!(
                "{}|{}",
                pad_right(&left, channel_width),
                pad_right(&right, message_width.saturating_sub(1))
            );
            push_line(&mut frame, (row + 2) as u16, &truncate(&line, cols));
        }

        let separator_row = (body_height + 2) as u16;
        push_line(&mut frame, separator_row, &"-".repeat(cols));
        push_line(
            &mut frame,
            separator_row + 1,
            &truncate(&format!("> {}", app.input), cols),
        );

        let footer = if app.status.is_empty() {
            " Esc /home to return | /quit to exit | Tab switch channel "
        } else {
            app.status.as_str()
        };
        push_line(&mut frame, separator_row + 2, &truncate(footer, cols));

        let cursor_col = 3 + app.input[..app.cursor].chars().count();
        frame.push_str(&format!(
            "\x1b[{};{}H",
            separator_row + 1,
            cursor_col.min(cols.saturating_sub(1)).max(1)
        ));

        if frame == self.last_frame {
            return Ok(());
        }

        self.last_frame = frame.clone();
        write!(self.stderr, "{frame}")?;
        self.stderr.flush()?;
        Ok(())
    }
}

impl Drop for AnsiUi {
    fn drop(&mut self) {
        let _ = write!(self.stderr, "\x1b[2J\x1b[H\x1b[?1049l\x1b[?25h");
        let _ = self.stderr.flush();
    }
}

fn header_peer_status(app: &App) -> String {
    if app.p2p_connecting && app.peer_count == 0 {
        "connecting...".to_string()
    } else {
        format!("{} peer(s)", app.peer_count)
    }
}

fn push_line(frame: &mut String, row: u16, line: &str) {
    frame.push_str(&format!("\x1b[{};1H\x1b[2K{}", row, line));
}

fn render_channel_lines(app: &App, width: usize, height: usize) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(truncate("Channels", width));
    for (index, channel) in app.channels.iter().enumerate() {
        if lines.len() >= height {
            break;
        }
        let prefix = if index == app.active_channel {
            ">"
        } else {
            " "
        };
        lines.push(truncate(&format!("{}{}", prefix, channel.name), width));
    }
    lines
}

fn render_message_lines(app: &App, width: usize, height: usize) -> Vec<String> {
    let Some(channel) = app.channels.get(app.active_channel) else {
        return vec![truncate("No channels. Use /join #channel to start.", width)];
    };

    let mut rows = Vec::new();
    for msg in &channel.messages {
        rows.extend(format_message_lines(msg, width));
    }

    if rows.len() > height {
        rows = rows.split_off(rows.len() - height);
    }
    rows
}

fn format_message_lines(msg: &Message, width: usize) -> Vec<String> {
    let display_ts = msg.display_ts.unwrap_or(msg.ts);
    let prefix = if msg.sender_nick == "*" {
        format!("[{}] * ", format_time(display_ts))
    } else {
        let badge = match msg.verified {
            Some(true) => "v",
            Some(false) => "x",
            None => ".",
        };
        let _color_idx = nick_color(&msg.sender_nick);
        format!("[{}] {} <{}> ", format_time(display_ts), badge, msg.sender_nick)
    };

    wrap_prefixed(&prefix, &msg.content, width)
}

fn wrap_prefixed(prefix: &str, body: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let prefix_width = prefix.chars().count();
    let body_width = width.saturating_sub(prefix_width).max(1);
    let wrapped = wrap_text(body, body_width);
    let continuation = " ".repeat(prefix_width);
    wrapped
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx == 0 {
                truncate(&format!("{}{}", prefix, line), width)
            } else {
                truncate(&format!("{}{}", continuation, line), width)
            }
        })
        .collect()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let mut current = String::new();
        for ch in raw_line.chars() {
            current.push(ch);
            if current.chars().count() >= width {
                out.push(std::mem::take(&mut current));
            }
        }
        if current.is_empty() {
            out.push(String::new());
        } else {
            out.push(current);
        }
    }
    out
}

fn pad_right(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        truncate(text, width)
    } else {
        format!("{}{}", text, " ".repeat(width - len))
    }
}

fn truncate(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

fn terminal_size() -> (u16, u16) {
    let cols = std::env::var("ELASTOS_TERM_COLS")
        .ok()
        .or_else(|| std::env::var("COLUMNS").ok())
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(80);
    let rows = std::env::var("ELASTOS_TERM_ROWS")
        .ok()
        .or_else(|| std::env::var("LINES").ok())
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(24);
    (cols, rows)
}

fn format_time(ts: u64) -> String {
    let ts_i64 = ts as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&ts_i64, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}
