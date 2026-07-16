pub(crate) fn fit_line(text: &str, cols: usize) -> String {
    let max = cols.max(20);
    let trimmed = truncate(text, max);
    format!("{:<width$}", trimmed, width = max)
}

pub(crate) fn pad_ansi_line(text: &str, cols: usize) -> String {
    let max = cols.max(20);
    let visible = visible_text_width(text);
    if visible >= max {
        return text.to_string();
    }
    format!("{}{}", text, " ".repeat(max - visible))
}

pub(crate) fn rule(cols: usize) -> String {
    "-".repeat(cols.max(20))
}

pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let proposed_len = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };

        if proposed_len > width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

pub(crate) fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let marker = "...";
    let marker_len = marker.len().min(max);
    let keep = max.saturating_sub(marker_len) / 2;
    let suffix = max.saturating_sub(keep + marker_len);
    let start: String = value.chars().take(keep).collect();
    let end: String = value
        .chars()
        .rev()
        .take(suffix)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}{}", start, marker, end)
}

pub(crate) fn conversation_role_label(role: &str) -> String {
    match role {
        "owner" | "admin" => "conversation manager".to_string(),
        "member" => "trusted participant".to_string(),
        _ => role.to_string(),
    }
}

pub(crate) fn visible_text_width(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut idx = 0;
    let mut width = 0;

    while idx < bytes.len() {
        if bytes[idx] == 0x1b {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'[' {
                idx += 1;
                while idx < bytes.len() && bytes[idx] != b'm' {
                    idx += 1;
                }
                if idx < bytes.len() {
                    idx += 1;
                }
                continue;
            }
        }

        let ch = text[idx..].chars().next().unwrap();
        width += 1;
        idx += ch.len_utf8();
    }

    width
}
