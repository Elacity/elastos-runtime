fn render_line_dashboard(snapshot: &HomeSnapshot) -> Result<()> {
    print_cli_page_header(snapshot, "Home");
    println!("A compact Home shell for working CLI journeys.");
    println!(
        "Version: runtime {}  home {}",
        snapshot.version, DASHBOARD_VERSION
    );

    println!();
    println!("Now");
    println!("  User:      {}", snapshot.user);
    println!("  Nick:      {}", display_name(snapshot));
    println!("  Identity:  {}", identity_summary(snapshot));
    println!("  Network:   {}", network_summary(snapshot));
    println!("  Shell:     {}", active_shell_label(snapshot));
    println!(
        "  Apps:      {} installed / {} running",
        snapshot.cached_capsules.len(),
        snapshot.runtime.running_capsules.len()
    );

    println!();
    let quick_actions = quick_launch_action_indices(snapshot);
    if quick_actions.is_empty() {
        println!("Start Here");
        println!("  No CLI actions are ready in this snapshot.");
    } else {
        println!("Start Here");
        for (slot, action_idx) in quick_actions.iter().enumerate() {
            let action = &snapshot.actions[*action_idx];
            println!(
                "  {}. {} [{}]",
                slot + 1,
                action_display_label(action),
                if action.ready { "ready" } else { "blocked" }
            );
            println!("     {}", home_action_summary(action));
            if !action.command.trim().is_empty() {
                println!("     {}", action.command);
            }
            if let Some(reason) = &action.reason {
                println!("     setup: {}", reason);
            }
        }
    }

    let alerts = alerts_lines(snapshot, 80, snapshot.notice.as_deref());
    if !alerts.is_empty() {
        println!();
        println!("Needs Attention");
        for line in alerts {
            println!("  {}", line);
        }
    }

    println!();
    println!("Inbox");
    println!(
        "  Attention: {} waiting / {} unread",
        snapshot.notifications.attention_count, snapshot.notifications.unread_count
    );
    for entry in snapshot.notifications.entries.iter().take(3) {
        println!("  - {}", entry.body);
    }

    println!();
    println!("Apps");
    for line in apps_summary_lines(snapshot) {
        println!("  {}", line);
    }

    println!();
    for line in dashboard_command_hint_lines() {
        println!("{line}");
    }

    println!();
    println!("Choose an action number, `r` to refresh, `q` to return to Desktop, `?` for help.");
    io::stdout().flush()?;
    Ok(())
}

fn cli_page_header(snapshot: &HomeSnapshot, title: &str) -> String {
    format!(
        "\x1B[2J\x1B[HHome CLI / {title}\nuser {}  |  identity {}  |  network {}  |  shell {}\n\n",
        display_name(snapshot),
        identity_summary(snapshot),
        network_summary(snapshot),
        active_shell_label(snapshot)
    )
}

fn print_cli_page_header(snapshot: &HomeSnapshot, title: &str) {
    print!("{}", cli_page_header(snapshot, title));
}

fn print_line_help() -> Result<()> {
    print_cli_help_topic("");
    wait_for_enter()
}

fn print_cli_help_topic(topic: &str) {
    println!();
    for line in help_lines(topic) {
        println!("{line}");
    }
}

fn dashboard_command_hint_lines() -> Vec<String> {
    let mut lines = vec!["Commands".to_string()];
    append_tab_help_lines(&mut lines, HELP_TAB_COMMANDS);
    lines.push("  help advanced           Power-user command reference".to_string());
    lines.push("  help debug              Developer diagnostics reference".to_string());
    lines
}

fn help_lines(topic: &str) -> Vec<String> {
    if let Some(lines) = help_category_lines(topic) {
        return lines;
    }

    let normalized = normalize_contract_command(topic);
    if !normalized.is_empty() {
        if let Some(command) = command_by_name(&normalized) {
            return command_help_lines(&command);
        }
    }

    first_run_help_lines()
}

fn first_run_help_lines() -> Vec<String> {
    let mut lines = vec!["Home CLI Help".to_string()];
    append_tab_help_section(&mut lines, "Tabs", HELP_TAB_COMMANDS);
    append_help_section(&mut lines, "Controls", HELP_CONTROL_COMMANDS);
    lines.push("Advanced".to_string());
    lines.push("  help advanced           Power-user command reference".to_string());
    lines.push("Debug".to_string());
    lines.push("  help debug              Developer diagnostics reference".to_string());
    lines.push("  <number>                Launch a visible Home action".to_string());
    lines
}

fn help_category_lines(topic: &str) -> Option<Vec<String>> {
    match normalize_lookup(topic).as_str() {
        "tabs" | "core" | "basics" => Some(help_section_lines("Tabs", HELP_TAB_COMMANDS)),
        "control" | "controls" => Some(help_section_lines("Controls", HELP_CONTROL_COMMANDS)),
        "task" | "tasks" => Some(help_section_lines("Advanced", HELP_ADVANCED_COMMANDS)),
        "advanced" | "adv" => Some(help_section_lines("Advanced", HELP_ADVANCED_COMMANDS)),
        "debug" | "dev" => Some(help_section_lines("Debug", HELP_DEBUG_COMMANDS)),
        _ => None,
    }
}

fn help_section_lines(title: &str, command_ids: &[&str]) -> Vec<String> {
    let mut lines = vec![format!("{title} Commands")];
    append_command_help_lines(&mut lines, command_ids);
    lines
}

fn append_help_section(lines: &mut Vec<String>, title: &str, command_ids: &[&str]) {
    lines.push(title.to_string());
    append_command_help_lines(lines, command_ids);
}

fn append_tab_help_section(lines: &mut Vec<String>, title: &str, command_ids: &[&str]) {
    lines.push(title.to_string());
    append_tab_help_lines(lines, command_ids);
}

fn append_tab_help_lines(lines: &mut Vec<String>, command_ids: &[&str]) {
    for command_id in command_ids {
        if let Some(command) = command_by_name(command_id) {
            lines.push(format!("  {:<24} {}", command.name, command.summary));
        }
    }
}

fn append_command_help_lines(lines: &mut Vec<String>, command_ids: &[&str]) {
    for command_id in command_ids {
        if let Some(command) = command_by_name(command_id) {
            lines.push(format!("  {:<24} {}", command.usage, command.summary));
        }
    }
}

fn command_help_lines(command: &CommandSpec) -> Vec<String> {
    let mut lines = vec![command.usage.clone(), format!("  {}", command.description)];
    if !command.aliases.is_empty() {
        lines.push(format!("  aliases: {}", command.aliases.join(", ")));
    }
    lines
}

fn command_by_name(name: &str) -> Option<CommandSpec> {
    command_contract()
        .commands
        .into_iter()
        .find(|command| command.name == name)
}

fn cli_invoke_intent(input: &str, snapshot: &HomeSnapshot) -> Option<Result<HomeInvokeIntent>> {
    let mut parts = input.split_whitespace();
    let raw_name = parts.next()?;
    if normalize_contract_command(raw_name) != "invoke" {
        return None;
    }
    let arg = parts.collect::<Vec<_>>().join(" ");
    if arg.trim().is_empty() || arg.trim() == "list" || arg.trim().starts_with("list ") {
        return None;
    }
    Some(resolve_cli_invoke_intent(&arg, snapshot))
}

fn resolve_cli_invoke_intent(arg: &str, snapshot: &HomeSnapshot) -> Result<HomeInvokeIntent> {
    let raw = arg.trim();
    let Some((capsule_input, rest)) = raw.split_once(char::is_whitespace) else {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    };
    let rest = rest.trim();
    if rest.is_empty() {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    }
    let (method_input, input) = match rest.split_once(char::is_whitespace) {
        Some((method, input)) => (method, input.trim()),
        None => (rest, ""),
    };
    let capsule = find_capsule_fact(snapshot, capsule_input)
        .ok_or_else(|| anyhow!("capsule not found: {capsule_input}"))?;
    let capsule_name = json_text(capsule, "name");
    let (interface_id, entry, method) = resolve_cli_method(snapshot, capsule_name, method_input)?;
    if let Some(reason) = cli_method_block_reason(entry, method) {
        anyhow::bail!("blocked: {reason}");
    }
    let resource = json_text(method, "resource");
    if resource.is_empty() {
        anyhow::bail!("blocked: executable method is missing its Runtime resource binding");
    }
    Ok(HomeInvokeIntent {
        capsule: capsule_name.to_string(),
        interface_id: interface_id.to_string(),
        method: json_text(method, "id").to_string(),
        resource: resource.to_string(),
        input: parse_cli_invoke_input(input, method)?,
    })
}

fn resolve_cli_method<'a>(
    snapshot: &'a HomeSnapshot,
    capsule_name: &str,
    method_input: &str,
) -> Result<(&'a str, &'a serde_json::Value, &'a serde_json::Value)> {
    let method_query = normalize_lookup(method_input);
    if method_query.is_empty() {
        anyhow::bail!("usage: invoke <capsule> <method> [json|target]");
    }
    let mut matches = Vec::new();
    for entry in cli_interface_entries_for(snapshot, Some(capsule_name)) {
        let descriptor = interface_descriptor(entry);
        let interface_id = json_text(descriptor, "id");
        if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
            for method in methods {
                if normalize_lookup(json_text(method, "id")) == method_query {
                    matches.push((interface_id, entry, method));
                }
            }
        }
    }
    match matches.len() {
        1 => Ok(matches[0]),
        0 => anyhow::bail!("method not found: {method_input}"),
        _ => anyhow::bail!("ambiguous method: {method_input}"),
    }
}

fn cli_method_block_reason(
    entry: &serde_json::Value,
    method: &serde_json::Value,
) -> Option<String> {
    let method_id = json_text(method, "id");
    let Some(binding) = interface_method_binding(entry, method_id) else {
        return Some("Runtime did not report an invocation binding".to_string());
    };
    if binding.get("executable").and_then(|value| value.as_bool()) != Some(true) {
        return Some(
            binding
                .get("reason")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Runtime did not mark this method executable")
                .to_string(),
        );
    }
    let approval = json_text(method, "approval");
    let approval = if approval.is_empty() {
        json_text(method, "approval_mode")
    } else {
        approval
    };
    let risk = json_text(method, "risk");
    let risk = if risk.is_empty() {
        json_text(method, "risk_level")
    } else {
        risk
    };
    if approval == "user" {
        return Some("user approval is required before invocation".to_string());
    }
    if ["payment", "rights", "actuator", "privileged"].contains(&risk) {
        return Some(format!("{risk} risk requires explicit user approval"));
    }
    None
}

fn interface_method_binding<'a>(
    entry: &'a serde_json::Value,
    method_id: &str,
) -> Option<&'a serde_json::Value> {
    entry
        .get("bindings")
        .and_then(|bindings| bindings.as_array())?
        .iter()
        .find(|binding| json_text(binding, "method") == method_id)
}

fn parse_cli_invoke_input(input: &str, method: &serde_json::Value) -> Result<serde_json::Value> {
    let raw = input.trim();
    if raw.is_empty() {
        return Ok(serde_json::json!({}));
    }
    if raw.starts_with('{') || raw.starts_with('[') {
        return serde_json::from_str(raw).map_err(Into::into);
    }
    if json_text(method, "resource") == "elastos://capsules/*"
        && json_text(method, "operation") == "launch"
    {
        return Ok(serde_json::json!({ "target": raw }));
    }
    anyhow::bail!("enter this action's input as JSON")
}

fn handle_shared_line_command(input: &str, snapshot: &HomeSnapshot) -> Result<bool> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(false);
    };
    let arg = parts.collect::<Vec<_>>().join(" ");
    let name = normalize_contract_command(raw_name);
    match name.as_str() {
        "home" => {
            render_line_dashboard(snapshot)?;
        }
        "apps" => {
            print_cli_section(snapshot, "Apps", &apps_summary_lines(snapshot));
        }
        "inbox" => {
            print_cli_inbox(snapshot);
        }
        "people" => {
            print_cli_people(snapshot);
        }
        "mywebsite" => {
            print_cli_mywebsite(snapshot);
        }
        "wallet" => {
            print_cli_wallet(snapshot);
        }
        "exits" => {
            print_cli_services(snapshot, "remote_exit");
        }
        "system" => {
            print_cli_system(snapshot, &arg);
        }
        "invoke" => {
            print_cli_invokable(snapshot, &arg);
        }
        "debug" => {
            print_cli_debug(snapshot, &arg);
        }
        "help" => {
            print_cli_help_topic(&arg);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn people_line_action(input: &str, snapshot: &HomeSnapshot) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    if normalize_contract_command(raw_name) != "people" {
        return Ok(None);
    }

    let mut args = parts.collect::<Vec<_>>();
    if normalize_lookup(raw_name) == "discovery" {
        args.insert(0, "discovery");
    }
    if args.is_empty() {
        return Ok(None);
    }

    let action_id = match normalize_lookup(args[0]).as_str() {
        "discovery" => match args.get(1).map(|value| normalize_lookup(value)).as_deref() {
            Some("on" | "enable" | "start") => "people-discovery-enable".to_string(),
            Some("off" | "disable" | "stop") => "people-discovery-disable".to_string(),
            Some("refresh" | "reload") => "people-discovery-refresh".to_string(),
            _ => anyhow::bail!("usage: people discovery on|off|refresh"),
        },
        "request" | "add" => {
            let Some(peer_id) = line_arg_tail(&args, 1) else {
                anyhow::bail!("usage: people request <peer-id>");
            };
            format!("people-request-peer:{peer_id}")
        }
        "accept" => {
            let Some(request_id) = line_arg_tail(&args, 1) else {
                anyhow::bail!("usage: people accept <request-id>");
            };
            format!("people-accept-request:{request_id}")
        }
        "remove" | "delete" => {
            let Some(contact_ref) = line_arg_tail(&args, 1) else {
                anyhow::bail!("usage: people remove <contact-id>");
            };
            let contact_id =
                people_contact_id_for_reference(snapshot, &contact_ref).unwrap_or(contact_ref);
            format!("people-remove-contact:{contact_id}")
        }
        "message" | "chat" => {
            let Some(contact_ref) = line_arg_tail(&args, 1) else {
                anyhow::bail!("usage: people message <contact-id>");
            };
            let contact_id =
                people_contact_id_for_reference(snapshot, &contact_ref).unwrap_or(contact_ref);
            format!("people-message:{contact_id}")
        }
        _ => return Ok(None),
    };

    people_actions(snapshot)
        .into_iter()
        .find(|action| action.id == action_id && action.ready)
        .map(|action| action.id)
        .ok_or_else(|| {
            anyhow!("People action is not available in the current Home snapshot: {action_id}")
        })
        .map(Some)
}

fn line_arg_tail(args: &[&str], start: usize) -> Option<String> {
    let value = args.get(start..)?.join(" ");
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn mywebsite_line_action(input: &str) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    if normalize_contract_command(raw_name) != "mywebsite" {
        return Ok(None);
    }

    let Some(raw_verb) = parts.next() else {
        return Ok(None);
    };
    let verb = normalize_lookup(raw_verb);
    match verb.as_str() {
        "status" => Ok(None),
        "stage" => {
            let path = parts.collect::<Vec<_>>().join(" ");
            let path = path.trim();
            if path.is_empty() {
                anyhow::bail!("usage: mywebsite stage <dir>");
            }
            Ok(Some(format!("site-stage:{path}")))
        }
        "preview" | "serve" => Ok(Some("site-local".to_string())),
        "publish" | "public" | "go-public" => Ok(Some("site-ephemeral".to_string())),
        "open" => Ok(Some("site-open".to_string())),
        _ => anyhow::bail!(
            "unknown MyWebSite command: {raw_verb}. Try status, stage <dir>, preview, publish, or open"
        ),
    }
}

fn system_line_action(input: &str, snapshot: &HomeSnapshot) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(raw_name) = parts.next() else {
        return Ok(None);
    };
    let command = normalize_contract_command(raw_name);
    if command == "signout" {
        if parts.next().is_some() {
            anyhow::bail!("usage: signout");
        }
        let action = sign_out_action(snapshot);
        if !action.ready {
            anyhow::bail!("{}", action.reason.unwrap_or_else(|| "sign out unavailable".to_string()));
        }
        return Ok(Some(action.id));
    }
    if command != "system" {
        return Ok(None);
    }

    let Some(raw_topic) = parts.next() else {
        return Ok(None);
    };
    if normalize_system_topic(raw_topic) != "shell" {
        return Ok(None);
    }

    let Some(raw_target) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() {
        anyhow::bail!("usage: system shell home-gui");
    }

    let target = normalize_shell_target(raw_target);
    if target != "home-gui" {
        anyhow::bail!("unsupported shell target `{raw_target}`; use `system shell home-gui`");
    }
    let action = shell_switch_home_gui_action(snapshot);
    if !action.ready {
        anyhow::bail!(
            "{}",
            action
                .reason
                .unwrap_or_else(|| "shell switch unavailable".to_string())
        );
    }
    Ok(Some(action.id))
}

fn normalize_shell_target(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "gui" | "desktop" | "home" | "home-gui" => "home-gui".to_string(),
        "cli" | "terminal" | "home-cli" => "home-cli".to_string(),
        other => other.to_string(),
    }
}

fn print_cli_system(snapshot: &HomeSnapshot, arg: &str) {
    let mut parts = arg.split_whitespace();
    let topic = parts.next().map(normalize_system_topic).unwrap_or_default();
    match topic.as_str() {
        "" => print_cli_system_section(snapshot, "System", &system_settings_lines(snapshot)),
        "shell" => {
            print_cli_system_section(snapshot, "System Shell", &system_shell_lines(snapshot))
        }
        "updates" => print_cli_system_section(snapshot, "Updates", &system_update_lines(snapshot)),
        "identity" => {
            print_cli_system_section(snapshot, "Profile", &system_identity_lines(snapshot))
        }
        "diagnostics" => {
            print_cli_system_section(snapshot, "Diagnostics", &system_diagnostics_lines(snapshot))
        }
        _ => {
            print_cli_system_header(snapshot, "System");
            println!("Unknown System topic: {arg}");
            println!("  Try: system shell, updates, profile, diagnostics");
        }
    }
}

fn normalize_system_topic(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "shells" | "active-shell" | "home-shell" => "shell".to_string(),
        "source" | "sources" | "trusted-source" | "seed" => "updates".to_string(),
        "update" | "updates" | "upgrade" | "release" => "updates".to_string(),
        "id" | "identity" | "profile" => "identity".to_string(),
        "diag" | "diagnostic" | "diagnostics" | "health" => "diagnostics".to_string(),
        other => other.to_string(),
    }
}

fn print_cli_debug(snapshot: &HomeSnapshot, arg: &str) {
    let mut parts = arg.split_whitespace();
    let Some(raw_name) = parts.next() else {
        print_cli_debug_help(snapshot);
        return;
    };
    let rest = parts.collect::<Vec<_>>().join(" ");
    match normalize_debug_command(raw_name).as_str() {
        "capsules" => print_cli_capsules(snapshot),
        "inspect" => print_cli_inspect(snapshot, &rest),
        "affordances" => print_cli_affordances(snapshot, &rest),
        "gates" => print_cli_gates(snapshot, &rest),
        "audit" => print_cli_audit(snapshot, &rest),
        "people" => {
            print_cli_section(snapshot, "Debug People", &people_debug_lines(snapshot));
            print_cli_contacts(snapshot);
        }
        "spaces" => print_cli_spaces(snapshot, raw_name, &rest),
        "services" => print_cli_services(snapshot, ""),
        "browser" => print_cli_browser(snapshot),
        "contract" => print_cli_contract(snapshot),
        "terminal" => print_cli_terminal_contract(snapshot),
        _ => {
            println!("Unknown debug topic: {raw_name}");
            print_cli_debug_help(snapshot);
        }
    }
}

fn print_cli_debug_help(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Debug");
    println!("Developer facts are hidden from the default Home CLI surface.");
    println!("Affordance and gate {DESCRIPTOR_AUTHORITY_COPY}.");
    println!();
    println!("Debug Topics");
    println!("  debug capsules              installed capsule catalog");
    println!("  debug inspect <capsule>     catalog projection for one capsule");
    println!("  debug affordances [capsule] declared capability descriptors");
    println!("  debug gates [capsule]       declared gate descriptors");
    println!("  debug audit <capsule>       provenance and trust facts");
    println!("  debug people                People internals, room, and transport facts");
    println!("  debug spaces [root]         root and WebSpace projection");
    println!("  debug services              local and remote service offers");
    println!("  debug browser               Browser target and exit facts");
    println!("  debug terminal              Runtime PTY terminal contract");
    println!("  debug contract              shared capsule interface model");
}

fn normalize_debug_command(input: &str) -> String {
    match normalize_lookup(input).as_str() {
        "caps" | "catalog" => "capsules".to_string(),
        "affordance" | "interface" | "interfaces" | "ifaces" => "affordances".to_string(),
        "gate" => "gates".to_string(),
        "trust" | "provenance" => "audit".to_string(),
        "roots" | "places" | "mywebsite" | "public" | "local" | "webspaces" => "spaces".to_string(),
        "shortcuts" | "keys" => "terminal".to_string(),
        "model" | "interface-contract" => "contract".to_string(),
        other => other.to_string(),
    }
}

fn catalog_capsules(snapshot: &HomeSnapshot) -> &[serde_json::Value] {
    snapshot
        .capsule_catalog
        .as_ref()
        .and_then(|catalog| catalog.get("capsules"))
        .and_then(|capsules| capsules.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn interface_registry_entries(snapshot: &HomeSnapshot) -> &[serde_json::Value] {
    snapshot
        .capsule_interfaces
        .as_ref()
        .and_then(|registry| registry.get("interfaces"))
        .and_then(|interfaces| interfaces.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn json_text<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value.get(key).and_then(|item| item.as_str()).unwrap_or("")
}

fn json_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|item| item.as_bool())
        .unwrap_or(false)
}

fn json_array_len(value: &serde_json::Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(|item| item.as_array())
        .map(Vec::len)
        .unwrap_or_default()
}

fn accepted_content_titles(capsule: &serde_json::Value) -> Vec<String> {
    capsule
        .get("accepted_content")
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let title = json_text(item, "title");
                    (!title.is_empty()).then(|| title.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn accepted_content_for_viewer(snapshot: &HomeSnapshot, viewer: &str) -> Vec<String> {
    let mut labels = find_capsule_fact(snapshot, viewer)
        .map(accepted_content_titles)
        .unwrap_or_default();
    let mut extensions = BTreeSet::new();
    for entry in cli_interface_entries_for(snapshot, Some(viewer)) {
        let descriptor = interface_descriptor(entry);
        for method in descriptor
            .get("methods")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            for accepted in method
                .get("input_schema")
                .and_then(|schema| schema.get("accepts"))
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                if json_text(accepted, "mode") == "unsupported_family_diagnostic" {
                    continue;
                }
                for extension in accepted
                    .get("extensions")
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                {
                    extensions.insert(extension.to_string());
                }
            }
        }
    }
    if !extensions.is_empty() {
        labels.push(format!(
            "{} files",
            extensions.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    labels
}

fn capsule_requirement_titles(snapshot: &HomeSnapshot, capsule: &serde_json::Value) -> Vec<String> {
    capsule
        .get("requires")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|requirement| requirement.get("name").and_then(|value| value.as_str()))
        .map(|name| {
            find_capsule_fact(snapshot, name)
                .map(|capsule| json_text(capsule, "title"))
                .filter(|title| !title.is_empty())
                .unwrap_or(name)
                .to_string()
        })
        .collect()
}

fn capsule_executable_action_labels(snapshot: &HomeSnapshot, capsule_name: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for entry in cli_interface_entries_for(snapshot, Some(capsule_name)) {
        let descriptor = interface_descriptor(entry);
        let methods = descriptor
            .get("methods")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .map(|method| (json_text(method, "id"), method))
            .collect::<BTreeMap<_, _>>();
        for binding in entry
            .get("bindings")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter(|binding| json_bool(binding, "executable"))
        {
            let method_id = json_text(binding, "method");
            if method_id == "capsule.open" {
                continue;
            }
            if let Some(method) = methods.get(method_id) {
                let description = json_text(method, "description").trim_end_matches('.');
                if !description.is_empty() {
                    labels.push(description.to_string());
                }
            }
        }
    }
    labels.sort();
    labels.dedup();
    labels
}

fn projection_surface_state(capsule: &serde_json::Value, surface: &str) -> String {
    capsule
        .get("projection")
        .and_then(|projection| projection.get(surface))
        .and_then(|surface| surface.get("state"))
        .and_then(|state| state.as_str())
        .unwrap_or("unknown")
        .to_string()
}

fn projection_surface_note(capsule: &serde_json::Value, surface: &str) -> String {
    capsule
        .get("projection")
        .and_then(|projection| projection.get(surface))
        .and_then(|surface| surface.get("note"))
        .and_then(|note| note.as_str())
        .unwrap_or("")
        .to_string()
}

fn capsule_matches(capsule: &serde_json::Value, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    [
        json_text(capsule, "name"),
        json_text(capsule, "title"),
        json_text(capsule, "launch_target"),
    ]
    .iter()
    .any(|value| value.to_lowercase() == needle)
}

fn find_capsule_fact<'a>(snapshot: &'a HomeSnapshot, query: &str) -> Option<&'a serde_json::Value> {
    catalog_capsules(snapshot)
        .iter()
        .find(|capsule| capsule_matches(capsule, query))
}

fn require_capsule_arg<'a>(arg: &'a str, command: &str) -> Option<&'a str> {
    let query = arg.trim();
    if query.is_empty() {
        println!();
        println!("Usage: {command} <capsule>");
        return None;
    }
    Some(query)
}

fn print_cli_capsules(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Capsules");
    println!("Capsules");
    let capsules = catalog_capsules(snapshot);
    if capsules.is_empty() {
        println!("  Capsule catalog facts are not available in this snapshot.");
        return;
    }
    if let Some(counts) = snapshot
        .capsule_catalog
        .as_ref()
        .and_then(|catalog| catalog.get("counts"))
    {
        println!(
            "  total {} - installed {} - launchable {} - interfaces {} - methods {}",
            counts
                .get("total")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("installed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("launchable")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("interfaces")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            counts
                .get("methods")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        );
    }
    for capsule in capsules.iter().take(18) {
        println!(
            "  {:<24} {:<9} cli={} gates={} {}",
            json_text(capsule, "name"),
            json_text(capsule, "role"),
            projection_surface_state(capsule, "cli"),
            projection_surface_state(capsule, "gates"),
            if json_bool(capsule, "launchable") {
                "launchable"
            } else {
                "facts"
            }
        );
    }
    if capsules.len() > 18 {
        println!("  ... {} more", capsules.len() - 18);
    }
}

fn print_cli_inspect(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Inspect");
    let Some(query) = require_capsule_arg(arg, "inspect") else {
        return;
    };
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("inspect: capsule not found: {query}");
        return;
    };
    println!("Capsule {}", json_text(capsule, "name"));
    println!("  title       {}", json_text(capsule, "title"));
    println!(
        "  role/type   {}/{}",
        json_text(capsule, "role"),
        json_text(capsule, "type")
    );
    println!("  state       {}", json_text(capsule, "state"));
    println!("  trust       {}", json_text(capsule, "trust_state"));
    if !json_text(capsule, "route").is_empty() {
        println!("  route       {}", json_text(capsule, "route"));
    }
    if !json_text(capsule, "provides").is_empty() {
        println!("  provides    {}", json_text(capsule, "provides"));
    }
    let requirements = capsule_requirement_titles(snapshot, capsule);
    if !requirements.is_empty() {
        println!("  needs       {}", requirements.join(", "));
    }
    if !json_text(capsule, "viewer").is_empty() {
        let viewer_title = json_text(capsule, "viewer_title");
        let viewer = json_text(capsule, "viewer");
        println!(
            "  opens with  {}",
            if viewer_title.is_empty() {
                viewer
            } else {
                viewer_title
            }
        );
    }
    let accepted_content = accepted_content_for_viewer(snapshot, json_text(capsule, "name"));
    if !accepted_content.is_empty() {
        println!("  accepts     {}", accepted_content.join(", "));
    }
    let available = capsule_executable_action_labels(snapshot, json_text(capsule, "name"));
    if !available.is_empty() {
        println!("  available   {}", available.join(", "));
    }
    for surface in [
        "web",
        "cli",
        "facts",
        "affordances",
        "gates",
        "audit_mirror",
        "carrier",
    ] {
        println!(
            "  {:<12} {}",
            surface,
            projection_surface_state(capsule, surface)
        );
    }
}

fn cli_interface_entries_for<'a>(
    snapshot: &'a HomeSnapshot,
    capsule_name: Option<&str>,
) -> Vec<&'a serde_json::Value> {
    let entries = interface_registry_entries(snapshot);
    if entries.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|entry| {
            capsule_name
                .map(|name| json_text(entry, "capsule") == name)
                .unwrap_or(true)
        })
        .collect()
}

fn interface_descriptor(entry: &serde_json::Value) -> &serde_json::Value {
    entry.get("interface").unwrap_or(entry)
}

fn method_binding_label(entry: &serde_json::Value, method: &serde_json::Value) -> String {
    let Some(binding) = interface_method_binding(entry, json_text(method, "id")) else {
        return "not executable".to_string();
    };
    if binding.get("executable").and_then(|value| value.as_bool()) == Some(true) {
        return "invoke".to_string();
    }
    json_text(binding, "state").replace('-', " ")
}

fn print_cli_invokable(snapshot: &HomeSnapshot, arg: &str) {
    let query = arg.trim().strip_prefix("list").unwrap_or(arg.trim()).trim();
    let capsule_name = if query.is_empty() {
        None
    } else {
        match find_capsule_fact(snapshot, query) {
            Some(capsule) => Some(json_text(capsule, "name")),
            None => {
                println!("invoke: capsule not found: {query}");
                return;
            }
        }
    };
    println!("Invokable methods");
    let methods = cli_invokable_methods(snapshot, capsule_name);
    for (capsule, interface, method) in &methods {
        println!("  invoke {capsule} {method}  ({interface})");
    }
    if methods.is_empty() {
        println!("  No methods are executable through generic Runtime invocation.");
    }
}

fn cli_invokable_methods<'a>(
    snapshot: &'a HomeSnapshot,
    capsule_name: Option<&str>,
) -> Vec<(&'a str, &'a str, &'a str)> {
    let mut methods = Vec::new();
    for entry in cli_interface_entries_for(snapshot, capsule_name) {
        let descriptor = interface_descriptor(entry);
        let interface_id = json_text(descriptor, "id");
        let Some(declared) = descriptor.get("methods").and_then(|value| value.as_array()) else {
            continue;
        };
        for method in declared {
            if interface_method_binding(entry, json_text(method, "id"))
                .and_then(|binding| binding.get("executable"))
                .and_then(|value| value.as_bool())
                == Some(true)
            {
                methods.push((
                    json_text(entry, "capsule"),
                    interface_id,
                    json_text(method, "id"),
                ));
            }
        }
    }
    methods
}

fn print_cli_affordances(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Affordances");
    let capsule = if arg.trim().is_empty() {
        None
    } else {
        match find_capsule_fact(snapshot, arg.trim()) {
            Some(capsule) => Some(capsule),
            None => {
                println!("affordances: capsule not found: {}", arg.trim());
                return;
            }
        }
    };
    let capsule_name = capsule.map(|capsule| json_text(capsule, "name"));
    let entries = cli_interface_entries_for(snapshot, capsule_name);
    println!("Affordances");
    println!("  {DESCRIPTOR_AUTHORITY_COPY}.");
    if entries.is_empty() {
        println!("  No declared affordances in this snapshot.");
        return;
    }
    for entry in entries.iter().take(16) {
        let descriptor = interface_descriptor(entry);
        println!(
            "  {} :: {}",
            json_text(entry, "capsule"),
            json_text(descriptor, "id")
        );
        if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
            for method in methods.iter().take(8) {
                println!(
                    "    - {:<24} {:<20} risk={} approval={}",
                    json_text(method, "id"),
                    method_binding_label(entry, method),
                    json_text(method, "risk"),
                    json_text(method, "approval")
                );
            }
        }
    }
    if entries.len() > 16 {
        println!("  ... {} more interfaces", entries.len() - 16);
    }
}

fn print_cli_gates(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Gates");
    let query = arg.trim();
    if query.is_empty() {
        let entries = cli_interface_entries_for(snapshot, None);
        println!("Gates");
        println!("  {DESCRIPTOR_AUTHORITY_COPY}.");
        if entries.is_empty() {
            println!("  No declared method gates in this snapshot.");
            return;
        }
        for entry in entries.iter().take(16) {
            print_cli_gate_entry(entry, 8);
        }
        if entries.len() > 16 {
            println!("  ... {} more interfaces", entries.len() - 16);
        }
        return;
    }
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("gates: capsule not found: {query}");
        return;
    };
    let capsule_name = json_text(capsule, "name");
    println!("Gates {capsule_name}");
    println!("  {DESCRIPTOR_AUTHORITY_COPY}.");
    println!(
        "  projection {}",
        projection_surface_state(capsule, "gates")
    );
    let note = projection_surface_note(capsule, "gates");
    if !note.is_empty() {
        println!("  note       {note}");
    }
    let entries = cli_interface_entries_for(snapshot, Some(capsule_name));
    if entries.is_empty() {
        println!("  No declared method gates.");
        return;
    }
    for entry in entries {
        print_cli_gate_entry(entry, usize::MAX);
    }
}

fn print_cli_gate_entry(entry: &serde_json::Value, method_limit: usize) {
    let descriptor = interface_descriptor(entry);
    println!(
        "  {} :: {}",
        json_text(entry, "capsule"),
        json_text(descriptor, "id")
    );
    if let Some(methods) = descriptor.get("methods").and_then(|value| value.as_array()) {
        for method in methods.iter().take(method_limit) {
            println!(
                "    - {:<24} {:<20} risk={} approval={}",
                json_text(method, "id"),
                method_binding_label(entry, method),
                json_text(method, "risk"),
                json_text(method, "approval")
            );
        }
    }
}

fn print_cli_audit(snapshot: &HomeSnapshot, arg: &str) {
    print_cli_page_header(snapshot, "Audit");
    let Some(query) = require_capsule_arg(arg, "audit") else {
        return;
    };
    let Some(capsule) = find_capsule_fact(snapshot, query) else {
        println!("audit: capsule not found: {query}");
        return;
    };
    println!("Audit {}", json_text(capsule, "name"));
    println!("  trust      {}", json_text(capsule, "trust_state"));
    println!("  signature  {}", json_text(capsule, "signature_state"));
    println!("  cid        {}", json_text(capsule, "cid_state"));
    println!("  payment    {}", json_text(capsule, "payment_state"));
    println!("  drm        {}", json_text(capsule, "drm_state"));
    println!("  source     {}", json_text(capsule, "source"));
    println!("  interfaces {}", json_array_len(capsule, "interfaces"));
    let note = projection_surface_note(capsule, "audit_mirror");
    if !note.is_empty() {
        println!("  mirror     {note}");
    }
}

fn print_cli_section(snapshot: &HomeSnapshot, title: &str, lines: &[String]) {
    print_cli_page_header(snapshot, title);
    println!("{title}");
    if lines.is_empty() {
        println!("  (none)");
        return;
    }
    for line in lines {
        println!("  {line}");
    }
}

fn cli_system_page_header(snapshot: &HomeSnapshot, title: &str) -> String {
    format!(
        "\x1B[2J\x1B[HHome CLI / {title}\nidentity {}  |  shell {}\n\n",
        identity_summary(snapshot),
        active_shell_label(snapshot)
    )
}

fn print_cli_system_header(snapshot: &HomeSnapshot, title: &str) {
    print!("{}", cli_system_page_header(snapshot, title));
}

fn print_cli_system_section(snapshot: &HomeSnapshot, title: &str, lines: &[String]) {
    print_cli_system_header(snapshot, title);
    println!("{title}");
    if lines.is_empty() {
        println!("  (none)");
        return;
    }
    for line in lines {
        println!("  {line}");
    }
}

fn print_cli_spaces(snapshot: &HomeSnapshot, raw_name: &str, arg: &str) {
    let query = space_query_for_command(raw_name, arg);
    if query.is_empty() {
        print_cli_section(snapshot, "Spaces", &spaces_summary_lines(snapshot));
        return;
    }

    let normalized = normalize_lookup(&query);
    let Some(root) = snapshot
        .roots
        .iter()
        .find(|root| normalize_lookup(&root.name) == normalized)
    else {
        print_cli_page_header(snapshot, "Spaces");
        println!("Spaces");
        println!("  Unknown root: {}", query.trim());
        println!("  Try: MyWebSite, Public, Local, WebSpaces");
        return;
    };

    let title = format!("Spaces / {}", root.name);
    print_cli_section(snapshot, &title, &space_detail_lines(root, snapshot, 80));
}

fn space_query_for_command(raw_name: &str, arg: &str) -> String {
    let trimmed = arg.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    let normalized = normalize_lookup(raw_name);
    match normalized.as_str() {
        "mywebsite" => "MyWebSite".to_string(),
        "public" => "Public".to_string(),
        "local" => "Local".to_string(),
        "webspaces" => "WebSpaces".to_string(),
        _ => String::new(),
    }
}

fn print_cli_mywebsite(snapshot: &HomeSnapshot) {
    print_cli_section(snapshot, "MyWebSite", &mywebsite_task_lines(snapshot));
}

fn mywebsite_task_lines(snapshot: &HomeSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("Status   {}", website_summary(snapshot)),
        "Stage    mywebsite stage <dir>".to_string(),
        format!(
            "Preview  mywebsite preview ({})",
            action_state_label(action_by_id(snapshot, "site-local"))
        ),
        format!(
            "Publish  mywebsite publish ({})",
            action_state_label(action_by_id(snapshot, "site-ephemeral"))
        ),
        format!(
            "Open     mywebsite open ({})",
            action_state_label(action_by_id(snapshot, "site-open"))
        ),
    ];

    if let Some(url) = snapshot.site.local_url.as_deref() {
        lines.push(format!("Preview  {}", url.trim_end_matches('/')));
    }
    if let Some(release) = snapshot.site.active_release.as_deref() {
        let live = snapshot
            .site
            .active_channel
            .as_deref()
            .map(|channel| format!("{} on {}", release, channel))
            .unwrap_or_else(|| release.to_string());
        lines.push(format!("Live     {live}"));
    } else if snapshot.site.release_count > 0 {
        lines.push(format!("Releases {}", snapshot.site.release_count));
    }
    if let Some(cid) = snapshot.site.active_bundle_cid.as_deref() {
        lines.push(format!("Bundle   elastos://{}", cid));
    }
    if !snapshot.site.staged {
        lines.push("Next     stage a directory containing index.html".to_string());
    }
    lines
}

fn print_cli_inbox(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Inbox");
    println!("Inbox");
    println!(
        "  Attention: {} waiting / {} unread",
        snapshot.notifications.attention_count, snapshot.notifications.unread_count
    );
    let entries = notification_entries(snapshot);
    if entries.is_empty() {
        println!("  No inbox entries waiting.");
        return;
    }
    for entry in entries.iter().take(8) {
        println!(
            "  - [{}{}] {}",
            entry.severity,
            if entry.read { "" } else { ", new" },
            entry.title
        );
        if !entry.body.trim().is_empty() {
            println!("    {}", entry.body);
        }
    }
}

fn print_cli_people(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "People");
    println!("People");
    for line in people_overview_lines(snapshot, 80) {
        println!("  {line}");
    }
    println!();
    println!("Actions");
    let actions = people_actions(snapshot);
    if actions.is_empty() {
        println!("  No People actions are available right now.");
    } else {
        for action in actions.iter().take(12) {
            println!(
                "  - {} [{}]",
                action.label,
                if action.ready { "ready" } else { "setup" }
            );
            println!("    {}", action.command);
            if let Some(reason) = action.reason.as_deref() {
                println!("    {reason}");
            }
        }
    }
}

fn print_cli_wallet(snapshot: &HomeSnapshot) {
    let wallet_available = catalog_capsules(snapshot)
        .iter()
        .any(|capsule| json_text(capsule, "name") == "wallet");
    let wallet_entries = notification_entries(snapshot)
        .iter()
        .filter(|entry| {
            [
                entry.source_app.as_str(),
                entry.kind.as_str(),
                entry.title.as_str(),
                entry.body.as_str(),
                entry
                    .action_ref
                    .as_ref()
                    .map(|action| action.app.as_str())
                    .unwrap_or(""),
            ]
            .join(" ")
            .to_lowercase()
            .contains("wallet")
                || [
                    entry.kind.as_str(),
                    entry.title.as_str(),
                    entry.body.as_str(),
                ]
                .join(" ")
                .to_lowercase()
                .contains("approval")
                || entry.body.to_lowercase().contains("sign")
        })
        .collect::<Vec<_>>();

    print_cli_page_header(snapshot, "Wallet");
    println!("Wallet");
    println!(
        "  Status    {}",
        if wallet_available {
            "available"
        } else {
            "not installed"
        }
    );
    println!("  Requests  {}", wallet_entries.len());
    for entry in wallet_entries.iter().take(8) {
        let action = entry
            .action_ref
            .as_ref()
            .map(|action| format!(" -> {}:{}", action.app, action.action_id))
            .unwrap_or_default();
        println!("  - {}{}", entry.title, action);
    }
}

fn print_cli_services(snapshot: &HomeSnapshot, kind_filter: &str) {
    let offers = cli_service_offers(snapshot, kind_filter);
    let title = if kind_filter == "remote_exit" {
        "Browser Exits"
    } else {
        "Sharing"
    };
    print_cli_page_header(snapshot, title);
    println!("{title}");
    if offers.is_empty() {
        if kind_filter == "remote_exit" {
            println!("  No Browser Exit offers visible in this snapshot.");
        } else {
            println!("  No service offers visible in this snapshot.");
        }
        return;
    }
    for offer in offers.iter().take(12) {
        let id = first_json_text(offer, &["offer_id", "id"]);
        let kind = first_json_text(offer, &["service_kind", "kind"]);
        let name = first_json_text(offer, &["service_display_name", "display_name"]);
        let status = first_json_text(offer, &["status"]);
        let route = first_json_text(offer, &["route"]);
        if kind_filter == "remote_exit" {
            println!(
                "  {:<30} {}",
                if name.is_empty() {
                    if id.is_empty() {
                        "Browser Exit"
                    } else {
                        id
                    }
                } else {
                    name
                },
                if status.is_empty() {
                    "available"
                } else {
                    status
                }
            );
            continue;
        }
        println!(
            "  {:<30} {:<12} {:<12} {}{}",
            if id.is_empty() { "offer" } else { id },
            if kind.is_empty() { "service" } else { kind },
            if status.is_empty() {
                "available"
            } else {
                status
            },
            if name.is_empty() { id } else { name },
            if route.is_empty() {
                String::new()
            } else {
                format!(" -> {route}")
            }
        );
    }
    if offers.len() > 12 {
        println!("  ... {} more offers", offers.len() - 12);
    }
}

fn print_cli_browser(snapshot: &HomeSnapshot) {
    let browser = find_capsule_fact(snapshot, "browser");
    let engine = cli_service_offers(snapshot, "browser_engine")
        .into_iter()
        .next();
    let exits = cli_service_offers(snapshot, "remote_exit");
    print_cli_page_header(snapshot, "Browser");
    println!("Browser");
    println!(
        "  target    {}",
        browser
            .map(|capsule| json_text(capsule, "state"))
            .filter(|state| !state.is_empty())
            .unwrap_or("missing")
    );
    println!(
        "  route     {}",
        browser
            .map(|capsule| json_text(capsule, "route"))
            .filter(|route| !route.is_empty())
            .unwrap_or("/apps/browser/")
    );
    println!(
        "  engine    {}",
        engine
            .map(|offer| first_json_text(offer, &["status", "display_name", "offer_id"]))
            .filter(|text| !text.is_empty())
            .unwrap_or("unknown")
    );
    println!("  exits     {}", exits.len());
    for exit in exits.iter().take(8) {
        let name = first_json_text(exit, &["display_name", "offer_id"]);
        let status = first_json_text(exit, &["status"]);
        println!(
            "  - {} ({})",
            if name.is_empty() {
                "Browser Exit"
            } else {
                name
            },
            if status.is_empty() { "unknown" } else { status }
        );
    }
}

fn cli_service_offers<'a>(
    snapshot: &'a HomeSnapshot,
    kind_filter: &str,
) -> Vec<&'a serde_json::Value> {
    let Some(services) = snapshot.services.as_ref() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut offers = Vec::new();
    for key in [
        "local_offers",
        "remote_offers",
        "available_local_offers",
        "available_remote_offers",
        "service_offers",
    ] {
        if let Some(items) = services.get(key).and_then(|value| value.as_array()) {
            for offer in items {
                let kind = first_json_text(offer, &["service_kind", "kind"]);
                if !kind_filter.is_empty() && kind != kind_filter {
                    continue;
                }
                let id = first_json_text(offer, &["offer_id", "id", "service_uri"]);
                let dedupe = if id.is_empty() {
                    offer.to_string()
                } else {
                    id.to_string()
                };
                if seen.insert(dedupe) {
                    offers.push(offer);
                }
            }
        }
    }
    offers
}

fn first_json_text<'a>(value: &'a serde_json::Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| {
            let text = json_text(value, key);
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .unwrap_or("")
}

fn print_cli_contacts(snapshot: &HomeSnapshot) {
    if snapshot.people.contacts.is_empty() {
        println!("  Contacts   No accepted ElastOS contacts yet.");
        return;
    }
    println!("  Contacts");
    for contact in snapshot.people.contacts.iter().take(8) {
        let device = contact
            .device_label
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!(" on {value}"))
            .unwrap_or_default();
        println!(
            "  - {}{} - {}",
            people_contact_display_name(contact, "Person"),
            device,
            if contact.relationship.trim().is_empty() {
                "connected"
            } else {
                contact.relationship.as_str()
            }
        );
    }
}

fn print_cli_contract(snapshot: &HomeSnapshot) {
    print_cli_page_header(snapshot, "Contract");
    println!("Capsule Interface Contract");
    println!("  Home:       Runtime-owned facts, gates, active-shell state, and host routing");
    println!("  home-gui:   GUI shell surface over the same Home facts");
    println!("  home-cli:   terminal shell surface over the same Home facts");
    println!("  entrypoint: `elastos home` runs this home-cli capsule over local Home state");
    println!("  facts:      Runtime catalog/interface streams are the shared truth");
    println!("  affordance: {DESCRIPTOR_AUTHORITY_COPY}");
    println!(
        "  gates:      {DESCRIPTOR_AUTHORITY_COPY}; Runtime/provider/Inbox gates decide access"
    );
    println!("  carrier:    capsule-to-capsule actions stay provider/Carrier intents");
}

fn print_cli_terminal_contract(snapshot: &HomeSnapshot) {
    let contract = command_contract();
    let terminal = contract.terminal;
    print_cli_page_header(snapshot, "Terminal");
    println!("Home CLI terminal contract");
    println!(
        "  renderer  {}",
        terminal
            .renderer
            .as_deref()
            .unwrap_or("Runtime-owned PTY terminal projection")
    );
    println!(
        "  entrypoint {}",
        terminal
            .entrypoint
            .as_deref()
            .unwrap_or("snapshot dashboard with shared high-level command vocabulary")
    );
    println!(
        "  transport {} ({})",
        terminal.transport.as_deref().unwrap_or("runtime snapshot"),
        terminal
            .transport_scope
            .as_deref()
            .unwrap_or("local_runtime_adapter")
    );
    println!(
        "  input     {}",
        terminal
            .input
            .as_deref()
            .unwrap_or("keyboard, paste, mouse, and resize events -> Runtime-owned PTY stream")
    );
    println!(
        "  PTY       {}",
        terminal.pty.as_deref().unwrap_or("not attached")
    );
    println!(
        "  xterm     {}",
        terminal
            .xterm
            .as_deref()
            .unwrap_or("capsule-local xterm.js renderer over Runtime PTY stream")
    );
    if !contract.controls.is_empty() {
        println!();
        println!("Controls");
        for control in contract.controls {
            println!("  {:<9} {}", control.key, control.description);
        }
    }
}
