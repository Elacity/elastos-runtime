fn home_action_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    prioritized_action_indices(snapshot, HOME_ACTION_IDS)
}

fn people_actions(snapshot: &HomeSnapshot) -> Vec<PeopleAction> {
    let mut actions = Vec::new();
    for contact in &snapshot.people.contacts {
        let name = people_contact_display_name(contact, "Person");
        if people_contact_message_target(contact).is_some() && !contact.contact_id.trim().is_empty()
        {
            actions.push(PeopleAction {
                id: format!("people-message:{}", contact.contact_id),
                label: format!("Chat with {name}"),
                description: format!("Open Chat for {name} through the Runtime People route."),
                command: format!("people message {}", contact.contact_id),
                ready: true,
                reason: None,
            });
        }
        if !contact.contact_id.trim().is_empty() {
            actions.push(PeopleAction {
                id: format!("people-remove-contact:{}", contact.contact_id),
                label: format!("Remove {name}"),
                description: "Remove this person from People through the Runtime People route."
                    .to_string(),
                command: format!("people remove {}", contact.contact_id),
                ready: true,
                reason: None,
            });
        }
    }
    actions
}

fn selected_people_action(snapshot: &HomeSnapshot, selected: usize) -> Option<PeopleAction> {
    let actions = people_actions(snapshot);
    actions
        .get(selected.min(actions.len().saturating_sub(1)))
        .cloned()
}

fn system_actions(snapshot: &HomeSnapshot) -> Vec<SystemAction> {
    vec![
        shell_switch_home_gui_action(snapshot),
        sign_out_action(snapshot),
    ]
}

fn shell_switch_home_gui_action(snapshot: &HomeSnapshot) -> SystemAction {
    let active = active_shell_label(snapshot);
    let mode = snapshot.session.mode.trim();
    let (ready, reason) = if active == "home-gui" {
        (false, Some("Desktop is already active".to_string()))
    } else if mode != "browser_pty" {
        (
            false,
            Some("native terminal has no browser root shell to switch".to_string()),
        )
    } else if !active_shell_candidate_launchable(snapshot, "home-gui") {
        (false, Some("Desktop is not available".to_string()))
    } else {
        (true, None)
    };

    SystemAction {
        id: "shell-switch:home-gui".to_string(),
        label: "Return to Home Desktop".to_string(),
        description: "Switch the active Home shell back to the graphical desktop.".to_string(),
        command: "system shell home-gui".to_string(),
        ready,
        reason,
    }
}

fn sign_out_action(snapshot: &HomeSnapshot) -> SystemAction {
    let browser_session = snapshot.session.mode.trim() == "browser_pty";
    SystemAction {
        id: "auth-sign-out".to_string(),
        label: "Sign out".to_string(),
        description: "End this browser Home session.".to_string(),
        command: "signout".to_string(),
        ready: browser_session,
        reason: (!browser_session).then(|| "Use exit to close native Home CLI".to_string()),
    }
}

fn system_action_state_label(action: &SystemAction) -> &'static str {
    if action.ready {
        "ready"
    } else if action
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("already active"))
    {
        "current"
    } else {
        "unavailable"
    }
}

fn selected_system_action(snapshot: &HomeSnapshot, selected: usize) -> Option<SystemAction> {
    let actions = system_actions(snapshot);
    actions
        .get(selected.min(actions.len().saturating_sub(1)))
        .cloned()
}

fn home_app_target_from_route(route: &str) -> Option<String> {
    let rest = route.trim().strip_prefix("/apps/")?;
    let target = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

fn people_contact_message_target(contact: &PeopleContactStatus) -> Option<String> {
    if !contact.can_message {
        return None;
    }
    home_app_target_from_route(&contact.route)
}

fn people_contact_id_for_reference(snapshot: &HomeSnapshot, contact_ref: &str) -> Option<String> {
    let needle = people_contact_lookup_key(contact_ref);
    if needle.is_empty() {
        return None;
    }
    snapshot
        .people
        .contacts
        .iter()
        .find(|contact| people_contact_reference_matches(contact, &needle))
        .and_then(|contact| {
            let contact_id = contact.contact_id.trim();
            if contact_id.is_empty() {
                None
            } else {
                Some(contact_id.to_string())
            }
        })
}

fn people_contact_reference_matches(contact: &PeopleContactStatus, needle: &str) -> bool {
    let matches = |value: &str| {
        let key = people_contact_lookup_key(value);
        !key.is_empty() && key == needle
    };
    matches(&contact.contact_id)
        || matches(&people_contact_display_name(contact, ""))
        || contact.handle.as_deref().is_some_and(matches)
        || contact.device_label.as_deref().is_some_and(matches)
        || contact.profile_card.as_ref().is_some_and(|profile| {
            matches(&profile.display_name) || profile.handle.as_deref().is_some_and(matches)
        })
}

fn people_contact_lookup_key(value: &str) -> String {
    let value = value.trim();
    value.strip_prefix('@').unwrap_or(value).to_lowercase()
}

fn people_contact_display_name(contact: &PeopleContactStatus, fallback: &str) -> String {
    let profile = contact.profile_card.as_ref();
    let display_name = profile
        .map(|profile| profile.display_name.as_str())
        .unwrap_or("")
        .trim();
    if !display_name.is_empty() && display_name != "ElastOS user" {
        return display_name.to_string();
    }
    let direct = contact.display_name.trim();
    if !direct.is_empty() && direct != "ElastOS user" {
        return direct.to_string();
    }
    profile
        .and_then(|profile| profile.handle.as_deref())
        .or(contact.handle.as_deref())
        .or(contact.device_label.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn notification_entries(snapshot: &HomeSnapshot) -> &[NotificationEntryStatus] {
    &snapshot.notifications.entries
}

fn notification_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    (0..notification_entries(snapshot).len()).collect()
}

fn quick_launch_action_indices(snapshot: &HomeSnapshot) -> Vec<usize> {
    home_action_indices(snapshot)
}

fn prioritized_action_indices(snapshot: &HomeSnapshot, ids: &[&str]) -> Vec<usize> {
    let mut indices = Vec::new();
    for id in ids {
        if let Some(idx) = snapshot.actions.iter().position(|action| action.id == *id) {
            if snapshot.actions[idx].ready && !indices.contains(&idx) {
                indices.push(idx);
            }
        }
    }
    indices
}

fn selected_action<'a>(
    snapshot: &'a HomeSnapshot,
    indices: &[usize],
    selected: usize,
) -> Option<&'a ActionInfo> {
    let idx = indices.get(selected.min(indices.len().saturating_sub(1)))?;
    snapshot.actions.get(*idx)
}

fn selected_notification(
    snapshot: &HomeSnapshot,
    selected: usize,
) -> Option<&NotificationEntryStatus> {
    let entries = notification_entries(snapshot);
    entries.get(selected.min(entries.len().saturating_sub(1)))
}

fn selected_notification_read_action(snapshot: &HomeSnapshot, selected: usize) -> Option<String> {
    let entry = selected_notification(snapshot, selected)?;
    Some(format!("notification-read:{}", entry.id))
}

fn selected_notification_dismiss_action(
    snapshot: &HomeSnapshot,
    selected: usize,
) -> Option<String> {
    let entry = selected_notification(snapshot, selected)?;
    Some(format!("notification-dismiss:{}", entry.id))
}

fn selected_notification_action(snapshot: &HomeSnapshot, selected: usize) -> Option<ActionInfo> {
    let entry = selected_notification(snapshot, selected)?;
    let action_ref = entry.action_ref.as_ref()?;
    let action_id = action_ref.action_id.trim();
    if let Some(action) = action_by_id(snapshot, action_id) {
        return Some(action.clone());
    }
    inbox_review_handoff_action(snapshot, entry, action_ref)
}

fn inbox_review_handoff_action(
    snapshot: &HomeSnapshot,
    entry: &NotificationEntryStatus,
    action_ref: &NotificationActionRefStatus,
) -> Option<ActionInfo> {
    if !notification_action_uses_inbox_review(action_ref.action_id.trim()) {
        return None;
    }
    let notification_id = entry.id.trim();
    if notification_id.is_empty() {
        return None;
    }
    let ready = snapshot.session.mode.trim() == "browser_pty"
        && active_shell_candidate_launchable(snapshot, "home-gui")
        && snapshot
            .targets
            .iter()
            .any(|target| target.target == INBOX_TARGET_ID && target.target_kind == "app");
    Some(ActionInfo {
        id: format!("{INBOX_NOTIFICATION_HANDOFF_ACTION_PREFIX}{notification_id}"),
        label: "Open Inbox on Desktop".to_string(),
        description: "Open Inbox on the Home Desktop to review this pending request.".to_string(),
        command: "home: open Inbox".to_string(),
        ready,
        reason: (!ready).then(|| {
            "Open Inbox from the Home Desktop to review this pending request.".to_string()
        }),
    })
}

fn notification_action_uses_inbox_review(action_id: &str) -> bool {
    [
        "contact-accept-request:",
        "wallet-approve-request:",
        "wallet-review-request:",
        "capability-approve-request:",
        "inspect-approve-request:",
        "wallet-price-http-approve:",
    ]
    .iter()
    .any(|prefix| action_id.starts_with(prefix))
}

fn selected_app_action(snapshot: &HomeSnapshot, selected: usize) -> Option<&ActionInfo> {
    let entries = app_entries(snapshot);
    let entry = entries.get(selected.min(entries.len().saturating_sub(1)))?;
    let action_id = entry.action_id.as_deref()?;
    action_by_id(snapshot, action_id)
}

fn action_by_id<'a>(snapshot: &'a HomeSnapshot, id: &str) -> Option<&'a ActionInfo> {
    snapshot.actions.iter().find(|action| action.id == id)
}

fn action_state_label(action: Option<&ActionInfo>) -> String {
    match action {
        Some(action) if action.ready => "ready".to_string(),
        Some(action) => format!(
            "blocked ({})",
            action.reason.as_deref().unwrap_or("setup needed")
        ),
        None => "not available".to_string(),
    }
}

fn next_step_command(reason: &str) -> Option<&str> {
    reason
        .split_once("run: ")
        .map(|(_, command)| command.trim())
}

fn render_home_actions(
    snapshot: &HomeSnapshot,
    indices: &[usize],
    selected: usize,
    width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (slot, action_idx) in indices.iter().take(5).enumerate() {
        let action = &snapshot.actions[*action_idx];
        let state = home_action_state(action, snapshot);
        let summary = home_action_summary(action);
        let label = action_display_label(action);
        lines.push(format!(
            "{} {} {} [{}]  {}",
            selected_marker(slot == selected),
            slot + 1,
            label,
            state,
            truncate(summary, width.saturating_sub(label.len() + 18).max(16))
        ));
        if let Some(reason) = &action.reason {
            lines.push(format!(
                "    setup: {}",
                truncate(reason, width.saturating_sub(11).max(16))
            ));
        }
    }
    lines
}

fn home_action_state<'a>(action: &'a ActionInfo, snapshot: &HomeSnapshot) -> &'a str {
    match action.id.as_str() {
        "site-local" => {
            if snapshot.site.local_url.is_some() {
                "preview"
            } else if snapshot.site.staged && !action.ready {
                "staged"
            } else if !snapshot.site.staged {
                "empty"
            } else if action.ready {
                "ready"
            } else {
                "setup"
            }
        }
        _ => {
            if action.ready {
                "ready"
            } else {
                "setup"
            }
        }
    }
}

fn home_action_summary(action: &ActionInfo) -> &str {
    match action.id.as_str() {
        "chat" => "Send a message and return home",
        "room-approve" => "Approve the next pending Chat web guest request",
        "room-deny" => "Deny the next pending Chat web guest request",
        "room-revoke-all" => "Disconnect active Chat web guest sessions",
        "site-local" => "Start or reuse the local MyWebSite preview",
        "site-ephemeral" => "Publish a temporary public HTTPS URL for MyWebSite",
        "site-open" => "Open the MyWebSite preview in a browser",
        "shares-list" => "Review shared channels, open links, and next steps",
        _ => action.description.as_str(),
    }
}

fn action_display_label(action: &ActionInfo) -> &str {
    match action.id.as_str() {
        "chat" => "Chat",
        "room-approve" => "Approve access",
        "room-deny" => "Deny access",
        "room-revoke-all" => "Disconnect browsers",
        "site-local" => "Preview",
        "site-ephemeral" => "Publish",
        "site-open" => "Open",
        "shares-list" => "Shared",
        _ => action.label.as_str(),
    }
}

fn current_notice<'a>(state: &'a TuiState, snapshot: &'a HomeSnapshot) -> Option<&'a str> {
    state.notice.as_deref().or(snapshot.notice.as_deref())
}

fn alerts_lines(snapshot: &HomeSnapshot, width: usize, notice: Option<&str>) -> Vec<String> {
    let mut alerts = Vec::new();
    let notice = notice.unwrap_or("").trim().to_string();
    if !notice.is_empty() {
        alerts.push(notice.clone());
    }
    if snapshot.did.is_none() {
        alerts.push(
            "Identity is not initialized yet. Run elastos setup to create the local DID."
                .to_string(),
        );
    }
    if site_workflow_started(snapshot) && !snapshot.site.staged {
        alerts.push(
            "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`."
                .to_string(),
        );
    }
    for entry in snapshot.notifications.entries.iter().take(3) {
        alerts.push(entry.body.clone());
    }
    if snapshot.notifications.entries.len() > 3 {
        alerts.push(format!(
            "{} more inbox notification(s) waiting.",
            snapshot.notifications.entries.len() - 3
        ));
    }
    if snapshot.room.active_session_count > 0 {
        alerts.push(format!(
            "Chat has {} active web guest session(s): {}.",
            snapshot.room.active_session_count,
            format_room_participants(&snapshot.room.active_participants)
        ));
    }
    if snapshot.source.is_none() {
        alerts.push(
            "No trusted release source is configured yet, so update flows stay manual.".to_string(),
        );
    }
    alerts
        .into_iter()
        .filter(|item| item.trim() == notice || !notice_covers_alert(&notice, item))
        .flat_map(|item| wrap_text(&item, width))
        .collect()
}

fn home_next_step(snapshot: &HomeSnapshot, notice: Option<&str>) -> Option<String> {
    if notice.is_some_and(|value| !value.trim().is_empty()) {
        return None;
    }
    if snapshot.did.is_none() {
        return Some("Run `elastos setup` to create the local DID.".to_string());
    }
    if snapshot.notifications.attention_count > 0 || !snapshot.notifications.entries.is_empty() {
        return Some("Open Inbox to handle the waiting item.".to_string());
    }
    if let Some(action) = first_blocked_home_action(snapshot) {
        return Some(format!(
            "{}: {}",
            action_display_label(action),
            action.reason.as_deref().unwrap_or("setup needed")
        ));
    }
    if snapshot.room.active_session_count > 0 {
        return Some(
            "Use Disconnect browsers when the shared Chat session should end.".to_string(),
        );
    }
    if site_workflow_started(snapshot) && !snapshot.site.staged {
        return Some(
            "Run `mywebsite stage <dir>` when you are ready to publish a site.".to_string(),
        );
    }
    if quick_launch_action_indices(snapshot).is_empty() {
        return Some("Use Tab to choose Inbox, People, Apps, or System.".to_string());
    }
    Some("Press Enter for the selected action, or Tab to choose another area.".to_string())
}

fn first_blocked_home_action(snapshot: &HomeSnapshot) -> Option<&ActionInfo> {
    HOME_ACTION_IDS.iter().find_map(|id| {
        snapshot
            .actions
            .iter()
            .find(|action| action.id == *id && !action.ready)
    })
}

fn site_workflow_started(snapshot: &HomeSnapshot) -> bool {
    snapshot.site.staged
        || snapshot.site.local_url.is_some()
        || snapshot.site.active_release.is_some()
        || snapshot.site.active_channel.is_some()
        || snapshot.site.active_bundle_cid.is_some()
        || snapshot.site.release_count > 0
}

fn notice_covers_alert(notice: &str, alert: &str) -> bool {
    if notice.is_empty() {
        return false;
    }

    let notice = notice.trim();
    let alert = alert.trim();

    notice == alert
        || notice.starts_with(alert)
        || alert.starts_with(notice)
        || (notice.contains("MyWebSite is empty.") && alert.contains("MyWebSite is empty."))
}

fn format_room_participants(participants: &[RoomParticipantStatus]) -> String {
    if participants.is_empty() {
        return "browser room active".to_string();
    }
    participants
        .iter()
        .take(3)
        .map(|participant| {
            format!(
                "{} on {}",
                participant.display_name, participant.device_label
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn runtime_state_label(snapshot: &HomeSnapshot) -> String {
    if !snapshot.runtime.running {
        return "offline".to_string();
    }
    snapshot
        .runtime
        .kind
        .clone()
        .unwrap_or_else(|| "running".to_string())
}

fn system_runtime_summary_label(snapshot: &HomeSnapshot) -> &'static str {
    if snapshot.runtime.running {
        "ready"
    } else {
        "offline"
    }
}

fn should_render_notice(notice: &str) -> bool {
    let trimmed = notice.trim();
    !trimmed.is_empty()
        && trimmed != "Home is live. Launch an app and you return here automatically when it exits."
        && trimmed != "Snapshot refreshed from live local state."
        && !trimmed.starts_with("Returned home from ")
}

fn space_detail_lines(root: &RootStatus, snapshot: &HomeSnapshot, width: usize) -> Vec<String> {
    let mut details = vec![
        format!("Group      {}", root_group_name(&root.name)),
        format!("Kind       {}", root.kind),
        format!("URI        {}", root.uri),
        format!("Exists     {}", if root.exists { "yes" } else { "no" }),
    ];
    if let Some(path) = &root.path {
        details.push(format!("Path       {}", path));
    }
    details.extend(wrap_with_label("Meaning", &root.description, width));
    details.extend(wrap_with_label("Example", &root.example, width));
    match root.name.as_str() {
        "MyWebSite" => {
            details.push(format!("State      {}", website_summary(snapshot)));
            if let Some(url) = snapshot.site.local_url.as_deref() {
                details.push(format!("Preview    {}", url.trim_end_matches('/')));
            } else if let Some(action) = action_by_id(snapshot, "site-local") {
                if action.ready {
                    details.push("Preview    mywebsite preview".to_string());
                } else if let Some(reason) = action.reason.as_deref() {
                    if let Some(command) = next_step_command(reason) {
                        details.push(format!("Next       {}", command));
                    } else {
                        details.extend(wrap_with_label("Setup", reason, width));
                    }
                }
            } else {
                details.push("Next       elastos site stage <dir>".to_string());
            }
            if let Some(release) = snapshot.site.active_release.as_deref() {
                let live = snapshot
                    .site
                    .active_channel
                    .as_deref()
                    .map(|channel| format!("{} on {}", release, channel))
                    .unwrap_or_else(|| release.to_string());
                details.push(format!("Live       {}", live));
            } else if snapshot.site.release_count > 0 {
                details.push(format!("Releases   {}", snapshot.site.release_count));
            }
            if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
                details.push(format!("Bundle     elastos://{}", cid));
            }
            details.push("Public     mywebsite publish gives a temporary HTTPS URL".to_string());
            details.extend(wrap_with_label(
                "Commands",
                "mywebsite stage <dir> - mywebsite preview - mywebsite publish - mywebsite open - elastos site publish --release <name> - elastos site activate --channel live - elastos site rollback --target publisher",
                width,
            ));
        }
        "Public" => {
            details.push(format!(
                "Channels   {} total - {} active",
                snapshot.shares.channel_count, snapshot.shares.active_count
            ));
            if let Some(author_did) = snapshot.shares.author_did.as_deref() {
                details.push(format!(
                    "Signer     {}",
                    truncate(author_did, width.saturating_sub(13).max(16))
                ));
            }
            if let Some(channel) = snapshot.shares.channels.first() {
                details.push(format!(
                    "Latest     {} v{} {}",
                    channel.name, channel.latest_version, channel.status
                ));
                details.push(format!(
                    "Open       elastos://{}",
                    truncate(&channel.latest_cid, width.saturating_sub(16).max(16))
                ));
                if let Some(head_cid) = channel.head_cid.as_deref() {
                    details.push(format!(
                        "Head       elastos://{}",
                        truncate(head_cid, width.saturating_sub(16).max(16))
                    ));
                }
            } else {
                details.push("Latest     none yet".to_string());
            }
            details.extend(wrap_with_label(
                "Commands",
                "elastos share <path> - elastos shares list - elastos attest <cid> - elastos open elastos://<cid>",
                width,
            ));
        }
        "Local" => {
            details.extend(wrap_with_label(
                "Commands",
                "Use Local for temporary working state, session roots, and transient data.",
                width,
            ));
        }
        "WebSpaces" => {
            details.extend(wrap_with_label(
                "Commands",
                "elastos webspace ... resolves named monikers into dynamic typed handles.",
                width,
            ));
        }
        _ => {}
    }
    details
}

fn app_entries(snapshot: &HomeSnapshot) -> Vec<AppEntry> {
    home_visible_targets(snapshot)
        .into_iter()
        .map(|target| {
            let action_id = app_target_action_id(snapshot, &target);
            let action = action_id
                .as_deref()
                .and_then(|id| action_by_id(snapshot, id));
            let active = app_target_active(snapshot, &target);
            let state = app_target_state(&target, action, active);
            AppEntry {
                name: target.target.clone(),
                action_id,
                label: canonical_target_title(&target.target, &target.title),
                category: app_target_category(&target),
                description: if target.description.trim().is_empty() {
                    format!("Home target {}", target.target)
                } else {
                    target.description.clone()
                },
                command: app_target_command(&target, action),
                state,
                viewer: target.viewer.clone(),
                viewer_title: target.viewer_title.clone(),
                is_control: false,
            }
        })
        .collect()
}

fn home_visible_targets(snapshot: &HomeSnapshot) -> Vec<HomeTargetStatus> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for target in &snapshot.targets {
        if target.role == "shell" || target.target.trim().is_empty() {
            continue;
        }
        if !seen.insert(target.target.clone()) {
            continue;
        }
        targets.push(target.clone());
    }
    targets.sort_by_key(|target| usize::from(target.target_kind == "object"));
    targets
}

fn canonical_target_title(target: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        target.to_string()
    } else {
        title.to_string()
    }
}

fn app_target_action_id(snapshot: &HomeSnapshot, target: &HomeTargetStatus) -> Option<String> {
    let target_id = target.target.as_str();
    if target_id == "chat" && action_by_id(snapshot, "chat").is_some() {
        return Some("chat".to_string());
    }
    let explicit_open = format!("open-gui:{target_id}");
    action_by_id(snapshot, &explicit_open).map(|_| explicit_open)
}

fn app_target_active(snapshot: &HomeSnapshot, target: &HomeTargetStatus) -> bool {
    snapshot.runtime.running_capsules.iter().any(|item| {
        item == &target.target
            || item.starts_with(&format!("{} ", target.target))
            || item.starts_with(&format!("{}(", target.target))
    })
}

fn app_target_state(
    target: &HomeTargetStatus,
    action: Option<&ActionInfo>,
    active: bool,
) -> String {
    if active {
        return "active".to_string();
    }
    if let Some(action) = action {
        return if action.ready { "ready" } else { "setup" }.to_string();
    }
    if target.target_kind == "object" {
        "read-only".to_string()
    } else {
        "gui-only".to_string()
    }
}

fn app_target_category(target: &HomeTargetStatus) -> &'static str {
    if target.target_kind == "object" {
        "Library"
    } else {
        "Apps"
    }
}

fn app_target_command(target: &HomeTargetStatus, action: Option<&ActionInfo>) -> String {
    if let Some(action) = action {
        return action.command.clone();
    }
    match target.target.as_str() {
        PEOPLE_TARGET_ID => "Use the People tab in Home CLI.".to_string(),
        "system" => "Use the System tab in Home CLI.".to_string(),
        _ => "Open from Desktop, or use an explicit approved Home action.".to_string(),
    }
}

#[cfg(test)]
fn chat_room_app_entry(snapshot: &HomeSnapshot) -> Option<AppEntry> {
    if snapshot.room.room_slug.is_empty()
        && snapshot.room.pending_count == 0
        && snapshot.room.active_session_count == 0
        && snapshot.room.member_count == 0
        && snapshot.room.local_runtime_role.is_none()
    {
        return None;
    }

    let state = if snapshot.room.pending_count > 0 {
        "attention"
    } else if snapshot.room.active_session_count > 0 {
        "active"
    } else if !snapshot.room.browser_access_allowed {
        "restricted"
    } else if snapshot.room.member_count > 0 || snapshot.room.local_runtime_role.is_some() {
        "ready"
    } else {
        "idle"
    };

    Some(AppEntry {
        name: "chat-room".to_string(),
        action_id: Some("chat-room".to_string()),
        label: "Shared Conversation".to_string(),
        category: "Communication",
        description:
            "Chat with other ElastOS users and approved web guests, with attachments opening as ElastOS documents."
                .to_string(),
        command: "Conversation access stays local to this Home.".to_string(),
        state: state.to_string(),
        viewer: None,
        viewer_title: None,
        is_control: false,
    })
}

fn room_control_entries(snapshot: &HomeSnapshot) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    for action in &snapshot.actions {
        let is_room_control = action.id.starts_with("room-approve-request:")
            || action.id.starts_with("room-deny-request:")
            || action.id.starts_with("room-revoke-session:")
            || matches!(
                action.id.as_str(),
                "room-policy-toggle-guests"
                    | "room-policy-toggle-members"
                    | "room-policy-toggle-member-hosts"
            );
        if !is_room_control {
            continue;
        }
        entries.push(AppEntry {
            name: "chat-room".to_string(),
            action_id: Some(action.id.clone()),
            label: action.label.clone(),
            category: "Communication",
            description: action.description.clone(),
            command: action.command.clone(),
            state: if action.ready {
                "ready".to_string()
            } else {
                "blocked".to_string()
            },
            viewer: None,
            viewer_title: None,
            is_control: true,
        });
    }
    entries
}

fn chat_room_app_detail_lines(
    snapshot: &HomeSnapshot,
    entry: &AppEntry,
    width: usize,
) -> Vec<String> {
    let mut details = vec![
        format!("Surface    {}", entry.name),
        format!("State      {}", entry.state),
        format!("Category   {}", entry.category),
    ];
    details.extend(wrap_with_label("What it does", &entry.description, width));

    if !snapshot.room.title.is_empty() {
        details.push(format!("Title      {}", snapshot.room.title));
    }
    if !snapshot.room.room_slug.is_empty() {
        details.push(format!("Channel    {}", snapshot.room.room_slug));
    }
    if let Some(role) = snapshot.room.local_runtime_role.as_deref() {
        details.push(format!("Access     {}", conversation_role_label(role)));
    } else {
        details.push("Access     this device is not connected to this conversation".to_string());
    }
    details.push(format!(
        "People     {} trusted - {} admins - {} active",
        snapshot.room.member_count, snapshot.room.admin_count, snapshot.room.active_member_count
    ));
    details.push(format!("Key epoch  {}", snapshot.room.current_key_epoch));
    details.push(format!(
        "Web guests {}",
        if snapshot.room.allow_guest_invites {
            "public join requests enabled"
        } else {
            "public join requests disabled"
        }
    ));
    details.push(format!(
        "ElastOS    {}",
        if snapshot.room.allow_member_invites {
            "user invites enabled"
        } else {
            "user invites disabled"
        }
    ));
    details.push(format!(
        "Approvals  {}",
        if snapshot.room.allow_members_to_host_guests {
            "trusted users may approve web guests"
        } else {
            "conversation managers approve web guests"
        }
    ));
    if let Some(url) = snapshot.room.canonical_hosted_guest_url.as_deref() {
        details.push(format!(
            "Public URL {}",
            truncate(url, width.saturating_sub(12).max(28))
        ));
    }
    if let Some(url) = snapshot.room.ephemeral_hosted_guest_url.as_deref() {
        details.push(format!(
            "Quick URL  {}",
            truncate(url, width.saturating_sub(12).max(28))
        ));
    }
    if snapshot.room.pending_invite_count > 0 {
        details.push(format!(
            "Invites    {} pending",
            snapshot.room.pending_invite_count
        ));
    } else {
        details.push("Invites    no ElastOS user invites pending".to_string());
    }
    if snapshot.room.member_count == 0 {
        details.push("People     no trusted ElastOS users yet".to_string());
    } else {
        details.push(format!(
            "People     {} trusted participant(s)",
            snapshot.room.member_count
        ));
    }

    if !snapshot.room.browser_access_allowed {
        if let Some(reason) = snapshot.room.browser_access_block_reason.as_deref() {
            details.extend(wrap_with_label("Web link", reason, width));
        } else {
            details.push("Web link   access blocked on this device".to_string());
        }
    } else {
        details.push("Web link   access allowed from this device".to_string());
    }

    if snapshot.room.pending_requests.is_empty() {
        details.push("Pending    no web guest join requests".to_string());
    } else {
        details.push(format!(
            "Pending    {} web guest request(s)",
            snapshot.room.pending_requests.len()
        ));
        for request in snapshot.room.pending_requests.iter().take(3) {
            details.push(format!(
                "Request    {} on {}",
                request.display_name, request.device_label
            ));
        }
    }

    if snapshot.room.active_sessions.is_empty() {
        details.push("Web guests no active web guest sessions".to_string());
    } else {
        details.push(format!(
            "Web guests {} active session(s)",
            snapshot.room.active_sessions.len()
        ));
        for session in snapshot.room.active_sessions.iter().take(3) {
            details.push(format!(
                "Web guest  {} on {}",
                session.display_name, session.device_label
            ));
        }
    }

    let available_controls = room_control_entries(snapshot);
    if available_controls.is_empty() {
        details.push("Control    No conversation actions are waiting right now.".to_string());
    } else {
        details.push(format!(
            "Control    {} targeted conversation action(s) are available below this entry in Apps.",
            available_controls.len()
        ));
        for control in available_controls.iter().take(3) {
            details.push(format!("Next       {}", control.label));
        }
    }
    details
}

fn render_app_list(entries: &[AppEntry], selected: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut last_category = "";
    for (idx, entry) in entries.iter().enumerate() {
        if entry.category != last_category {
            lines.push(format!("{}:", entry.category));
            last_category = entry.category;
        }
        lines.push(format!(
            "{} {} [{}]",
            selected_marker(idx == selected),
            if entry.is_control {
                format!("  {}", entry.label)
            } else {
                app_entry_display_label(entry)
            },
            entry.state
        ));
    }
    lines
}

fn app_entry_display_label(entry: &AppEntry) -> String {
    match app_entry_viewer_label(entry) {
        Some(viewer) if !viewer.trim().is_empty() => format!("{} -> {}", entry.label, viewer),
        _ => entry.label.clone(),
    }
}

fn app_entry_viewer_label(entry: &AppEntry) -> Option<&str> {
    entry
        .viewer_title
        .as_deref()
        .or(entry.viewer.as_deref())
        .filter(|viewer| !viewer.trim().is_empty())
}

#[cfg(test)]
mod tests;
