fn render_tui(snapshot: &HomeSnapshot, state: &TuiState) -> Result<()> {
    let cols = term_cols();
    let rows = term_rows();
    let screen = build_tui_screen(snapshot, state, cols, rows);

    print!("{}", screen);
    io::stdout().flush()?;
    Ok(())
}

fn build_tui_screen(snapshot: &HomeSnapshot, state: &TuiState, cols: usize, rows: usize) -> String {
    let cols = terminal_paint_cols(cols);
    let body_width = cols.saturating_sub(4);
    let mut screen = String::new();
    let mut body = String::new();
    // Steady-state redraws repaint from the home position and clear to the end of the
    // alternate screen. This avoids old tail lines surviving shorter frames without
    // bringing back the heavier full-screen clear on every keypress.
    screen.push_str("\x1b[H\x1b[J");
    push_screen_line(&mut screen, &render_tabs(state.tab, cols));
    push_screen_line(&mut screen, &rule(cols));

    if state.show_help {
        render_help_tab(&mut body, body_width);
    } else {
        match state.tab {
            Tab::Home => render_home_tab(&mut body, snapshot, state, body_width),
            Tab::Inbox => render_inbox_tab(&mut body, snapshot, state, body_width),
            Tab::People => render_people_tab(&mut body, snapshot, state, body_width),
            Tab::Apps => render_apps_tab(&mut body, snapshot, state, body_width),
            Tab::System => render_system_tab(&mut body, snapshot, state, body_width),
        }
    }

    if let Some(notice) = state
        .notice
        .as_deref()
        .or(snapshot.notice.as_deref())
        .filter(|notice| should_render_notice(notice))
    {
        push_screen_blank(&mut body);
        push_screen_line(&mut body, &section_title("Notice", cols));
        for line in wrap_text(notice, body_width) {
            push_screen_line(&mut body, &format!("  {}", line));
        }
    }

    let header_lines = 2usize;
    let footer_lines = 3usize;
    let body_rows = rows.saturating_sub(header_lines + footer_lines);
    let body_lines = push_bounded_screen_body(&mut screen, &body, body_rows, cols);
    if body_lines < body_rows {
        for _ in 0..(body_rows - body_lines) {
            push_screen_blank(&mut screen);
        }
    }

    push_screen_blank(&mut screen);
    push_screen_line(&mut screen, &rule(cols));
    push_screen_line(&mut screen, &fit_line(tui_footer_text(state), cols));
    trim_trailing_screen_newline(&mut screen);
    screen
}

fn tui_footer_text(state: &TuiState) -> &'static str {
    if state.show_help {
        TUI_HELP_FOOTER_TEXT
    } else {
        TUI_FOOTER_TEXT
    }
}

fn terminal_paint_cols(cols: usize) -> usize {
    // Leave the final terminal column untouched. xterm-compatible terminals can
    // enter autowrap after a full-width line, and the following CRLF may scroll
    // the first row off the viewport.
    cols.saturating_sub(1).max(20)
}

fn push_bounded_screen_body(
    screen: &mut String,
    body: &str,
    max_rows: usize,
    cols: usize,
) -> usize {
    if max_rows == 0 {
        return 0;
    }
    let lines = body.split_terminator("\r\n").collect::<Vec<_>>();
    if lines.len() <= max_rows {
        let rendered = lines.len();
        for line in lines {
            push_screen_line(screen, line);
        }
        return rendered;
    }

    let visible_rows = max_rows.saturating_sub(1);
    for line in lines.iter().take(visible_rows) {
        push_screen_line(screen, line);
    }
    push_screen_line(screen, &fit_line("  ...", cols));
    max_rows
}

fn trim_trailing_screen_newline(screen: &mut String) {
    if screen.ends_with("\r\n") {
        screen.truncate(screen.len().saturating_sub(2));
    }
}

fn render_help_tab(buf: &mut String, width: usize) {
    push_screen_line(buf, "  Home CLI Controls");
    push_screen_blank(buf);
    for line in tui_control_help_lines() {
        for wrapped in wrap_text(&line, width) {
            push_screen_line(buf, &wrapped);
        }
    }
}

fn tui_control_help_lines() -> Vec<String> {
    command_contract()
        .controls
        .into_iter()
        .map(|control| format!("  {:<12} {}", control.key, control.description))
        .collect()
}

fn render_home_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let text_width = total_width.saturating_sub(2);
    let primary_actions = quick_launch_action_indices(snapshot);
    let active_notice = current_notice(state, snapshot);
    for line in render_home_actions(snapshot, &primary_actions, state.home_index, text_width) {
        push_screen_line(buf, &format!("  {}", fit_line(&line, total_width)));
    }

    let alerts = alerts_lines(snapshot, text_width, active_notice);
    if !alerts.is_empty() {
        push_screen_blank(buf);
        push_screen_line(
            buf,
            &format!("  {}", fit_line("Needs attention", total_width)),
        );
        for line in alerts {
            push_screen_line(buf, &format!("  {}", fit_line(&line, total_width)));
        }
    }

    if let Some(next) = home_next_step(snapshot, active_notice) {
        push_screen_blank(buf);
        push_screen_line(buf, &format!("  {}", fit_line("Next", total_width)));
        push_screen_line(buf, &format!("  {}", fit_line(&next, total_width)));
    }
}

fn render_inbox_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let entries = notification_entries(snapshot);
    let list = if entries.is_empty() {
        vec!["No inbox entries waiting.".to_string()]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                format!(
                    "{} {} [{}{}]",
                    selected_marker(idx == state.inbox_index),
                    entry.title,
                    entry.severity,
                    if entry.read { "" } else { ", new" }
                )
            })
            .collect::<Vec<_>>()
    };
    push_section_lines(&mut left, "Inbox", &list);

    let overview = vec![
        format!("Unread     {}", snapshot.notifications.unread_count),
        format!("Attention  {}", snapshot.notifications.attention_count),
        format!("Entries    {}", entries.len()),
    ];
    push_section_lines(&mut left, "Overview", &overview);

    if let Some(entry) = selected_notification(snapshot, state.inbox_index) {
        let mut details = vec![
            format!("Title      {}", entry.title),
            format!("Severity   {}", entry.severity),
            format!("Source     {}", entry.source_app),
            format!("State      {}", if entry.read { "read" } else { "unread" }),
        ];
        details.extend(wrap_with_label("Body", &entry.body, column_width));
        if let Some(action) = selected_notification_action(snapshot, state.inbox_index) {
            details.push(format!("Action     {}", action.label));
            details.push(format!(
                "ActionUse  {}",
                if action.ready { "ready" } else { "blocked" }
            ));
            details.push("Enter      run this inbox action and return here".to_string());
            if let Some(reason) = &action.reason {
                details.extend(wrap_with_label("Setup", reason, column_width));
            }
        } else if entry.action_ref.is_some() {
            details.push("Action     no longer available".to_string());
        } else {
            details.push("Action     informational only".to_string());
        }
        details.push("m          mark this inbox entry read".to_string());
        details.push("d          dismiss this inbox entry".to_string());
        push_section_lines(&mut right, "Selected", &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_people_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    push_section_lines(
        &mut left,
        "My Profile",
        &people_profile_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut left,
        "People",
        &people_contact_lines(snapshot, column_width),
    );

    push_section_lines(
        &mut right,
        "Discovery",
        &people_discovery_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut right,
        "Add People",
        &people_visible_peer_lines(snapshot, column_width),
    );
    push_section_lines(
        &mut right,
        "Requests",
        &people_request_lines(snapshot, column_width),
    );

    let people_actions = people_actions(snapshot);
    if !people_actions.is_empty() {
        let actions = people_actions
            .iter()
            .enumerate()
            .map(|(slot, action)| {
                format!(
                    "{} {} [{}]",
                    selected_marker(slot == state.people_index),
                    action.label,
                    if action.ready { "ready" } else { "setup" }
                )
            })
            .collect::<Vec<_>>();
        push_section_lines(&mut left, "Actions", &actions);
    }

    if let Some(action) = selected_people_action(snapshot, state.people_index) {
        let mut profile = vec![
            format!("Action     {}", action.label),
            format!(
                "State      {}",
                if action.ready { "ready" } else { "setup" }
            ),
            format!("Command    {}", action.command),
        ];
        if let Some(reason) = &action.reason {
            profile.extend(wrap_with_label("Prep", reason, column_width));
        } else {
            profile.push("Enter      run this People action and return home".to_string());
        }
        profile.extend(wrap_with_label("What", &action.description, column_width));
        push_section_lines(&mut right, "Selected Action", &profile);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_apps_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let entries = app_entries(snapshot);
    let list = render_app_list(&entries, state.app_index);
    push_section_lines(&mut left, "Apps", &list);

    if let Some(entry) = entries.get(state.app_index.min(entries.len().saturating_sub(1))) {
        let mut details = if entry.action_id.as_deref() == Some("chat-room") {
            chat_room_app_detail_lines(snapshot, entry, column_width)
        } else {
            let mut details = vec![
                format!("Surface    {}", entry.name),
                format!("State      {}", entry.state),
                format!("Category   {}", entry.category),
            ];
            if let Some(viewer) = app_entry_viewer_label(entry) {
                details.push(format!("Opens with {}", viewer));
            }
            let accepted_content = accepted_content_for_viewer(snapshot, &entry.name);
            if !accepted_content.is_empty() {
                details.push(format!("Accepts    {}", accepted_content.join(", ")));
            }
            if let Some(capsule) = find_capsule_fact(snapshot, &entry.name) {
                let requirements = capsule_requirement_titles(snapshot, capsule);
                if !requirements.is_empty() {
                    details.push(format!("Needs      {}", requirements.join(", ")));
                }
            }
            let available = capsule_executable_action_labels(snapshot, &entry.name);
            if !available.is_empty() {
                details.push(format!("Available  {}", available.join(", ")));
            }
            details.extend(wrap_with_label(
                "What it does",
                &entry.description,
                column_width,
            ));
            details.extend(wrap_with_label("Command", &entry.command, column_width));
            details
        };
        if let Some(action) = selected_app_action(snapshot, state.app_index) {
            if action.ready {
                details.push(if entry.is_control {
                    "Enter      run this room action and return here".to_string()
                } else {
                    "Enter      launch from Home".to_string()
                });
            } else {
                details.push(if entry.is_control {
                    "Enter      room action not ready yet".to_string()
                } else {
                    "Enter      not ready from Home yet".to_string()
                });
                if let Some(reason) = &action.reason {
                    details.extend(wrap_with_label("Setup", reason, column_width));
                }
            }
        } else if entry.action_id.as_deref() == Some("chat-room") {
            details.push(
                "Enter      no direct launch; review the room controls listed below in Apps"
                    .to_string(),
            );
        } else {
            details.push(
            "Enter      read-only in Home CLI; use Desktop or an explicit approved Home action"
                    .to_string(),
            );
        }
        push_section_lines(&mut right, &entry.label, &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn render_system_tab(buf: &mut String, snapshot: &HomeSnapshot, state: &TuiState, width: usize) {
    let total_width = width.max(60);
    let column_width = column_width(total_width);
    let mut left = Vec::new();
    let mut right = Vec::new();

    let actions = system_actions(snapshot);
    let action_lines = actions
        .iter()
        .enumerate()
        .map(|(slot, action)| {
            format!(
                "{} {} [{}]",
                selected_marker(slot == state.system_index),
                action.label,
                system_action_state_label(action)
            )
        })
        .collect::<Vec<_>>();
    push_section_lines(&mut left, "Shell", &action_lines);
    push_section_lines(&mut left, "Status", &system_status_summary_lines(snapshot));

    if let Some(action) = selected_system_action(snapshot, state.system_index) {
        let mut details = vec![
            format!("Action     {}", action.label),
            format!("State      {}", system_action_state_label(&action)),
            format!("Command    {}", action.command),
        ];
        if let Some(reason) = &action.reason {
            details.extend(wrap_with_label("Info", reason, column_width));
        } else {
            details.push("Enter      switch active Home shell".to_string());
        }
        details.extend(wrap_with_label("Scope", &action.description, column_width));
        push_section_lines(&mut right, "Action", &details);
    }

    render_two_columns(buf, &left, &right, total_width);
}

fn push_screen_line(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push_str("\r\n");
}

fn push_screen_blank(buf: &mut String) {
    buf.push_str("\r\n");
}

fn render_tabs(active: Tab, cols: usize) -> String {
    let tabs = DEFAULT_TABS
        .iter()
        .map(|tab| render_tab(active == *tab, tab.label()))
        .collect::<Vec<_>>()
        .join("  ");
    pad_ansi_line(&tabs, cols)
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Inbox => "Inbox",
            Tab::People => "People",
            Tab::Apps => "Apps",
            Tab::System => "System",
        }
    }
}

fn render_tab(active: bool, label: &str) -> String {
    if active {
        format!("\x1b[30;46;1m {} \x1b[0m", label)
    } else {
        format!("\x1b[2m{}\x1b[0m", label)
    }
}

fn render_two_columns(buf: &mut String, left: &[String], right: &[String], total_width: usize) {
    let total_width = total_width.max(60);
    if total_width < 90 {
        for line in left {
            push_screen_line(buf, &format!("  {}", fit_line(line, total_width)));
        }
        if !left.is_empty() && !right.is_empty() {
            push_screen_blank(buf);
        }
        for line in right {
            push_screen_line(buf, &format!("  {}", fit_line(line, total_width)));
        }
        return;
    }

    let gutter = 3usize;
    let left_width = (total_width - gutter) / 2;
    let right_width = total_width - gutter - left_width;
    let rows = left.len().max(right.len());

    for idx in 0..rows {
        let left_line = left
            .get(idx)
            .map(|line| fit_line(line, left_width))
            .unwrap_or_else(|| " ".repeat(left_width));
        let right_line = right
            .get(idx)
            .map(|line| fit_line(line, right_width))
            .unwrap_or_else(|| " ".repeat(right_width));
        push_screen_line(
            buf,
            &format!("  {}{}{}", left_line, " ".repeat(gutter), right_line),
        );
    }
}

fn push_section_lines(target: &mut Vec<String>, title: &str, lines: &[String]) {
    target.push(title.to_string());
    target.extend(lines.iter().cloned());
}

fn wrap_with_label(label: &str, text: &str, width: usize) -> Vec<String> {
    let first_width = width.saturating_sub(label.len() + 2).max(12);
    let rest_width = width.max(20);
    let wrapped = wrap_text(text, first_width);
    let mut lines = Vec::new();
    if let Some(first) = wrapped.first() {
        lines.push(format!("{:<10} {}", label, first));
        for line in wrapped.iter().skip(1) {
            lines.push(format!(
                "{:<10} {}",
                "",
                fit_line(line, rest_width.saturating_sub(11))
            ));
        }
    }
    lines
}

fn column_width(total_width: usize) -> usize {
    if total_width < 90 {
        total_width.max(20)
    } else {
        ((total_width - 3) / 2).max(20)
    }
}

fn selected_marker(selected: bool) -> &'static str {
    if selected {
        ">"
    } else {
        " "
    }
}

fn people_overview_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut lines = people_profile_lines(snapshot, width);
    lines.push(format!("Contacts   {}", snapshot.people.contacts.len()));
    lines.push(format!(
        "Discovery  {}",
        people_discovery_state_label(&snapshot.people.discovery)
    ));
    let peers = people_visible_peers(snapshot);
    lines.push(format!("Visible    {}", peers.len()));
    let requests = people_visible_requests(snapshot);
    lines.push(format!("Requests   {}", requests.len()));
    lines
}

fn people_profile_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut lines = vec![
        format!("Display    {}", display_name(snapshot)),
        format!("Identity   {}", identity_summary(snapshot)),
    ];
    if !snapshot.user.trim().is_empty() && snapshot.user != display_name(snapshot) {
        lines.extend(wrap_with_label("User", &snapshot.user, width));
    }
    lines
}

fn people_contact_lines(snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    if snapshot.people.contacts.is_empty() {
        return vec!["No people yet. Turn on Discovery to find another ElastOS home.".to_string()];
    }
    snapshot
        .people
        .contacts
        .iter()
        .take(8)
        .flat_map(|contact| {
            let mut lines = vec![format!(
                "{} - {}",
                people_contact_display_name(contact, "Person"),
                if contact.relationship.trim().is_empty() {
                    "connected"
                } else {
                    contact.relationship.as_str()
                }
            )];
            let mut details = Vec::new();
            if let Some(handle) = contact
                .handle
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                details.push(handle.to_string());
            }
            if let Some(device) = contact
                .device_label
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                details.push(device.to_string());
            }
            if people_contact_message_target(contact).is_some() {
                lines.push(format!("  Message  people message {}", contact.contact_id));
            }
            if !contact.contact_id.trim().is_empty() {
                lines.push(format!("  Remove   people remove {}", contact.contact_id));
            }
            if !details.is_empty() {
                lines.extend(wrap_with_label(" ", &details.join(" - "), width));
            }
            lines
        })
        .collect()
}

fn people_discovery_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let discovery = &snapshot.people.discovery;
    let mut lines = vec![
        format!("State      {}", people_discovery_state_label(discovery)),
        format!(
            "Status     {}",
            if discovery.status_message.trim().is_empty() {
                discovery.status.as_str()
            } else {
                discovery.status_message.as_str()
            }
        ),
    ];
    if discovery.enabled {
        lines.push(format!(
            "Remaining  {}",
            people_discovery_remaining_text(discovery.remaining_seconds.unwrap_or(0))
        ));
    }
    let peers = people_visible_peers(snapshot);
    if !peers.is_empty() {
        lines.push(format!("Visible    {}", peers.len()));
    }
    let requests = people_visible_requests(snapshot);
    if !requests.is_empty() {
        lines.push(format!("Requests   {}", requests.len()));
    }
    lines
}

fn people_visible_peer_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let peers = people_visible_peers(snapshot);
    if peers.is_empty() {
        return vec![
            "No people visible for adding yet.".to_string(),
            "Use Turn On or Refresh while another ElastOS home is discoverable.".to_string(),
        ];
    }
    peers
        .into_iter()
        .take(8)
        .map(|peer| {
            let mut line = format!(
                "{} - {}",
                people_peer_display_name(peer, "Visible person"),
                if peer.status.trim().is_empty() {
                    "visible"
                } else {
                    peer.status.as_str()
                }
            );
            if !peer.peer_id.trim().is_empty() {
                line.push_str(&format!(" - Add: people request {}", peer.peer_id));
            }
            line
        })
        .collect()
}

fn people_request_lines(snapshot: &HomeSnapshot, _width: usize) -> Vec<String> {
    let requests = people_visible_requests(snapshot);
    if requests.is_empty() {
        return vec!["No People requests waiting.".to_string()];
    }
    requests
        .into_iter()
        .take(8)
        .map(|request| {
            let mut line = format!(
                "{} - {}",
                people_request_display_name(request, "Person"),
                if request.status.trim().is_empty() {
                    "requested"
                } else {
                    request.status.as_str()
                }
            );
            if request.status == "incoming" && !request.request_id.trim().is_empty() {
                line.push_str(&format!(" - Accept: people accept {}", request.request_id));
            }
            line
        })
        .collect()
}

fn people_debug_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("You        {}", snapshot.user),
        format!("Nick       {}", display_name(snapshot)),
        format!("Identity   {}", identity_summary(snapshot)),
        format!("ContactsSchema {}", snapshot.people.schema),
        format!("Contacts   {}", snapshot.people.contact_count),
        format!("Services   {}", snapshot.people.service_offer_count),
        format!(
            "Discovery  {}",
            people_discovery_state_label(&snapshot.people.discovery)
        ),
        format!("DiscoverySchema {}", snapshot.people.discovery.schema),
        format!("Topic      {}", snapshot.people.discovery.topic),
        format!(
            "LocalPeer  {}",
            snapshot
                .people
                .discovery
                .local_peer_id
                .as_deref()
                .unwrap_or("not advertised")
        ),
        format!("Network    {}", network_summary(snapshot)),
        format!(
            "Source     {}",
            snapshot
                .source
                .as_ref()
                .map(|source| source.name.as_str())
                .unwrap_or("unknown")
        ),
        format!(
            "Profile    {}",
            action_state_label(action_by_id(snapshot, "identity-nickname-set"))
        ),
        format!(
            "Chat       {}",
            action_state_label(action_by_id(snapshot, "chat"))
        ),
        format!(
            "Peers      {}",
            format!(
                "{} endpoints reachable",
                snapshot.runtime.peer_count.unwrap_or_default()
            )
        ),
    ];
    if let Some(ticket) = &snapshot.runtime.ticket {
        lines.push(format!("Ticket     {}", truncate(ticket, 42)));
    } else {
        lines.push("Ticket     waiting for runtime".to_string());
    }
    if let Some(delay) = snapshot.people.discovery.next_refresh_after_ms {
        lines.push(format!("RefreshMs  {delay}"));
    }
    for contact in snapshot.people.contacts.iter().take(3) {
        if let Some(last_seen_at) = contact.last_seen_at {
            lines.push(format!(
                "ContactSeen {} {}",
                people_contact_display_name(contact, "Person"),
                last_seen_at
            ));
        }
    }
    for peer in snapshot.people.discovery.discovered_peers.iter().take(3) {
        if peer.last_seen_at > 0 {
            lines.push(format!(
                "PeerSeen   {} {}",
                people_peer_display_name(peer, "Visible person"),
                peer.last_seen_at
            ));
        }
    }
    for request in snapshot.people.discovery.requests.iter().take(3) {
        if request.created_at > 0 {
            lines.push(format!(
                "ReqCreated {} {}",
                people_request_display_name(request, "Person"),
                request.created_at
            ));
        }
        if let Some(invite_id) = request
            .invite_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            lines.push(format!("ReqInvite  {}", truncate(invite_id, 42)));
        }
    }
    lines.push(format!(
        "RoomGuests {}",
        if snapshot.room.allow_guest_invites {
            "public join requests enabled"
        } else {
            "public join requests disabled"
        }
    ));
    lines.push(format!(
        "RoomUsers  {}",
        if snapshot.room.allow_member_invites {
            "ElastOS user invites enabled"
        } else {
            "ElastOS user invites disabled"
        }
    ));
    lines.push(format!("RoomReqs   {}", snapshot.room.pending_count));
    lines.push(format!("RoomWeb    {}", snapshot.room.active_session_count));
    lines.push("Manage     elastos identity nickname set".to_string());
    lines
}

fn spaces_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("MyWebSite  {}", website_summary(snapshot)),
        format!(
            "Public     {} shared channel{} ready to open",
            snapshot.shares.channel_count,
            if snapshot.shares.channel_count == 1 {
                ""
            } else {
                "s"
            }
        ),
        "Local      scratch space for temporary work and session state".to_string(),
        "WebSpaces  named handles into content, peers, identity, and AI".to_string(),
    ]
}

fn apps_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let entries = app_entries(snapshot)
        .into_iter()
        .filter(|entry| !entry.is_control)
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut last_category = "";
    for entry in entries.into_iter().take(8) {
        if entry.category != last_category {
            lines.push(format!("{}:", entry.category));
            last_category = entry.category;
        }
        lines.push(format!(
            "  {} [{}]",
            app_entry_display_label(&entry),
            entry.state
        ));
    }
    lines
}

fn system_settings_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    lines.extend(system_shell_summary_lines(snapshot));
    let sign_out = sign_out_action(snapshot);
    lines.push(if sign_out.ready {
        "Account    sign out with `signout`".to_string()
    } else {
        "Session    close with `exit`".to_string()
    });
    lines.extend(system_status_summary_lines(snapshot));
    lines
}

fn system_shell_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let active = active_shell_label(snapshot);
    let action = shell_switch_home_gui_action(snapshot);
    let mut lines = vec![
        format!("Active     {active}"),
        format!("Session    {}", home_cli_session_summary_label(snapshot)),
        format!("Desktop    {}", system_action_state_label(&action)),
        format!(
            "Home CLI   {}",
            if active == "home-cli" {
                "current"
            } else {
                "available"
            }
        ),
    ];
    if action.ready {
        lines.push(
            "Enter      return to the graphical Home desktop, or run `system shell home-gui`"
                .to_string(),
        );
    } else if let Some(reason) = action.reason {
        lines.extend(wrap_with_label("Info", &reason, 80));
    }
    lines
}

fn system_update_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("Policy     {}", source_update_policy_label(snapshot)),
        format!("Source     {}", source_label(snapshot)),
        format!("Channel    {}", source_channel_label(snapshot)),
        format!("Runtime    {}", snapshot.version),
        "Apply      no automatic update is run from Home CLI".to_string(),
    ]
}

fn system_shell_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let action = shell_switch_home_gui_action(snapshot);
    let mut lines = vec![format!(
        "Shell      active {}",
        active_shell_label(snapshot)
    )];
    if action.ready {
        lines.push("Switch     system shell home-gui".to_string());
    } else if let Some(reason) = action.reason {
        lines.extend(wrap_with_label("Switch", &reason, 80));
    }
    lines
}

fn system_status_summary_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("Home       {}", system_runtime_summary_label(snapshot)),
        format!(
            "Updates    {} / {}",
            source_brief_label(snapshot),
            source_update_policy_label(snapshot)
        ),
    ];
    if snapshot.notifications.attention_count > 0 || snapshot.notifications.unread_count > 0 {
        lines.push(format!(
            "Inbox      {} attention / {} unread",
            snapshot.notifications.attention_count, snapshot.notifications.unread_count
        ));
    }
    lines
}

fn system_identity_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("User       {}", snapshot.user),
        format!("Display    {}", display_name(snapshot)),
        format!("Shell      {}", active_shell_label(snapshot)),
        format!("Session    {}", home_cli_session_summary_label(snapshot)),
    ]
}

fn home_cli_session_summary_label(snapshot: &HomeSnapshot) -> String {
    match snapshot.session.mode.trim() {
        "browser_pty" => "browser terminal".to_string(),
        "native_terminal" => "native terminal".to_string(),
        "" => "home-cli".to_string(),
        other => other.to_string(),
    }
}

fn system_diagnostics_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    vec![
        format!("Runtime    {}", runtime_state_label(snapshot)),
        format!("Version    {}", snapshot.version),
        format!("Session    {}", home_cli_session_summary_label(snapshot)),
        format!("Shell      {}", active_shell_label(snapshot)),
        format!("Source     {}", source_brief_label(snapshot)),
        format!(
            "Inbox      {} attention",
            snapshot.notifications.attention_count
        ),
    ]
}

#[cfg(test)]
fn compact_system_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    system_settings_lines(snapshot)
}

fn active_shell_label(snapshot: &HomeSnapshot) -> String {
    snapshot
        .active_shell
        .active
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("home-cli")
        .to_string()
}

fn active_shell_candidate_launchable(snapshot: &HomeSnapshot, name: &str) -> bool {
    snapshot
        .active_shell
        .candidates
        .iter()
        .any(|candidate| candidate.name == name && candidate.launchable)
}

fn root_group_name(root: &str) -> &'static str {
    match root {
        "Users" | "UsersAI" => "People",
        "Local" | "Public" | "MyWebSite" | "WebSpaces" => "Spaces",
        "AppCapsules" => "Apps",
        "ElastOS" => "System",
        _ => "World",
    }
}

fn truncate_did(did: &str) -> String {
    truncate(did, 36)
}

fn network_summary(snapshot: &HomeSnapshot) -> String {
    if !snapshot.runtime.running {
        return "home session not running yet".to_string();
    }

    let peers = snapshot.runtime.peer_count.unwrap_or(0);
    if peers == 0 {
        if snapshot.runtime.ticket.is_some() {
            "Carrier bootstrap ready; waiting for another participant".to_string()
        } else {
            "starting up".to_string()
        }
    } else if peers == 1 {
        "1 Carrier endpoint reachable".to_string()
    } else {
        format!("{} Carrier endpoints reachable", peers)
    }
}

fn identity_summary(snapshot: &HomeSnapshot) -> String {
    snapshot
        .did
        .as_deref()
        .map(truncate_did)
        .unwrap_or_else(|| "not initialized yet".to_string())
}

fn display_name(snapshot: &HomeSnapshot) -> String {
    snapshot
        .nickname
        .as_deref()
        .filter(|nick| !nick.is_empty())
        .unwrap_or(&snapshot.user)
        .to_string()
}

fn website_summary(snapshot: &HomeSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(url) = snapshot.site.local_url.as_deref() {
        parts.push(format!("preview at {}", url.trim_end_matches('/')));
    } else if snapshot.site.staged {
        parts.push("staged at localhost://MyWebSite".to_string());
    } else {
        parts.push("not staged locally".to_string());
    }

    if let Some(release) = snapshot.site.active_release.as_deref() {
        if let Some(channel) = snapshot.site.active_channel.as_deref() {
            parts.push(format!("live {} on {}", release, channel));
        } else {
            parts.push(format!("release {}", release));
        }
    } else if snapshot.site.release_count > 0 {
        let suffix = if snapshot.site.release_count == 1 {
            ""
        } else {
            "s"
        };
        parts.push(format!(
            "{} saved release{}",
            snapshot.site.release_count, suffix
        ));
    }

    if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
        parts.push(format!("elastos://{}", truncate(cid, 18)));
    }

    parts.join(" - ")
}

fn source_label(snapshot: &HomeSnapshot) -> String {
    match &snapshot.source {
        Some(source) => {
            let name = if source.name == "default" {
                "default".to_string()
            } else {
                source.name.clone()
            };
            match &source.gateway {
                Some(gateway) => {
                    let host = gateway
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/');
                    if name == host {
                        host.to_string()
                    } else {
                        format!("{} via {}", name, host)
                    }
                }
                None => name,
            }
        }
        None => "no trusted source configured".to_string(),
    }
}

fn source_brief_label(snapshot: &HomeSnapshot) -> String {
    let Some(source) = snapshot.source.as_ref() else {
        return "not configured".to_string();
    };
    let name = source.name.trim();
    let name = if name.is_empty() { "default" } else { name };
    format!("{} - {}", name, source_channel_label(snapshot))
}

fn source_channel_label(snapshot: &HomeSnapshot) -> String {
    snapshot
        .source
        .as_ref()
        .map(|source| {
            let channel = source.channel.trim();
            if channel.is_empty() {
                "stable"
            } else {
                channel
            }
        })
        .unwrap_or("not configured")
        .to_string()
}

fn source_update_policy_label(snapshot: &HomeSnapshot) -> String {
    let Some(source) = snapshot.source.as_ref() else {
        return "disabled (no trusted source)".to_string();
    };
    if snapshot.version.contains("dev") {
        return "disabled in dev builds; use explicit source/update commands".to_string();
    }
    let channel = if source.channel.trim().is_empty() {
        "stable"
    } else {
        source.channel.trim()
    };
    format!("manual updates allowed on {channel}")
}

fn section_title(title: &str, cols: usize) -> String {
    fit_line(title, cols)
}
