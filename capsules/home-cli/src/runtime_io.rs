fn command_contract() -> CommandContract {
    let contract: CommandContract =
        serde_json::from_str(COMMAND_CONTRACT_JSON).expect("valid Home CLI command contract");
    assert_eq!(
        contract.schema, "elastos.home-cli.command-contract/v1",
        "Home CLI command definitions are incompatible"
    );
    contract
}

fn normalize_contract_command(input: &str) -> String {
    let query = input.trim().to_lowercase();
    if query.is_empty() {
        return String::new();
    }
    for command in command_contract().commands {
        if command.name == query || command.aliases.iter().any(|alias| alias == &query) {
            return command.name;
        }
    }
    query
}

fn normalize_lookup(input: &str) -> String {
    input.trim().to_lowercase()
}

#[cfg(test)]
fn contract_commands_for(surface: &str) -> Vec<CommandSpec> {
    command_contract()
        .commands
        .into_iter()
        .filter(|command| {
            command.surface.is_empty() || command.surface.iter().any(|item| item == surface)
        })
        .collect()
}

fn with_client<F, R>(f: F) -> R
where
    F: FnOnce(&mut RuntimeClient) -> R,
{
    CLIENT.with(|client| f(&mut client.borrow_mut()))
}

fn request_capability(resource: &str, action: &str) -> Result<String> {
    with_client(|client| {
        client
            .request_capability(resource, action)
            .map_err(|e| anyhow!("Request failed: {}", e))
    })
}

fn carrier_invoke(
    token: &str,
    uri: &str,
    operation: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    with_client(
        |client| match client.carrier_invoke(uri, operation, body, token) {
            Ok(value) => Ok(value),
            Err(err) => Err(anyhow!("Action {} {} failed: {}", operation, uri, err)),
        },
    )
}

fn storage_read_utf8(token: &str, path: &str) -> Result<Vec<u8>> {
    let body = serde_json::json!({
        "path": path,
        "encoding": "utf8",
    });
    let result = carrier_invoke(token, path, "read", &body)?;
    storage_read_bytes_from_result(&result)
}

fn storage_result_body(result: &serde_json::Value) -> Result<&serde_json::Value> {
    let response = result.get("response").unwrap_or(result);
    if response.get("type").and_then(|value| value.as_str()) == Some("carrier_result") {
        return response
            .get("result")
            .ok_or_else(|| anyhow!("carrier_result response missing result"));
    }
    Ok(response)
}

fn storage_read_bytes_from_result(result: &serde_json::Value) -> Result<Vec<u8>> {
    let body = storage_result_body(result)?;
    if body.get("status").and_then(|value| value.as_str()) == Some("error") {
        let code = body
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("read_failed");
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("localhost/read failed");
        return Err(anyhow!("localhost/read failed: {}: {}", code, message));
    }
    if body.get("type").and_then(|value| value.as_str()) == Some("error") {
        let code = body
            .get("code")
            .and_then(|value| value.as_str())
            .unwrap_or("read_failed");
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("localhost/read failed");
        return Err(anyhow!("localhost/read failed: {}: {}", code, message));
    }
    let data = body
        .get("data")
        .map(|value| {
            value
                .get("content")
                .or_else(|| value.get("data"))
                .unwrap_or(value)
        })
        .or_else(|| body.get("content"))
        .ok_or_else(|| anyhow!("localhost/read response missing data"))?;

    if let Some(bytes) = data.as_array() {
        return Ok(bytes
            .iter()
            .filter_map(|value| value.as_u64().map(|byte| byte as u8))
            .collect());
    }

    if let Some(text) = data.as_str() {
        return Ok(text.as_bytes().to_vec());
    }

    Err(anyhow!("localhost/read returned unsupported data shape"))
}

fn storage_write(token: &str, path: &str, content: Vec<u8>) -> Result<()> {
    carrier_invoke(
        token,
        path,
        "write",
        &serde_json::json!({
            "path": path,
            "content": content,
            "append": false,
        }),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let session_root = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("Home CLI capsule missing session root argument"))?;
    let session_scope = format!("{}/*", session_root.trim_end_matches('/'));
    let read_token = request_capability(&session_scope, "read")?;
    let write_token = request_capability(&session_scope, "write")?;
    let snapshot_path = format!("{}/snapshot.json", session_root.trim_end_matches('/'));
    let intent_path = format!("{}/intent.json", session_root.trim_end_matches('/'));
    let snapshot = load_snapshot(&read_token, &snapshot_path)?;

    dashboard_loop(
        &read_token,
        &snapshot_path,
        snapshot,
        &write_token,
        &intent_path,
    )
}

fn load_snapshot(read_token: &str, snapshot_path: &str) -> Result<HomeSnapshot> {
    Ok(serde_json::from_slice(&storage_read_utf8(
        read_token,
        snapshot_path,
    )?)?)
}

fn snapshot_render_fingerprint(snapshot: &HomeSnapshot) -> String {
    format!("{snapshot:?}")
}

fn apply_tui_snapshot_refresh(
    snapshot: &mut HomeSnapshot,
    fingerprint: &mut String,
    next_snapshot: HomeSnapshot,
) -> bool {
    let next_fingerprint = snapshot_render_fingerprint(&next_snapshot);
    if *fingerprint == next_fingerprint {
        return false;
    }
    *snapshot = next_snapshot;
    *fingerprint = next_fingerprint;
    true
}

fn dashboard_loop(
    read_token: &str,
    snapshot_path: &str,
    snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    if should_use_tui() {
        dashboard_tui_loop(
            read_token,
            snapshot_path,
            snapshot,
            write_token,
            intent_path,
        )
    } else {
        dashboard_line_loop(
            read_token,
            snapshot_path,
            snapshot,
            write_token,
            intent_path,
        )
    }
}

fn should_use_tui() -> bool {
    if let Ok(mode) = std::env::var("ELASTOS_HOME_TUI") {
        return matches!(mode.as_str(), "1" | "true" | "yes");
    }

    if std::env::var("ELASTOS_TERM_COLS").is_ok() && std::env::var("ELASTOS_TERM_ROWS").is_ok() {
        return true;
    }

    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn dashboard_tui_loop(
    read_token: &str,
    snapshot_path: &str,
    mut snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    let mut state = TuiState::default();
    let _guard = TerminalGuard::enter()?;
    let mut home_launch_armed = false;
    let mut home_launch_ready_at: Option<Instant> = None;
    let mut snapshot_fingerprint = snapshot_render_fingerprint(&snapshot);
    let mut needs_render = true;

    loop {
        if needs_render {
            render_tui(&snapshot, &state)?;
            needs_render = false;
        }

        let key = read_ui_key()?;
        if key == UiKey::None {
            if let Ok(next_snapshot) = load_snapshot(read_token, snapshot_path) {
                if apply_tui_snapshot_refresh(
                    &mut snapshot,
                    &mut snapshot_fingerprint,
                    next_snapshot,
                ) {
                    needs_render = true;
                }
            }
            continue;
        }
        match startup_home_enter_decision(
            &state,
            key,
            home_launch_armed,
            home_launch_ready_at,
            Instant::now(),
        ) {
            HomeLaunchDecision::Defer(ready_at) => {
                state.notice = Some(
                    "Press Enter again to launch Chat, or use arrows / Tab to pick something else."
                        .to_string(),
                );
                home_launch_armed = true;
                home_launch_ready_at = Some(ready_at);
                needs_render = true;
                continue;
            }
            HomeLaunchDecision::IgnoreDuplicate => {
                continue;
            }
            HomeLaunchDecision::Allow | HomeLaunchDecision::NotApplicable => {}
        }
        if !matches!(key, UiKey::None | UiKey::Enter) {
            home_launch_armed = true;
            home_launch_ready_at = None;
            if state.notice.take().is_some() {
                needs_render = true;
            }
        }

        match key {
            UiKey::Quit => {
                write_intent(write_token, intent_path, quit_action(&snapshot))?;
                return Ok(());
            }
            UiKey::Refresh => {
                write_intent(write_token, intent_path, "refresh")?;
                return Ok(());
            }
            UiKey::Help => {
                state.show_help = !state.show_help;
                state.notice = None;
                needs_render = true;
            }
            UiKey::Left => {
                state.prev_tab();
                state.notice = None;
                needs_render = true;
            }
            UiKey::Right => {
                state.next_tab();
                state.notice = None;
                needs_render = true;
            }
            UiKey::Up => {
                state.move_prev(&snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Down => {
                state.move_next(&snapshot);
                state.notice = None;
                needs_render = true;
            }
            UiKey::Enter => {
                state.notice = None;
                home_launch_ready_at = None;
                if let Some(action_id) = state.activate(&snapshot) {
                    write_intent(write_token, intent_path, &action_id)?;
                    return Ok(());
                }
            }
            UiKey::MarkRead => {
                if state.tab == Tab::Inbox {
                    if let Some(action_id) =
                        selected_notification_read_action(&snapshot, state.inbox_index)
                    {
                        write_intent(write_token, intent_path, &action_id)?;
                        return Ok(());
                    }
                }
            }
            UiKey::Dismiss => {
                if state.tab == Tab::Inbox {
                    if let Some(action_id) =
                        selected_notification_dismiss_action(&snapshot, state.inbox_index)
                    {
                        write_intent(write_token, intent_path, &action_id)?;
                        return Ok(());
                    }
                }
            }
            UiKey::Digit(index) => {
                let quick_actions = quick_launch_action_indices(&snapshot);
                if let Some(action_idx) = quick_actions.get(index.saturating_sub(1)).copied() {
                    state.tab = Tab::Home;
                    state.home_index = index.saturating_sub(1).min(quick_actions.len() - 1);
                    state.notice = None;
                    let action = &snapshot.actions[action_idx];
                    write_intent(write_token, intent_path, &action.id)?;
                    return Ok(());
                }
            }
            UiKey::Mouse(event) => {
                if state.handle_mouse(event, term_cols(), &snapshot) {
                    state.notice = None;
                    needs_render = true;
                }
            }
            UiKey::None => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeLaunchDecision {
    NotApplicable,
    Defer(Instant),
    IgnoreDuplicate,
    Allow,
}

fn startup_home_enter_decision(
    state: &TuiState,
    key: UiKey,
    home_launch_armed: bool,
    home_launch_ready_at: Option<Instant>,
    now: Instant,
) -> HomeLaunchDecision {
    if !matches!(key, UiKey::Enter)
        || state.tab != Tab::Home
        || state.home_index != 0
        || state.show_help
    {
        return HomeLaunchDecision::NotApplicable;
    }

    if !home_launch_armed {
        return HomeLaunchDecision::Defer(now + STARTUP_ENTER_SETTLE_WINDOW);
    }

    if home_launch_ready_at.is_some_and(|ready_at| now < ready_at) {
        return HomeLaunchDecision::IgnoreDuplicate;
    }

    HomeLaunchDecision::Allow
}

fn dashboard_line_loop(
    read_token: &str,
    snapshot_path: &str,
    mut snapshot: HomeSnapshot,
    write_token: &str,
    intent_path: &str,
) -> Result<()> {
    loop {
        render_line_dashboard(&snapshot)?;
        print!("Select action (number, r refresh, q exit, ? help): ");
        io::stdout().flush()?;

        if !stdin_has_input(LIVE_REFRESH_POLL_MS)? {
            if let Ok(next_snapshot) = load_snapshot(read_token, snapshot_path) {
                snapshot = next_snapshot;
            }
            continue;
        }

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            write_intent(write_token, intent_path, quit_action(&snapshot))?;
            return Ok(());
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        match trimmed {
            "q" | "quit" | "/quit" | "/q" => {
                write_intent(write_token, intent_path, quit_action(&snapshot))?;
                return Ok(());
            }
            "r" | "refresh" | "/refresh" => {
                write_intent(write_token, intent_path, "refresh")?;
                return Ok(());
            }
            "?" | "help" | "/help" => {
                print_line_help()?;
                continue;
            }
            _ => {}
        }

        if let Some(result) = cli_invoke_intent(trimmed, &snapshot) {
            match result {
                Ok(invoke) => {
                    write_invoke_intent(write_token, intent_path, invoke)?;
                    return Ok(());
                }
                Err(error) => {
                    println!();
                    println!("invoke: {}", error);
                    wait_for_enter()?;
                    continue;
                }
            }
        }

        match people_line_action(trimmed, &snapshot) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("people: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        match mywebsite_line_action(trimmed) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("mywebsite: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        match system_line_action(trimmed, &snapshot) {
            Ok(Some(action_id)) => {
                write_intent(write_token, intent_path, &action_id)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                println!();
                println!("system: {}", error);
                wait_for_enter()?;
                continue;
            }
        }

        if handle_shared_line_command(trimmed, &snapshot)? {
            wait_for_enter()?;
            continue;
        }

        let Ok(index) = trimmed.parse::<usize>() else {
            println!("Unknown command: {}. Type ? for help.", trimmed);
            wait_for_enter()?;
            continue;
        };

        let quick_actions = quick_launch_action_indices(&snapshot);
        let Some(action_idx) = quick_actions.get(index.saturating_sub(1)).copied() else {
            println!("No action {}. Pick 1-{}.", index, quick_actions.len());
            wait_for_enter()?;
            continue;
        };
        let action = &snapshot.actions[action_idx];

        if !action.ready {
            println!(
                "{} is not ready: {}",
                action.label,
                action.reason.as_deref().unwrap_or("missing prerequisites")
            );
            wait_for_enter()?;
            continue;
        }

        write_intent(write_token, intent_path, &action.id)?;
        return Ok(());
    }
}

fn quit_action(snapshot: &HomeSnapshot) -> &'static str {
    if snapshot.session.mode == "browser_pty" {
        "shell-switch:home-gui"
    } else {
        "quit"
    }
}

fn write_intent(write_token: &str, intent_path: &str, action: &str) -> Result<()> {
    storage_write(write_token, intent_path, home_intent_payload(action, None)?)
}

fn write_invoke_intent(
    write_token: &str,
    intent_path: &str,
    invoke: HomeInvokeIntent,
) -> Result<()> {
    storage_write(
        write_token,
        intent_path,
        home_intent_payload("invoke", Some(invoke))?,
    )
}

fn home_intent_payload(action: &str, invoke: Option<HomeInvokeIntent>) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&HomeIntent { action, invoke })?)
}
