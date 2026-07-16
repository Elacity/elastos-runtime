use super::*;

#[test]
fn storage_read_bytes_accepts_direct_provider_body() {
    let body = serde_json::json!({
        "status": "ok",
        "data": {
            "content": [104, 105],
            "size": 2
        }
    });

    assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
}

#[test]
fn storage_read_bytes_accepts_utf8_provider_body() {
    let body = serde_json::json!({
        "status": "ok",
        "data": {
            "content": "{\"ok\":true}",
            "encoding": "utf8",
            "size": 11
        }
    });

    assert_eq!(
        storage_read_bytes_from_result(&body).unwrap(),
        br#"{"ok":true}"#
    );
}

#[test]
fn storage_read_bytes_accepts_runtime_carrier_result_body() {
    let body = serde_json::json!({
        "type": "carrier_result",
        "result": {
            "status": "ok",
            "data": {
                "content": [104, 105],
                "size": 2
            }
        }
    });

    assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
}

#[test]
fn storage_read_bytes_accepts_wrapped_runtime_response() {
    let body = serde_json::json!({
        "response": {
            "type": "carrier_result",
            "result": {
                "status": "ok",
                "data": "hi"
            }
        }
    });

    assert_eq!(storage_read_bytes_from_result(&body).unwrap(), b"hi");
}

#[test]
fn storage_read_bytes_reports_provider_error() {
    let body = serde_json::json!({
        "type": "carrier_result",
        "result": {
            "status": "error",
            "code": "read_failed",
            "message": "no such object"
        }
    });

    let error = storage_read_bytes_from_result(&body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("read_failed"));
    assert!(error.contains("no such object"));
}

fn contract_command<'a>(contract: &'a CommandContract, name: &str) -> &'a CommandSpec {
    contract
        .commands
        .iter()
        .find(|command| command.name == name)
        .unwrap_or_else(|| panic!("missing command contract entry: {name}"))
}

fn assert_contains_all(label: &str, value: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            value.contains(needle),
            "{label} missing invariant phrase: {needle}"
        );
    }
}

#[test]
fn command_contract_is_home_cli_and_terminal_scoped() {
    let contract = command_contract();
    let home_cli_commands: Vec<String> = contract_commands_for("home-cli")
        .into_iter()
        .map(|command| command.name)
        .collect();
    let contract_copy = COMMAND_CONTRACT_JSON.to_string();
    let drift_surfaces: Vec<String> = contract
        .commands
        .iter()
        .flat_map(|command| command.surface.iter())
        .filter(|surface| surface.as_str() == "native" || surface.as_str() == "browser")
        .cloned()
        .collect();

    assert!(
        drift_surfaces.is_empty(),
        "Home CLI command contract must not split entrypoint-specific command vocabularies"
    );
    assert_eq!(
        home_cli_commands,
        vec![
            "home".to_string(),
            "inbox".to_string(),
            "people".to_string(),
            "apps".to_string(),
            "system".to_string(),
            "mywebsite".to_string(),
            "wallet".to_string(),
            "exits".to_string(),
            "invoke".to_string(),
            "debug".to_string(),
            "refresh".to_string(),
            "help".to_string(),
            "signout".to_string(),
            "exit".to_string(),
        ]
    );
    assert_eq!(normalize_contract_command("whoami"), "home");
    assert_eq!(normalize_contract_command("approvals"), "inbox");
    assert_eq!(normalize_contract_command("approve"), "inbox");
    assert_eq!(normalize_contract_command("contacts"), "people");
    assert_eq!(normalize_contract_command("spaces"), "mywebsite");
    assert_eq!(normalize_contract_command("exit-nodes"), "exits");
    assert_eq!(normalize_contract_command("settings"), "system");
    assert_eq!(normalize_contract_command("dev"), "debug");
    assert_eq!(normalize_debug_command("webspaces"), "spaces");
    assert_eq!(normalize_debug_command("shortcuts"), "terminal");
    assert_eq!(
        contract.terminal.transport.as_deref(),
        Some("runtime_pty_stream")
    );
    assert_contains_all(
        "terminal PTY copy",
        contract.terminal.pty.as_deref().unwrap_or_default(),
        &[
            "Runtime-owned PTY",
            "xterm",
            "without direct host process authority",
        ],
    );
    assert_contains_all(
        "terminal entrypoint copy",
        contract.terminal.entrypoint.as_deref().unwrap_or_default(),
        &["same home-cli binary", "Runtime PTY", "elastos home"],
    );
    assert!(
        contract
            .controls
            .iter()
            .any(|control| control.key == "q or Esc" && control.description.contains("Desktop")),
        "shell-switch copy must name the Desktop",
    );
    assert!(
        !COMMAND_CONTRACT_JSON.contains("Home GUI"),
        "command contract should not use a third prose name for home-gui",
    );
    assert!(
        contract_command(&contract, "debug")
            .description
            .contains("hidden from the default Home CLI product surface"),
        "debug help must stay outside the default user-facing surface",
    );
    assert_contains_all(
        "debug authority copy",
        &contract_command(&contract, "debug").description,
        &["declared capability", "not grants"],
    );
    assert_contains_all(
        "plain invoke command copy",
        &contract_command(&contract, "invoke").description,
        &["available app actions", "need approval"],
    );
    for stale in [
        "Runtime-owned Home summary",
        "Runtime facts",
        "structured Home intent",
    ] {
        assert!(
            !contract_copy.contains(stale),
            "Home CLI command contract kept internal public copy: {stale}",
        );
    }
}

#[test]
fn home_cli_public_help_stays_plain_and_debug_keeps_authority_warning() {
    let contract = command_contract();
    for command in contract
        .commands
        .iter()
        .filter(|command| command.name != "debug")
    {
        let copy = format!("{} {}", command.summary, command.description).to_lowercase();
        for internal in [
            "runtime-owned",
            "runtime facts",
            "projection",
            "provider boundary",
            "capability surface",
            "structured home intent",
        ] {
            assert!(
                !copy.contains(internal),
                "{} leaked {internal}",
                command.name
            );
        }
    }
    assert!(DESCRIPTOR_AUTHORITY_COPY.contains("declared capabilities"));
    assert!(DESCRIPTOR_AUTHORITY_COPY.contains("not grants"));
}

#[test]
fn first_run_help_matches_five_tabs_without_power_user_noise() {
    let help = help_lines("").join("\n");

    for heading in ["Home CLI Help", "Tabs", "Controls", "Advanced", "Debug"] {
        assert!(help.contains(heading), "first-run help missing {heading}");
    }
    for command in ["home", "inbox", "people", "apps", "system"] {
        assert!(help.contains(command), "first-run help missing {command}");
    }
    for control in ["refresh", "help [command]", "exit"] {
        assert!(help.contains(control), "first-run help missing {control}");
    }
    assert!(help.contains("help advanced"));
    assert!(help.contains("help debug"));
    for hidden in [
        "mywebsite",
        "wallet",
        "exits",
        "invoke <capsule>",
        "debug [capsules",
        "structured Home intent",
        "Runtime-owned PTY",
        "xterm",
        "launch-token",
        "projection",
        "system [shell",
        "surface:",
    ] {
        assert!(
            !help.contains(hidden),
            "first-run help leaked power/debug noise: {hidden}"
        );
    }
    assert!(!help.to_lowercase().contains("security"));
}

#[test]
fn advanced_and_debug_help_keep_contract_commands_available() {
    let advanced = help_lines("advanced").join("\n");
    let debug = help_lines("debug").join("\n");
    let invoke = help_lines("invoke").join("\n");

    assert!(advanced.contains("Advanced Commands"));
    assert!(advanced.contains("mywebsite [status|stage <dir>|preview|publish|open]"));
    assert!(advanced.contains("wallet"));
    assert!(advanced.contains("exits"));
    assert!(advanced.contains("invoke [list [capsule] | <capsule> <method> [json|target]]"));
    assert!(!advanced.contains("debug [capsules"));
    assert!(!advanced.contains("xterm"));
    assert!(debug.contains("Debug Commands"));
    assert!(debug.contains("debug [capsules"));
    assert!(!debug.contains("Runtime-owned PTY"));
    assert!(invoke.contains("invoke [list [capsule] | <capsule> <method> [json|target]]"));
    assert!(invoke.contains("aliases: call"));
    assert!(invoke.contains("available app actions"));
    assert!(!invoke.contains("structured Home intent"));
}

#[test]
fn dashboard_commands_match_five_tabs_without_power_user_noise() {
    let commands = dashboard_command_hint_lines().join("\n");

    assert!(commands.contains("Commands"));
    for command in ["home", "inbox", "people", "apps", "system"] {
        assert!(
            commands.contains(command),
            "dashboard command hints missing {command}"
        );
    }
    assert!(commands.contains("help advanced"));
    assert!(commands.contains("help debug"));
    for hidden in [
        "Other Commands",
        "mywebsite",
        "wallet",
        "exits",
        "invoke <capsule>",
        "Developer facts",
        "projection",
        "system [shell",
    ] {
        assert!(
            !commands.contains(hidden),
            "dashboard command hints leaked power/debug item: {hidden}"
        );
    }
}

#[test]
fn home_cli_line_mode_accepts_shared_snapshot_backed_commands() {
    let snapshot = sample_snapshot();
    for command in [
        "home",
        "apps",
        "inbox",
        "people",
        "mywebsite",
        "mywebsite status",
        "site",
        "website",
        "spaces",
        "wallet",
        "exits",
        "debug",
        "debug capsules",
        "debug inspect browser",
        "debug affordances browser",
        "debug gates",
        "debug gates browser",
        "debug audit browser",
        "debug people",
        "debug spaces",
        "debug spaces webspaces",
        "debug webspaces",
        "debug services",
        "debug browser",
        "debug terminal",
        "debug contract",
        "system",
        "settings",
        "system shell",
        "system updates",
        "system profile",
        "system diagnostics",
        "invoke",
        "invoke list",
        "invoke list browser",
        "approvals",
        "approve",
    ] {
        assert!(
            handle_shared_line_command(command, &snapshot).unwrap(),
            "Home CLI line mode did not accept shared command: {command}",
        );
    }
    for command in [
        "capsules",
        "inspect browser",
        "affordances browser",
        "gates browser",
        "audit browser",
        "services",
        "browser",
        "terminal",
        "contract",
    ] {
        assert!(
            !handle_shared_line_command(command, &snapshot).unwrap(),
            "developer command should require explicit debug prefix: {command}",
        );
    }
    assert_eq!(normalize_contract_command("call"), "invoke");
}

#[test]
fn system_line_mode_emits_home_gui_shell_switch() {
    let mut snapshot = sample_snapshot();
    snapshot.active_shell.active = Some("home-cli".to_string());
    snapshot.session.mode = "browser_pty".to_string();

    assert_eq!(
        system_line_action("system shell home-gui", &snapshot).unwrap(),
        Some("shell-switch:home-gui".to_string())
    );
    assert_eq!(
        system_line_action("settings shell gui", &snapshot).unwrap(),
        Some("shell-switch:home-gui".to_string())
    );
    assert!(system_line_action("system shell browser", &snapshot).is_err());

    snapshot.active_shell.active = Some("home-gui".to_string());
    assert!(system_line_action("system shell home-gui", &snapshot).is_err());

    snapshot.active_shell.active = Some("home-cli".to_string());
    snapshot.session.mode = "native_terminal".to_string();
    assert!(system_line_action("system shell home-gui", &snapshot).is_err());

    snapshot.session.mode = "browser_pty".to_string();
    snapshot
        .active_shell
        .candidates
        .retain(|candidate| candidate.name != "home-gui");
    let error = system_line_action("system shell home-gui", &snapshot)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Desktop is not available"));
}

#[test]
fn sign_out_is_available_only_for_browser_home_sessions() {
    let mut snapshot = sample_snapshot();
    snapshot.session.mode = "browser_pty".to_string();

    for command in ["signout", "logout", "sign-out"] {
        assert_eq!(
            system_line_action(command, &snapshot).unwrap(),
            Some("auth-sign-out".to_string())
        );
    }
    assert!(system_line_action("signout now", &snapshot).is_err());

    snapshot.session.mode = "native_terminal".to_string();
    let error = system_line_action("signout", &snapshot)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Use exit to close native Home CLI"));
}

#[test]
fn system_cli_exposes_shell_status_not_debug_dump() {
    let snapshot = sample_snapshot();
    let lines = system_settings_lines(&snapshot).join("\n");
    assert!(lines.contains("system shell home-gui"));
    assert!(lines.contains("signout"));
    assert!(lines.contains("Home"));
    assert!(lines.contains("Updates"));
    assert!(!lines.contains("Session"));
    assert!(!lines.contains("Profile"));
    assert!(!lines.contains("Diagnostics"));
    assert!(!lines.contains("system diagnostics"));
    assert!(!lines.contains("Services"));
    assert!(!lines.contains("Offers"));
    assert!(!lines.contains("Roots"));
    assert!(!lines.contains("Peers"));
    assert!(!lines.contains("Capsules"));
    assert!(!lines.contains("browser Runtime PTY"));
    assert!(!lines.contains("managed"));
    assert!(!lines.contains("DID"));
    assert!(!lines.contains("did:key"));
    assert!(!lines.contains("launch-token"));
    assert_ne!(normalize_system_topic("services"), "diagnostics");
    assert!(!system_identity_lines(&snapshot)
        .join("\n")
        .contains("launch-token"));
    assert!(!system_diagnostics_lines(&snapshot)
        .join("\n")
        .contains("launch-token"));

    let state = TuiState {
        tab: Tab::System,
        ..TuiState::default()
    };
    let screen = build_tui_screen(&snapshot, &state, 100, 30);
    assert!(screen.contains("System"));
    assert!(screen.contains("Return to Home Desktop"));
    assert!(screen.contains("system shell home-gui"));
    assert!(screen.contains("Sign out"));
    assert!(screen.contains("Shell"));
    assert!(screen.contains("Status"));
    assert!(!screen.contains("Commands"));
    assert!(!screen.contains("Profile"));
    assert!(!screen.contains("Diagnostics"));
    assert!(!screen.contains("Settings"));
    assert!(!screen.contains("Trusted Source"));
    assert!(!screen.contains("Health"));
    assert!(!screen.contains("Services"));
    assert!(!screen.contains("Offers"));
    assert!(!screen.contains("Roots"));
    assert!(!screen.contains("Peers"));
    assert!(!screen.contains("Capsules"));
    assert!(!screen.contains("browser Runtime PTY"));
    assert!(!screen.contains("managed"));
    assert!(!screen.contains("DID"));
    assert!(!screen.contains("did:key"));
    assert!(!screen.contains("launch-token"));
}

#[test]
fn people_line_mode_emits_snapshot_backed_people_actions() {
    let mut snapshot = sample_snapshot();
    snapshot.people = PeopleStatus {
        contact_count: 1,
        contacts: vec![PeopleContactStatus {
            contact_id: "contact-alice".to_string(),
            display_name: "Alice".to_string(),
            relationship: "connected".to_string(),
            route: "/apps/chat-room/".to_string(),
            can_message: true,
            ..PeopleContactStatus::default()
        }],
        discovery: PeopleDiscoveryStatus {
            enabled: true,
            remaining_seconds: Some(60),
            discovered_peers: vec![PeopleDiscoveryPeerStatus {
                peer_id: "peer-bob".to_string(),
                display_name: "Bob".to_string(),
                status: "visible".to_string(),
                ..PeopleDiscoveryPeerStatus::default()
            }],
            requests: vec![PeopleDiscoveryRequestStatus {
                request_id: "request-carol".to_string(),
                peer_id: "peer-carol".to_string(),
                display_name: "Carol".to_string(),
                status: "incoming".to_string(),
                ..PeopleDiscoveryRequestStatus::default()
            }],
            ..PeopleDiscoveryStatus::default()
        },
        ..PeopleStatus::default()
    };

    assert_eq!(
        people_line_action("people discovery off", &snapshot).unwrap(),
        Some("people-discovery-disable".to_string())
    );
    assert_eq!(
        people_line_action("discovery refresh", &snapshot).unwrap(),
        Some("people-discovery-refresh".to_string())
    );
    assert_eq!(
        people_line_action("people request peer-bob", &snapshot).unwrap(),
        Some("people-request-peer:peer-bob".to_string())
    );
    assert_eq!(
        people_line_action("people accept request-carol", &snapshot).unwrap(),
        Some("people-accept-request:request-carol".to_string())
    );
    assert_eq!(
        people_line_action("people message contact-alice", &snapshot).unwrap(),
        Some("people-message:contact-alice".to_string())
    );
    assert_eq!(
        people_line_action("people remove contact-alice", &snapshot).unwrap(),
        Some("people-remove-contact:contact-alice".to_string())
    );
    assert!(people_line_action("people request missing", &snapshot)
        .unwrap_err()
        .to_string()
        .contains("not available"));
    assert_eq!(people_line_action("people", &snapshot).unwrap(), None);
}

#[test]
fn people_line_mode_resolves_visible_contacts_and_requires_message_route() {
    let mut snapshot = sample_snapshot();
    snapshot.people = PeopleStatus {
        contact_count: 1,
        contacts: vec![PeopleContactStatus {
            contact_id: "contact-alice".to_string(),
            display_name: "Alice".to_string(),
            handle: Some("@alice".to_string()),
            relationship: "conversation".to_string(),
            route: "/apps/chat-room/".to_string(),
            can_message: true,
            profile_card: Some(PeopleProfileCardStatus {
                display_name: "Alice A.".to_string(),
                handle: Some("@alice-a".to_string()),
            }),
            ..PeopleContactStatus::default()
        }],
        ..PeopleStatus::default()
    };

    assert_eq!(
        people_line_action("people message Alice A.", &snapshot).unwrap(),
        Some("people-message:contact-alice".to_string())
    );
    assert_eq!(
        people_line_action("people chat @alice-a", &snapshot).unwrap(),
        Some("people-message:contact-alice".to_string())
    );
    assert_eq!(
        people_line_action("people remove alice", &snapshot).unwrap(),
        Some("people-remove-contact:contact-alice".to_string())
    );

    snapshot.people.contacts[0].route = "elastos://peer/peer-alice".to_string();
    assert!(
        people_line_action("people message contact-alice", &snapshot)
            .unwrap_err()
            .to_string()
            .contains("not available")
    );
}

#[test]
fn tui_snapshot_refresh_picks_up_people_changes() {
    let mut snapshot = sample_snapshot();
    let mut fingerprint = snapshot_render_fingerprint(&snapshot);
    let before = build_tui_screen(&snapshot, &TuiState::default(), 120, 30);
    assert!(!before.contains("Alice"));

    let mut next_snapshot = snapshot.clone();
    next_snapshot.people = PeopleStatus {
        contact_count: 1,
        contacts: vec![PeopleContactStatus {
            contact_id: "contact-alice".to_string(),
            display_name: "Alice".to_string(),
            relationship: "connected".to_string(),
            route: "/apps/chat-room/".to_string(),
            can_message: true,
            ..PeopleContactStatus::default()
        }],
        ..PeopleStatus::default()
    };

    assert!(apply_tui_snapshot_refresh(
        &mut snapshot,
        &mut fingerprint,
        next_snapshot
    ));
    let state = TuiState {
        tab: Tab::People,
        ..TuiState::default()
    };
    let after = build_tui_screen(&snapshot, &state, 120, 30);
    assert!(after.contains("Alice"));
    assert!(after.contains("connected"));
}

#[test]
fn mywebsite_line_mode_emits_explicit_site_actions() {
    assert_eq!(mywebsite_line_action("mywebsite").unwrap(), None);
    assert_eq!(mywebsite_line_action("spaces status").unwrap(), None);
    assert_eq!(
        mywebsite_line_action("mywebsite stage /tmp/my site").unwrap(),
        Some("site-stage:/tmp/my site".to_string())
    );
    assert_eq!(
        mywebsite_line_action("site preview").unwrap(),
        Some("site-local".to_string())
    );
    assert_eq!(
        mywebsite_line_action("website publish").unwrap(),
        Some("site-ephemeral".to_string())
    );
    assert_eq!(
        mywebsite_line_action("spaces open").unwrap(),
        Some("site-open".to_string())
    );
    assert!(mywebsite_line_action("mywebsite stage")
        .unwrap_err()
        .to_string()
        .contains("stage <dir>"));
}

#[test]
fn home_cli_pages_keep_context_header() {
    let snapshot = sample_snapshot();
    let header = cli_page_header(&snapshot, "People");

    assert!(header.starts_with("\x1B[2J\x1B[HHome CLI / People\n"));
    assert!(header.contains("user alex"));
    assert!(header.contains("identity did:key:z6M"));
    assert!(header.contains("shell home-cli"));
    assert!(!header.contains("ElastOS Home"));
}

#[test]
fn system_pages_hide_peer_context_in_header() {
    let snapshot = sample_snapshot();
    let header = cli_system_page_header(&snapshot, "System");

    assert!(header.starts_with("\x1B[2J\x1B[HHome CLI / System\n"));
    assert!(header.contains("identity did:key:z6M"));
    assert!(header.contains("shell home-cli"));
    assert!(!header.contains("user alex"));
    assert!(!header.contains("network"));
    assert!(!header.contains("Carrier"));
}

#[test]
fn debug_spaces_aliases_resolve_to_selected_roots() {
    assert_eq!(space_query_for_command("webspaces", ""), "WebSpaces");
    assert_eq!(space_query_for_command("mywebsite", ""), "MyWebSite");
    assert_eq!(space_query_for_command("spaces", "public"), "public");

    let snapshot = sample_snapshot();
    let webspaces = snapshot
        .roots
        .iter()
        .find(|root| root.name == "WebSpaces")
        .expect("sample WebSpaces root missing");
    let lines = space_detail_lines(webspaces, &snapshot, 80);

    assert!(lines.iter().any(|line| line == "Group      Spaces"));
    assert!(lines
        .iter()
        .any(|line| line.contains("localhost://WebSpaces/Elastos")));
    assert!(lines.iter().any(|line| line.contains("elastos webspace")));
}

#[test]
fn mywebsite_page_is_task_oriented_and_hides_space_roots() {
    let snapshot = sample_snapshot();
    let lines = mywebsite_task_lines(&snapshot);
    let text = lines.join("\n");

    assert!(text.contains("Stage    mywebsite stage <dir>"));
    assert!(text.contains("Preview  mywebsite preview"));
    assert!(text.contains("Publish  mywebsite publish"));
    assert!(text.contains("Open     mywebsite open"));
    assert!(!text.contains("WebSpaces"));
    assert!(!text.contains("scratch space"));
    assert!(!text.contains("localhost://Local"));
}

#[test]
fn home_cli_line_mode_reads_browser_exit_service_offers() {
    let snapshot = sample_snapshot();
    let exits = cli_service_offers(&snapshot, "remote_exit");
    let names = exits
        .iter()
        .map(|offer| first_json_text(offer, &["display_name", "offer_id"]))
        .collect::<Vec<_>>();

    assert_eq!(exits.len(), 2);
    assert!(names.contains(&"Browser Exit node"));
    assert!(names.contains(&"Seed Node Browser Exit"));
}

#[test]
fn home_cli_line_mode_builds_low_risk_invoke_intent() {
    let snapshot = sample_snapshot();
    let intent = resolve_cli_invoke_intent("home-cli capsule.open", &snapshot).unwrap();
    assert_eq!(intent.capsule, "home-cli");
    assert_eq!(intent.interface_id, "elastos.shell.cli");
    assert_eq!(intent.method, "capsule.open");
    assert_eq!(intent.resource, "elastos://capsules/*");
    assert_eq!(intent.input, serde_json::json!({}));
}

#[test]
fn home_cli_line_mode_rejects_declared_but_non_executable_method() {
    let snapshot = sample_snapshot();
    let error = resolve_cli_invoke_intent("browser open", &snapshot)
        .unwrap_err()
        .to_string();
    assert!(error.contains("provider capability path"));
}

#[test]
fn home_cli_invoke_list_contains_only_runtime_executable_methods() {
    let snapshot = sample_snapshot();
    assert_eq!(
        cli_invokable_methods(&snapshot, None),
        vec![("home-cli", "elastos.shell.cli", "capsule.open")]
    );
    assert!(cli_invokable_methods(&snapshot, Some("browser")).is_empty());
}

#[test]
fn home_cli_line_mode_serializes_structured_invoke_home_intent() {
    let snapshot = sample_snapshot();
    let intent =
        resolve_cli_invoke_intent("home-cli capsule.open {\"target\":\"browser\"}", &snapshot)
            .unwrap();
    let payload = home_intent_payload("invoke", Some(intent)).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "action": "invoke",
            "invoke": {
                "capsule": "home-cli",
                "interface": "elastos.shell.cli",
                "method": "capsule.open",
                "resource": "elastos://capsules/*",
                "input": {
                    "target": "browser"
                }
            }
        })
    );
}

#[test]
fn home_cli_line_mode_blocks_high_risk_invoke_intent() {
    let mut snapshot = sample_snapshot();
    let methods = snapshot
        .capsule_interfaces
        .as_mut()
        .and_then(|registry| registry.get_mut("interfaces"))
        .and_then(|interfaces| interfaces.as_array_mut())
        .and_then(|interfaces| interfaces.first_mut())
        .and_then(|entry| entry.get_mut("interface"))
        .and_then(|interface| interface.get_mut("methods"))
        .and_then(|methods| methods.as_array_mut())
        .expect("sample interface methods");
    methods.push(serde_json::json!({
        "id": "payment.send",
        "risk": "payment",
        "approval": "runtime_policy"
    }));
    snapshot
        .capsule_interfaces
        .as_mut()
        .and_then(|registry| registry.get_mut("interfaces"))
        .and_then(|interfaces| interfaces.as_array_mut())
        .and_then(|interfaces| interfaces.first_mut())
        .and_then(|entry| entry.get_mut("bindings"))
        .and_then(|bindings| bindings.as_array_mut())
        .expect("sample interface bindings")
        .push(serde_json::json!({
            "method": "payment.send",
            "state": "approval-required",
            "handler_available": true,
            "executable": false,
            "reason": "payment risk requires explicit user approval"
        }));
    let error = resolve_cli_invoke_intent("browser payment.send", &snapshot)
        .unwrap_err()
        .to_string();
    assert!(error.contains("payment risk requires explicit user approval"));
}

fn sample_snapshot() -> HomeSnapshot {
    HomeSnapshot {
        version: "0.1.0".to_string(),
        user: "alex".to_string(),
        nickname: Some("alex".to_string()),
        did: Some("did:key:z6MkhExample".to_string()),
        session: HomeCliSessionStatus {
            mode: "browser_pty".to_string(),
        },
        source: Some(SourceStatus {
            name: "elastos.elacitylabs.com".to_string(),
            channel: "stable".to_string(),
            gateway: Some("https://elastos.elacitylabs.com".to_string()),
        }),
        runtime: RuntimeStatus {
            running: true,
            kind: Some("managed".to_string()),
            peer_count: Some(2),
            ticket: Some("ticket:example".to_string()),
            running_capsules: vec!["chat".to_string()],
        },
        services: Some(serde_json::json!({
            "schema": "elastos.runtime.services/v1",
            "local_offers": [{
                "offer_id": "local:provider:browser-exit",
                "service_kind": "remote_exit",
                "display_name": "Browser Exit node",
                "status": "configured",
                "route": "/apps/browser/"
            }],
            "remote_offers": [{
                "offer_id": "remote:seed:browser-exit",
                "service_kind": "remote_exit",
                "display_name": "Seed Node Browser Exit",
                "status": "available"
            }]
        })),
        site: SiteStatus {
            staged: true,
            local_url: None,
            active_release: None,
            active_channel: None,
            active_bundle_cid: None,
            release_count: 0,
        },
        shares: ShareStatus::default(),
        room: RoomStatus {
            room_slug: "chat-room".to_string(),
            title: "Room".to_string(),
            owner_did: Some("did:key:z6Mkowner".to_string()),
            current_key_epoch: 1,
            admin_count: 1,
            member_count: 3,
            active_member_count: 1,
            pending_invite_count: 0,
            allow_guest_invites: true,
            allow_member_invites: true,
            allow_members_to_host_guests: true,
            local_runtime_role: Some("owner".to_string()),
            canonical_hosted_guest_url: Some(
                "https://elastos.elacitylabs.com/apps/chat-room/".to_string(),
            ),
            ephemeral_hosted_guest_url: None,
            browser_access_allowed: true,
            browser_access_block_reason: None,
            pending_count: 0,
            active_session_count: 0,
            active_participants: Vec::new(),
            pending_requests: Vec::new(),
            active_sessions: Vec::new(),
            members: vec![RoomMemberStatus {
                member_did: "did:key:z6MkhExample".to_string(),
                role: "owner".to_string(),
            }],
            pending_invites: Vec::new(),
        },
        people: PeopleStatus::default(),
        notifications: NotificationStatus::default(),
        roots: vec![
            RootStatus {
                name: "Users".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://Users".to_string(),
                path: Some("/tmp/Users".to_string()),
                exists: true,
                description: "People root".to_string(),
                example: "localhost://Users/<principal-root>".to_string(),
            },
            RootStatus {
                name: "UsersAI".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://UsersAI".to_string(),
                path: Some("/tmp/UsersAI".to_string()),
                exists: true,
                description: "AI root".to_string(),
                example: "localhost://UsersAI/self".to_string(),
            },
            RootStatus {
                name: "MyWebSite".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://MyWebSite".to_string(),
                path: Some("/tmp/MyWebSite".to_string()),
                exists: true,
                description: "Site root".to_string(),
                example: "localhost://MyWebSite/index.html".to_string(),
            },
            RootStatus {
                name: "Public".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://Public".to_string(),
                path: Some("/tmp/Public".to_string()),
                exists: true,
                description: "Shared root".to_string(),
                example: "localhost://Public/manual.pdf".to_string(),
            },
            RootStatus {
                name: "Local".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://Local".to_string(),
                path: Some("/tmp/Local".to_string()),
                exists: true,
                description: "Local root".to_string(),
                example: "localhost://Local/Shared".to_string(),
            },
            RootStatus {
                name: "WebSpaces".to_string(),
                kind: "dynamic".to_string(),
                uri: "localhost://WebSpaces".to_string(),
                path: None,
                exists: false,
                description: "Dynamic root".to_string(),
                example: "localhost://WebSpaces/Elastos".to_string(),
            },
            RootStatus {
                name: "ElastOS".to_string(),
                kind: "file-backed".to_string(),
                uri: "localhost://ElastOS".to_string(),
                path: Some("/tmp/ElastOS".to_string()),
                exists: true,
                description: "System root".to_string(),
                example: "localhost://ElastOS/SystemRegistry".to_string(),
            },
        ],
        actions: vec![
            ActionInfo {
                id: "chat".to_string(),
                label: "Chat".to_string(),
                description: String::new(),
                command: "home: open Chat".to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "site-local".to_string(),
                label: "Preview".to_string(),
                description: String::new(),
                command: "home: start MyWebSite local preview".to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "site-ephemeral".to_string(),
                label: "Publish".to_string(),
                description: String::new(),
                command: "home: publish a temporary HTTPS URL for MyWebSite".to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "site-open".to_string(),
                label: "Open".to_string(),
                description: String::new(),
                command: "home: open MyWebSite preview in browser".to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "shares-list".to_string(),
                label: "Shared".to_string(),
                description: String::new(),
                command: "elastos shares list".to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "capsule-gba-ucity".to_string(),
                label: "gba-ucity".to_string(),
                description: "Bundled uCity demo cartridge".to_string(),
                command: "elastos capsule gba-ucity --lifecycle interactive --interactive"
                    .to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "capsule-gba-emulator".to_string(),
                label: "gba-emulator".to_string(),
                description: "Browser GBA viewer bundle".to_string(),
                command: "elastos capsule gba-emulator --lifecycle interactive --interactive"
                    .to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "capsule-browser".to_string(),
                label: "browser".to_string(),
                description: "Browse websites from this device.".to_string(),
                command: "elastos capsule browser --lifecycle interactive --interactive"
                    .to_string(),
                ready: true,
                reason: None,
            },
            ActionInfo {
                id: "capsule-mystery-capsule".to_string(),
                label: "mystery-capsule".to_string(),
                description: "Unknown capsule".to_string(),
                command: "elastos capsule mystery-capsule --lifecycle interactive --interactive"
                    .to_string(),
                ready: true,
                reason: None,
            },
        ],
        active_shell: ActiveShellStatus {
            active: Some("home-cli".to_string()),
            candidates: vec![
                ActiveShellCandidateStatus {
                    name: "home-cli".to_string(),
                    launchable: true,
                },
                ActiveShellCandidateStatus {
                    name: "home-gui".to_string(),
                    launchable: true,
                },
            ],
        },
        targets: vec![
            HomeTargetStatus {
                target: "browser".to_string(),
                title: "Browser".to_string(),
                description: "Browse websites from this device.".to_string(),
                role: "app".to_string(),
                target_kind: "app".to_string(),
                viewer: None,
                viewer_title: None,
            },
            HomeTargetStatus {
                target: "chat".to_string(),
                title: "Chat".to_string(),
                description: "Talk to people and connected ElastOS homes.".to_string(),
                role: "app".to_string(),
                target_kind: "app".to_string(),
                viewer: None,
                viewer_title: None,
            },
            HomeTargetStatus {
                target: "gba-emulator".to_string(),
                title: "GBA Emulator".to_string(),
                description: "Browser GBA viewer bundle.".to_string(),
                role: "app".to_string(),
                target_kind: "app".to_string(),
                viewer: None,
                viewer_title: None,
            },
            HomeTargetStatus {
                target: "gba-ucity".to_string(),
                title: "uCity".to_string(),
                description: "Bundled uCity demo cartridge.".to_string(),
                role: "content".to_string(),
                target_kind: "object".to_string(),
                viewer: Some("gba-emulator".to_string()),
                viewer_title: Some("GBA Emulator".to_string()),
            },
            HomeTargetStatus {
                target: "people".to_string(),
                title: "People".to_string(),
                description: "See accepted ElastOS contacts and start conversations.".to_string(),
                role: "app".to_string(),
                target_kind: "app".to_string(),
                viewer: None,
                viewer_title: None,
            },
            HomeTargetStatus {
                target: "home-cli".to_string(),
                title: "Home CLI".to_string(),
                description: "Alternate Home shell.".to_string(),
                role: "shell".to_string(),
                target_kind: "app".to_string(),
                viewer: None,
                viewer_title: None,
            },
        ],
        cached_capsules: vec![
            "chat".to_string(),
            "agent".to_string(),
            "mystery-capsule".to_string(),
        ],
        capsule_catalog: Some(serde_json::json!({
            "schema": "elastos.capsules.catalog/v1",
            "counts": {
                "total": 4,
                "installed": 4,
                "launchable": 4,
                "interfaces": 3,
                "methods": 4
            },
            "capsules": [
                {
                    "name": "browser",
                    "version": "0.1.0",
                    "title": "Browser",
                    "role": "app",
                    "type": "wasm",
                    "state": "installed",
                    "installed": true,
                    "launchable": true,
                    "launch_target": "browser",
                    "route": "/apps/browser/",
                    "interfaces": [{
                        "id": "elastos.browser.page",
                        "methods": [
                            { "id": "page_status", "risk": "read", "approval": "runtime_policy" },
                            { "id": "open", "risk": "launch", "approval": "runtime_policy" }
                        ]
                    }],
                    "projection": {
                        "web": { "state": "available" },
                        "cli": { "state": "facts-only" },
                        "facts": { "state": "available" },
                        "affordances": { "state": "declared" },
                        "gates": {
                            "state": "declared",
                            "note": "Runtime route policy, launch tokens, Inbox/Wallet approval, and provider gates remain authoritative."
                        },
                        "audit_mirror": {
                            "state": "redacted",
                            "note": "signature=no-manifest-signature; cid=local-only; payment=not-declared; drm=not-declared; ordinary shells receive redacted mirror facts."
                        },
                        "carrier": { "state": "requires-provider-intents" }
                    },
                    "cid_state": "local-only",
                    "signature_state": "no-manifest-signature",
                    "trust_state": "local-dev",
                    "payment_state": "not-declared",
                    "drm_state": "not-declared",
                    "source": "installed"
                },
                {
                    "name": "gba-emulator",
                    "version": "0.1.0",
                    "title": "GBA Emulator",
                    "role": "viewer",
                    "type": "wasm",
                    "state": "installed",
                    "installed": true,
                    "launchable": true,
                    "launch_target": "gba-emulator",
                    "route": "/apps/gba-emulator/",
                    "accepted_content": [{
                        "name": "gba-ucity",
                        "title": "uCity",
                        "description": "Bundled uCity demo cartridge.",
                        "entrypoint": "ucity.gba"
                    }],
                    "interfaces": [{
                        "id": "elastos.gba.emulator",
                        "methods": [
                            {
                                "id": "rom.open",
                                "risk": "launch",
                                "approval": "runtime_policy",
                                "input_schema": {
                                    "accepts": [{ "extensions": [".gba"] }]
                                }
                            }
                        ]
                    }],
                    "projection": {
                        "web": { "state": "available" },
                        "cli": { "state": "facts-only" },
                        "facts": { "state": "available" },
                        "affordances": { "state": "declared" },
                        "gates": { "state": "declared" },
                        "audit_mirror": { "state": "redacted" },
                        "carrier": { "state": "requires-provider-intents" }
                    },
                    "cid_state": "local-only",
                    "signature_state": "no-manifest-signature",
                    "trust_state": "local-dev",
                    "payment_state": "not-declared",
                    "drm_state": "not-declared",
                    "source": "installed"
                },
                {
                    "name": "gba-ucity",
                    "version": "0.1.0",
                    "title": "uCity",
                    "role": "content",
                    "type": "data",
                    "state": "installed",
                    "installed": true,
                    "launchable": true,
                    "launch_target": "gba-ucity",
                    "route": "/apps/gba-emulator/?capsule=gba-ucity",
                    "viewer": "gba-emulator",
                    "viewer_title": "GBA Emulator",
                    "projection": {
                        "web": { "state": "available" },
                        "cli": { "state": "facts-only" },
                        "facts": { "state": "available" },
                        "affordances": { "state": "absent" },
                        "gates": { "state": "absent" },
                        "audit_mirror": { "state": "redacted" },
                        "carrier": { "state": "none" }
                    },
                    "cid_state": "local-only",
                    "signature_state": "no-manifest-signature",
                    "trust_state": "local-dev",
                    "payment_state": "not-declared",
                    "drm_state": "not-declared",
                    "source": "installed"
                },
                {
                    "name": "home-cli",
                    "version": "0.1.0",
                    "title": "Home CLI",
                    "role": "shell",
                    "type": "wasm",
                    "state": "installed",
                    "installed": true,
                    "launchable": true,
                    "launch_target": "home-cli",
                    "route": "/apps/home-cli/",
                    "interfaces": [],
                    "projection": {
                        "web": { "state": "available" },
                        "cli": { "state": "available" },
                        "facts": { "state": "available" },
                        "affordances": { "state": "absent" },
                        "gates": { "state": "absent" },
                        "audit_mirror": { "state": "redacted" },
                        "carrier": { "state": "none" }
                    },
                    "cid_state": "local-only",
                    "signature_state": "no-manifest-signature",
                    "trust_state": "local-dev",
                    "payment_state": "not-declared",
                    "drm_state": "not-declared",
                    "source": "installed"
                }
            ]
        })),
        capsule_interfaces: Some(serde_json::json!({
            "schema": "elastos.capsules.interfaces/v1",
            "counts": {
                "capsules": 3,
                "interfaces": 3,
                "methods": 4,
                "executable_methods": 1
            },
            "interfaces": [
                {
                    "capsule": "browser",
                    "capsule_version": "0.1.0",
                    "title": "Browser",
                    "role": "app",
                    "type": "wasm",
                    "trust_state": "local-dev",
                    "interface": {
                        "id": "elastos.browser.page",
                        "title": "Browser Page",
                        "methods": [
                            { "id": "page_status", "risk": "read", "approval": "runtime_policy" },
                            { "id": "open", "risk": "launch", "approval": "runtime_policy" }
                        ]
                    },
                    "bindings": [
                        {
                            "method": "page_status",
                            "state": "handler-unavailable",
                            "handler_available": false,
                            "executable": false,
                            "reason": "no live Runtime or provider handler is registered for this method"
                        },
                        {
                            "method": "open",
                            "state": "provider-path-only",
                            "handler_available": true,
                            "executable": false,
                            "handler_kind": "provider",
                            "handler": "browser-engine-adapter",
                            "reason": "available through the provider capability path, not generic interface invocation"
                        }
                    ]
                },
                {
                    "capsule": "gba-emulator",
                    "capsule_version": "0.1.0",
                    "title": "GBA Emulator",
                    "role": "viewer",
                    "type": "wasm",
                    "trust_state": "local-dev",
                    "interface": {
                        "id": "elastos.gba.emulator",
                        "title": "GBA Emulator",
                        "methods": [{
                            "id": "rom.open",
                            "risk": "launch",
                            "approval": "runtime_policy",
                            "input_schema": {
                                "accepts": [{ "extensions": [".gba"] }]
                            }
                        }]
                    },
                    "bindings": [{
                        "method": "rom.open",
                        "state": "provider-path-only",
                        "handler_available": true,
                        "executable": false,
                        "handler_kind": "provider",
                        "handler": "object-provider"
                    }]
                },
                {
                    "capsule": "home-cli",
                    "capsule_version": "0.1.0",
                    "title": "Home CLI",
                    "role": "shell",
                    "type": "wasm",
                    "trust_state": "local-dev",
                    "interface": {
                        "id": "elastos.shell.cli",
                        "title": "Home CLI",
                        "methods": [
                            {
                                "id": "capsule.open",
                                "risk": "launch",
                                "approval": "runtime_policy",
                                "resource": "elastos://capsules/*",
                                "operation": "launch"
                            }
                        ]
                    },
                    "bindings": [{
                        "method": "capsule.open",
                        "state": "executable",
                        "handler_available": true,
                        "executable": true,
                        "handler_kind": "runtime",
                        "handler": "runtime.capsule.launch"
                    }]
                }
            ]
        })),
        notice: None,
    }
}

#[derive(Clone)]
struct InboxFixtureScenario {
    name: &'static str,
    entry: NotificationEntryStatus,
    primary_action: ActionInfo,
    extra_actions: Vec<ActionInfo>,
    primary_action_id: &'static str,
}

impl InboxFixtureScenario {
    fn apply(&self, snapshot: &mut HomeSnapshot) {
        snapshot.notifications.entries = vec![self.entry.clone()];
        snapshot.notifications.unread_count = 1;
        snapshot.notifications.attention_count = usize::from(self.entry.severity == "attention");
        snapshot.actions.push(self.primary_action.clone());
        snapshot.actions.extend(self.extra_actions.clone());
    }
}

fn inbox_action(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    command: &'static str,
) -> ActionInfo {
    ActionInfo {
        id: id.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        command: command.to_string(),
        ready: true,
        reason: None,
    }
}

fn inbox_entry(
    id: &'static str,
    source_app: &'static str,
    kind: &'static str,
    title: &'static str,
    body: &'static str,
    severity: &'static str,
    action: (&'static str, &'static str),
) -> NotificationEntryStatus {
    NotificationEntryStatus {
        id: id.to_string(),
        source_app: source_app.to_string(),
        kind: kind.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        action_ref: Some(NotificationActionRefStatus {
            app: action.0.to_string(),
            action_id: action.1.to_string(),
        }),
        read: false,
        severity: severity.to_string(),
    }
}

fn wallet_signing_inbox_fixture() -> InboxFixtureScenario {
    InboxFixtureScenario {
        name: "wallet signing",
        entry: inbox_entry(
            "wallet-signing:tx-1",
            "wallet",
            "wallet_signing_request",
            "Wallet signature requested",
            "ela.city wants Wallet to sign a transaction.",
            "attention",
            ("wallet", "open-gui:wallet"),
        ),
        primary_action: inbox_action(
            "open-gui:wallet",
            "Open Wallet",
            "Review and sign or reject the pending wallet request.",
            "home: open Wallet",
        ),
        extra_actions: Vec::new(),
        primary_action_id: "open-gui:wallet",
    }
}

fn inspect_approval_inbox_fixture() -> InboxFixtureScenario {
    InboxFixtureScenario {
        name: "inspect approval",
        entry: inbox_entry(
            "inspect-approval:key-release-1",
            "system",
            "inspect_approval_request",
            "Inspect approval requested",
            "Capsule Inspector wants approval for key.release.",
            "attention",
            ("system", "open-gui:system"),
        ),
        primary_action: inbox_action(
            "open-gui:system",
            "Open System",
            "Review the Inspector gate preview before approving.",
            "home: open System",
        ),
        extra_actions: Vec::new(),
        primary_action_id: "open-gui:system",
    }
}

fn people_request_inbox_fixture() -> InboxFixtureScenario {
    InboxFixtureScenario {
        name: "people request",
        entry: inbox_entry(
            "people-request:people-req-1",
            "people",
            "people_request",
            "Bob wants to connect",
            "Bob from peer-bob sent a People request.",
            "attention",
            ("people", "people-accept-request:people-req-1"),
        ),
        primary_action: inbox_action(
            "people-accept-request:people-req-1",
            "Accept Bob",
            "Accept this People request.",
            "home: accept People request",
        ),
        extra_actions: Vec::new(),
        primary_action_id: "people-accept-request:people-req-1",
    }
}

fn chat_guest_inbox_fixture() -> InboxFixtureScenario {
    InboxFixtureScenario {
        name: "chat guest request",
        entry: inbox_entry(
            "room-access-request:chat-guest-1",
            "chat-room",
            "room_access_request",
            "Alice wants to join Chat",
            "Alice on Phone wants to join Chat.",
            "attention",
            ("chat-room", "room-approve-request:chat-guest-1"),
        ),
        primary_action: inbox_action(
            "room-approve-request:chat-guest-1",
            "Approve Alice on Phone",
            "Approve this Chat guest request.",
            "home: approve Chat guest",
        ),
        extra_actions: vec![inbox_action(
            "room-deny-request:chat-guest-1",
            "Deny Alice on Phone",
            "Deny this Chat guest request.",
            "home: deny Chat guest",
        )],
        primary_action_id: "room-approve-request:chat-guest-1",
    }
}

fn generic_capsule_inbox_fixture() -> InboxFixtureScenario {
    InboxFixtureScenario {
        name: "generic capsule notification",
        entry: inbox_entry(
            "capsule-documents-ready",
            "documents",
            "capsule_notification",
            "Documents finished importing",
            "Documents has a completed import ready to review.",
            "attention",
            ("documents", "open-gui:documents"),
        ),
        primary_action: inbox_action(
            "open-gui:documents",
            "Open Documents",
            "Open Documents to review the completed import.",
            "home: open Documents",
        ),
        extra_actions: Vec::new(),
        primary_action_id: "open-gui:documents",
    }
}

fn inbox_fixture_scenarios() -> Vec<InboxFixtureScenario> {
    vec![
        wallet_signing_inbox_fixture(),
        inspect_approval_inbox_fixture(),
        people_request_inbox_fixture(),
        chat_guest_inbox_fixture(),
        generic_capsule_inbox_fixture(),
    ]
}

#[test]
fn home_actions_stay_task_focused() {
    let snapshot = sample_snapshot();
    let ids: Vec<&str> = home_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat"]);
}

#[test]
fn quit_action_switches_browser_pty_back_to_home_gui_only() {
    let mut snapshot = sample_snapshot();
    assert_eq!(quit_action(&snapshot), "shell-switch:home-gui");

    snapshot.session.mode = "native_terminal".to_string();
    assert_eq!(quit_action(&snapshot), "quit");
}

#[test]
fn ignores_startup_enter_on_default_home_selection() {
    let state = TuiState::default();
    let now = Instant::now();
    assert!(matches!(
        startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
        HomeLaunchDecision::Defer(_)
    ));
}

#[test]
fn ignores_duplicate_startup_enter_inside_settle_window() {
    let state = TuiState::default();
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(
            &state,
            UiKey::Enter,
            true,
            Some(now + STARTUP_ENTER_SETTLE_WINDOW),
            now
        ),
        HomeLaunchDecision::IgnoreDuplicate
    );
}

#[test]
fn allows_enter_after_settle_window() {
    let state = TuiState::default();
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(
            &state,
            UiKey::Enter,
            true,
            Some(now),
            now + STARTUP_ENTER_SETTLE_WINDOW
        ),
        HomeLaunchDecision::Allow
    );
}

#[test]
fn does_not_defer_enter_after_default_home_launch_is_armed_and_ready() {
    let state = TuiState::default();
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(
            &state,
            UiKey::Enter,
            true,
            Some(now),
            now + STARTUP_ENTER_SETTLE_WINDOW
        ),
        HomeLaunchDecision::Allow
    );
}

#[test]
fn does_not_defer_non_enter_keys() {
    let state = TuiState::default();
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(&state, UiKey::Digit(1), false, None, now),
        HomeLaunchDecision::NotApplicable
    );
}

#[test]
fn does_not_defer_enter_off_default_home_selection() {
    let state = TuiState {
        home_index: 1,
        ..TuiState::default()
    };
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
        HomeLaunchDecision::NotApplicable
    );
}

#[test]
fn does_not_defer_enter_when_help_is_open() {
    let state = TuiState {
        show_help: true,
        ..TuiState::default()
    };
    let now = Instant::now();
    assert_eq!(
        startup_home_enter_decision(&state, UiKey::Enter, false, None, now),
        HomeLaunchDecision::NotApplicable
    );
}

#[test]
fn apps_use_runtime_targets_but_only_cli_native_or_explicit_open_entries_are_actionable() {
    let snapshot = sample_snapshot();
    let entries = app_entries(&snapshot);
    let labels = entries
        .iter()
        .map(|entry| entry.label.as_str())
        .collect::<Vec<_>>();

    assert!(entries.iter().all(|entry| !entry.is_control));
    for expected in ["Browser", "Chat", "GBA Emulator", "uCity", "People"] {
        assert!(
            labels.contains(&expected),
            "Apps should show Runtime Home target read-only when needed: {expected}",
        );
    }
    let rendered_list = render_app_list(&entries, usize::MAX).join("\n");
    assert!(rendered_list.contains("uCity -> GBA Emulator"));
    let mut saw_library = false;
    for entry in &entries {
        if entry.category == "Library" {
            saw_library = true;
        }
        assert!(
            !(saw_library && entry.category == "Apps"),
            "Apps section should not repeat after Library: {}",
            entry.label
        );
    }
    assert!(!labels.contains(&"mystery-capsule"));
    assert!(!labels.contains(&"Home CLI"));
    assert!(!labels.contains(&"Full-screen Chat"));
    assert!(!labels.contains(&"Shared Conversation"));
    assert!(!labels.contains(&"Shared"));

    let chat_index = labels.iter().position(|label| *label == "Chat").unwrap();
    assert_eq!(
        selected_app_action(&snapshot, chat_index).map(|action| action.id.as_str()),
        Some("chat")
    );
    for read_only in ["Browser", "uCity", "GBA Emulator"] {
        let index = labels.iter().position(|label| *label == read_only).unwrap();
        let entry = &entries[index];
        assert_eq!(entry.action_id, None);
        assert!(selected_app_action(&snapshot, index).is_none());
        assert!(entry.command.contains("Desktop"));
        assert_eq!(
            TuiState {
                tab: Tab::Apps,
                app_index: index,
                ..TuiState::default()
            }
            .activate(&snapshot),
            None
        );
    }
    let gba_index = labels
        .iter()
        .position(|label| *label == "GBA Emulator")
        .unwrap();
    let gba_screen = build_tui_screen(
        &snapshot,
        &TuiState {
            tab: Tab::Apps,
            app_index: gba_index,
            ..TuiState::default()
        },
        120,
        40,
    );
    assert!(gba_screen.contains("Accepts    uCity, .gba files"));
    let ucity_index = labels.iter().position(|label| *label == "uCity").unwrap();
    let ucity_screen = build_tui_screen(
        &snapshot,
        &TuiState {
            tab: Tab::Apps,
            app_index: ucity_index,
            ..TuiState::default()
        },
        120,
        40,
    );
    assert!(ucity_screen.contains("Opens with GBA Emulator"));
    let people_index = labels.iter().position(|label| *label == "People").unwrap();
    let people = &entries[people_index];
    assert_eq!(people.action_id, None);
    assert_eq!(people.command, "Use the People tab in Home CLI.");

    let mut snapshot_with_explicit_open = snapshot.clone();
    snapshot_with_explicit_open.actions.push(ActionInfo {
        id: "open-gui:browser".to_string(),
        label: "Open Browser".to_string(),
        description: "Open Browser from a server-issued Home action.".to_string(),
        command: "home: open Browser".to_string(),
        ready: true,
        reason: None,
    });
    let entries = app_entries(&snapshot_with_explicit_open);
    let browser_index = entries
        .iter()
        .position(|entry| entry.name == "browser")
        .unwrap();
    assert_eq!(entries[browser_index].state, "ready");
    assert_eq!(
        selected_app_action(&snapshot_with_explicit_open, browser_index)
            .map(|action| action.id.as_str()),
        Some("open-gui:browser")
    );
}

#[test]
fn quick_launch_only_includes_working_home_actions() {
    let snapshot = sample_snapshot();
    let ids: Vec<&str> = quick_launch_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat"]);
}

#[test]
fn every_visible_default_menu_item_is_actionable_or_has_next_step() {
    let mut snapshot = sample_snapshot();
    snapshot.site.staged = false;
    snapshot.actions.insert(
        1,
        ActionInfo {
            id: "room-approve".to_string(),
            label: "Approve web guest".to_string(),
            description: "Approve a pending Chat request.".to_string(),
            command: "home approve".to_string(),
            ready: false,
            reason: Some("open Inbox and review the request first".to_string()),
        },
    );

    assert_eq!(
        DEFAULT_TABS,
        &[Tab::Home, Tab::Inbox, Tab::People, Tab::Apps, Tab::System]
    );

    for action_idx in home_action_indices(&snapshot) {
        let action = &snapshot.actions[action_idx];
        assert!(
            action.ready
                || action
                    .reason
                    .as_ref()
                    .is_some_and(|reason| !reason.is_empty()),
            "visible Home action must be ready or explain the next step: {}",
            action.id
        );
    }
    for (index, entry) in app_entries(&snapshot).iter().enumerate() {
        if let Some(action) = selected_app_action(&snapshot, index) {
            assert!(
                action.ready,
                "actionable default app must be immediately ready: {}",
                entry.label
            );
        } else {
            assert!(
                matches!(entry.state.as_str(), "gui-only" | "read-only" | "setup"),
                "read-only app must say why Enter will not launch it: {} [{}]",
                entry.label,
                entry.state
            );
            assert!(
                entry.command.contains("home-gui")
                    || entry.command.contains("explicit approved Home action")
                    || entry.command.contains("Home CLI"),
                "read-only app must point to the approved launch path: {}",
                entry.label
            );
        }
    }

    let screen = build_tui_screen(&snapshot, &TuiState::default(), 120, 40);
    assert!(screen.contains("1 Chat [ready]"));
    assert!(!screen.contains("Approve access [setup]"));
    assert!(screen.contains("Approve access: open Inbox and review the request first"));
    assert!(!screen.contains("elastos site stage"));
    for hidden in [
        "Spaces",
        "Full-screen Chat",
        "Browser [ready]",
        "Updates [ready]",
        "Shared Conversation",
    ] {
        assert!(
            !screen.contains(hidden),
            "default Home CLI leaked hidden/developer item: {hidden}",
        );
    }
}

#[test]
fn blocked_mywebsite_is_not_a_default_home_action() {
    let mut snapshot = sample_snapshot();
    snapshot.site.staged = false;
    if let Some(action) = snapshot
        .actions
        .iter_mut()
        .find(|action| action.id == "site-local")
    {
        action.ready = false;
        action.reason = Some("stage a site first".to_string());
    }
    let ids: Vec<&str> = home_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat"]);
    assert!(!alerts_lines(&snapshot, 120, None)
        .iter()
        .any(|line| line.contains("elastos site stage")));
    assert!(mywebsite_task_lines(&snapshot)
        .join("\n")
        .contains("Stage    mywebsite stage <dir>"));
}

#[test]
fn shared_catalog_entries_stay_out_of_default_home_actions() {
    let mut snapshot = sample_snapshot();
    snapshot.shares.channel_count = 1;
    snapshot.shares.active_count = 1;

    let ids: Vec<&str> = home_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat"]);
}

#[test]
fn pending_browser_access_surfaces_approval_actions_on_home() {
    let mut snapshot = sample_snapshot();
    snapshot.room.pending_count = 1;
    snapshot.notifications.entries = vec![NotificationEntryStatus {
        id: "room-pair-request:req-1".to_string(),
        source_app: "chat-room".to_string(),
        kind: "room_pair_request".to_string(),
        title: "Alice wants to join Chat".to_string(),
        body: "Alice on Phone wants to join Chat.".to_string(),
        action_ref: Some(NotificationActionRefStatus {
            app: "chat-room".to_string(),
            action_id: "room-approve-request:req-1".to_string(),
        }),
        read: false,
        severity: "attention".to_string(),
    }];
    snapshot.notifications.unread_count = 1;
    snapshot.notifications.attention_count = 1;
    snapshot.actions.insert(
        1,
        ActionInfo {
            id: "room-approve".to_string(),
            label: "Approve web guest".to_string(),
            description: String::new(),
            command: "home approve".to_string(),
            ready: true,
            reason: None,
        },
    );
    snapshot.actions.insert(
        2,
        ActionInfo {
            id: "room-deny".to_string(),
            label: "Deny web guest".to_string(),
            description: String::new(),
            command: "home deny".to_string(),
            ready: true,
            reason: None,
        },
    );

    let ids: Vec<&str> = home_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat", "room-approve", "room-deny"]);
    let alerts = alerts_lines(&snapshot, 120, None);
    assert!(alerts
        .iter()
        .any(|line| line.contains("Alice on Phone wants to join Chat.")));
}

#[test]
fn active_browser_sessions_surface_disconnect_action_on_home() {
    let mut snapshot = sample_snapshot();
    snapshot.room.active_session_count = 2;
    snapshot.room.active_participants = vec![
        RoomParticipantStatus {
            display_name: "Alice".to_string(),
            device_label: "Phone".to_string(),
        },
        RoomParticipantStatus {
            display_name: "Bob".to_string(),
            device_label: "Safari".to_string(),
        },
    ];
    snapshot.actions.insert(
        1,
        ActionInfo {
            id: "room-revoke-all".to_string(),
            label: "Disconnect browsers".to_string(),
            description: String::new(),
            command: "home revoke".to_string(),
            ready: true,
            reason: None,
        },
    );

    let ids: Vec<&str> = home_action_indices(&snapshot)
        .into_iter()
        .map(|idx| snapshot.actions[idx].id.as_str())
        .collect();
    assert_eq!(ids, vec!["chat", "room-revoke-all"]);

    let alerts = alerts_lines(&snapshot, 120, None);
    assert!(alerts
        .iter()
        .any(|line| line.contains("2 active web guest session(s): Alice on Phone, Bob on Safari")));
}

#[test]
fn people_tab_uses_people_model_and_keeps_transport_in_debug() {
    let mut snapshot = sample_snapshot();
    snapshot.room.pending_count = 1;
    snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
        display_name: "Alice".to_string(),
        device_label: "Phone".to_string(),
    }];
    snapshot.room.active_session_count = 1;
    snapshot.room.active_sessions = vec![RoomSessionStatus {
        display_name: "Bob".to_string(),
        device_label: "Safari".to_string(),
    }];
    snapshot.actions.push(ActionInfo {
        id: "room-approve-request:req-1".to_string(),
        label: "Approve Alice on Phone".to_string(),
        description: String::new(),
        command: "home approve specific".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-deny-request:req-1".to_string(),
        label: "Deny Alice on Phone".to_string(),
        description: String::new(),
        command: "home deny specific".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-revoke-session:tok-1".to_string(),
        label: "Disconnect Bob on Safari".to_string(),
        description: String::new(),
        command: "home disconnect specific".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.people = PeopleStatus {
        schema: "elastos.people.contacts/v1".to_string(),
        contact_count: 1,
        contacts: vec![PeopleContactStatus {
            contact_id: "contact-alice".to_string(),
            display_name: "Alice".to_string(),
            handle: Some("@alice".to_string()),
            relationship: "connected".to_string(),
            route: "/apps/chat-room/".to_string(),
            can_message: true,
            device_label: Some("peer-alice".to_string()),
            profile_card: None,
            last_seen_at: Some(10),
        }],
        service_offer_count: 0,
        discovery: PeopleDiscoveryStatus {
            schema: "elastos.people.discovery/v1".to_string(),
            enabled: true,
            remaining_seconds: Some(120),
            visibility: "visible".to_string(),
            status: "ready".to_string(),
            status_message: "Discovery is ready.".to_string(),
            topic: "__elastos_internal/people-discovery-v1".to_string(),
            local_peer_id: Some("peer-local".to_string()),
            discovered_peers: vec![PeopleDiscoveryPeerStatus {
                peer_id: "peer-bob".to_string(),
                did: Some("did:key:bob".to_string()),
                display_name: "Bob".to_string(),
                handle: Some("@bob".to_string()),
                last_seen_at: 20,
                status: "visible".to_string(),
            }],
            requests: vec![PeopleDiscoveryRequestStatus {
                request_id: "request-carol".to_string(),
                peer_id: "peer-carol".to_string(),
                did: Some("did:key:carol".to_string()),
                display_name: "Carol".to_string(),
                handle: Some("@carol".to_string()),
                created_at: 30,
                status: "incoming".to_string(),
                invite_id: None,
            }],
            next_refresh_after_ms: None,
        },
    };

    let ids: Vec<String> = people_actions(&snapshot)
        .into_iter()
        .map(|action| action.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            "people-discovery-disable",
            "people-discovery-refresh",
            "people-accept-request:request-carol",
            "people-request-peer:peer-bob",
            "people-message:contact-alice",
            "people-remove-contact:contact-alice",
        ]
    );

    let mut buf = String::new();
    render_people_tab(&mut buf, &snapshot, &TuiState::default(), 120);
    assert!(buf.contains("My Profile"));
    assert!(buf.contains("People"));
    assert!(buf.contains("Discovery"));
    assert!(buf.contains("Add People"));
    assert!(buf.contains("Requests"));
    assert!(buf.contains("Alice"));
    assert!(buf.contains("Bob"));
    assert!(buf.contains("Carol"));
    assert!(buf.contains("Message  people message contact-alice"));
    assert!(buf.contains("Remove   people remove contact-alice"));
    assert!(buf.contains("Add: people request peer-bob"));
    assert!(buf.contains("Accept: people accept request-carol"));
    assert!(buf.contains("Chat with Alice"));
    assert!(!buf.contains("direct contact threads are not available yet"));
    assert!(buf.contains("Remove Alice"));
    for hidden in [
        "Conversation",
        "Request    Alice on Phone",
        "Web guest  Bob on Safari",
        "Ticket",
        "Carrier",
        "Roots",
        "RoomGuests",
        "Model",
        "Source",
        "Services",
        "__elastos_internal",
        "peer-local",
    ] {
        assert!(
            !buf.contains(hidden),
            "normal People view leaked debug detail: {hidden}"
        );
    }

    let debug = people_debug_lines(&snapshot).join("\n");
    assert!(debug.contains("Ticket"));
    assert!(debug.contains("RoomGuests"));
    assert!(debug.contains("RoomReqs"));
    assert!(debug.contains("RoomWeb"));
    assert!(debug.contains("ContactsSchema"));
    assert!(debug.contains("DiscoverySchema"));
    assert!(debug.contains("Source"));
    assert!(debug.contains("Services"));
    assert!(debug.contains("LocalPeer"));
    assert!(debug.contains("__elastos_internal/people-discovery-v1"));
}

#[test]
fn room_control_details_remain_available_to_debug_helpers() {
    let mut snapshot = sample_snapshot();
    snapshot.room.title = "Room".to_string();
    snapshot.room.room_slug = "chat-room".to_string();
    snapshot.room.local_runtime_role = Some("owner".to_string());
    snapshot.room.owner_did = Some("did:key:z6Mkowner".to_string());
    snapshot.room.current_key_epoch = 3;
    snapshot.room.admin_count = 1;
    snapshot.room.member_count = 4;
    snapshot.room.active_member_count = 2;
    snapshot.room.pending_count = 1;
    snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
        display_name: "Alice".to_string(),
        device_label: "Phone".to_string(),
    }];
    snapshot.room.pending_invite_count = 1;
    snapshot.room.pending_invites = vec![RoomInviteStatus {
        invited_did: "did:key:z6invitee".to_string(),
    }];
    snapshot.room.active_session_count = 1;
    snapshot.room.active_sessions = vec![RoomSessionStatus {
        display_name: "Bob".to_string(),
        device_label: "Safari".to_string(),
    }];
    snapshot.room.members = vec![
        RoomMemberStatus {
            member_did: "did:key:z6Mkowner".to_string(),
            role: "owner".to_string(),
        },
        RoomMemberStatus {
            member_did: "did:key:z6member".to_string(),
            role: "member".to_string(),
        },
    ];

    let entry = chat_room_app_entry(&snapshot).expect("room entry missing");

    let detail_lines = chat_room_app_detail_lines(&snapshot, &entry, 120);
    assert!(detail_lines
        .iter()
        .any(|line| line.contains("Public URL https://elastos.elacitylabs.com/apps/chat-room/")));
    assert!(detail_lines
        .iter()
        .any(|line| line.contains("Invite     did:key:z6invitee pending")));
    assert!(detail_lines
        .iter()
        .any(|line| line.contains("Person     did:key:z6member (trusted participant)")));
}

#[test]
fn targeted_room_controls_do_not_appear_as_default_apps() {
    let mut snapshot = sample_snapshot();
    snapshot.room.allow_guest_invites = true;
    snapshot.room.allow_member_invites = false;
    snapshot.room.pending_count = 1;
    snapshot.room.pending_requests = vec![RoomPendingRequestStatus {
        display_name: "Alice".to_string(),
        device_label: "Phone".to_string(),
    }];
    snapshot.room.pending_invite_count = 1;
    snapshot.room.pending_invites = vec![RoomInviteStatus {
        invited_did: "did:key:z6member".to_string(),
    }];
    snapshot.room.active_session_count = 1;
    snapshot.room.active_sessions = vec![RoomSessionStatus {
        display_name: "Bob".to_string(),
        device_label: "Safari".to_string(),
    }];
    snapshot.room.members.push(RoomMemberStatus {
        member_did: "did:key:z6member".to_string(),
        role: "member".to_string(),
    });
    snapshot.actions.push(ActionInfo {
        id: "room-policy-toggle-guests".to_string(),
        label: "Close public join requests".to_string(),
        description: "Stop new web guests from requesting access through the public Chat link."
            .to_string(),
        command: "home toggle public join requests".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-policy-toggle-members".to_string(),
        label: "Open ElastOS user invites".to_string(),
        description: "Allow new invites for trusted ElastOS users.".to_string(),
        command: "home toggle ElastOS user invites".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-revoke-invite:inv-1".to_string(),
        label: "Revoke invite for did:key:z6member".to_string(),
        description: "Cancel this pending ElastOS user invite".to_string(),
        command: "home revoke invite".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-remove-member:did:key:z6member".to_string(),
        label: "Remove did:key:z6member".to_string(),
        description: "Remove this trusted participant".to_string(),
        command: "home remove member".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-approve-request:req-1".to_string(),
        label: "Approve Alice on Phone".to_string(),
        description: "Approve this web guest".to_string(),
        command: "home approve specific".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-deny-request:req-1".to_string(),
        label: "Deny Alice on Phone".to_string(),
        description: "Deny this browser".to_string(),
        command: "home deny specific".to_string(),
        ready: true,
        reason: None,
    });
    snapshot.actions.push(ActionInfo {
        id: "room-revoke-session:tok-1".to_string(),
        label: "Disconnect Bob on Safari".to_string(),
        description: "Disconnect this browser".to_string(),
        command: "home disconnect specific".to_string(),
        ready: true,
        reason: None,
    });

    let entries = app_entries(&snapshot);
    assert!(entries.iter().any(|entry| entry.label == "Chat"));
    for hidden_control in [
        "Close public join requests",
        "Open ElastOS user invites",
        "Approve Alice on Phone",
        "Disconnect Bob on Safari",
    ] {
        assert!(
            !entries.iter().any(|entry| entry.label == hidden_control),
            "targeted room control leaked into default Apps: {hidden_control}",
        );
    }

    let controls = room_control_entries(&snapshot);
    assert!(controls
        .iter()
        .any(|entry| entry.label == "Close public join requests" && entry.is_control));
    assert!(controls
        .iter()
        .any(|entry| entry.label == "Approve Alice on Phone" && entry.is_control));
    assert!(controls
        .iter()
        .any(|entry| entry.label == "Disconnect Bob on Safari" && entry.is_control));
}

#[test]
fn inbox_tab_surfaces_notifications_and_resolves_actions() {
    let mut snapshot = sample_snapshot();
    snapshot.notifications.unread_count = 1;
    snapshot.notifications.attention_count = 1;
    snapshot.notifications.entries = vec![NotificationEntryStatus {
        id: "room-pair-request:req-1".to_string(),
        source_app: "chat-room".to_string(),
        kind: "room_pair_request".to_string(),
        title: "Alice wants to join Chat".to_string(),
        body: "Alice on Phone wants to join Chat.".to_string(),
        action_ref: Some(NotificationActionRefStatus {
            app: "chat-room".to_string(),
            action_id: "room-approve-request:req-1".to_string(),
        }),
        read: false,
        severity: "attention".to_string(),
    }];
    snapshot.actions.push(ActionInfo {
        id: "room-approve-request:req-1".to_string(),
        label: "Approve Alice on Phone".to_string(),
        description: "Approve this browser".to_string(),
        command: "home approve specific".to_string(),
        ready: true,
        reason: None,
    });

    let state = TuiState {
        tab: Tab::Inbox,
        ..TuiState::default()
    };

    let mut buf = String::new();
    render_inbox_tab(&mut buf, &snapshot, &state, 120);
    assert!(buf.contains("Alice wants to join Chat"));
    assert!(buf.contains("Approve Alice on Phone"));
    assert_eq!(
        state.activate(&snapshot),
        Some("room-approve-request:req-1".to_string())
    );
}

#[test]
fn inbox_fixture_scenarios_resolve_actions_and_mark_dismiss_intents() {
    for scenario in inbox_fixture_scenarios() {
        let mut snapshot = sample_snapshot();
        scenario.apply(&mut snapshot);

        let state = TuiState {
            tab: Tab::Inbox,
            ..TuiState::default()
        };

        let mut buf = String::new();
        render_inbox_tab(&mut buf, &snapshot, &state, 200);
        assert!(
            buf.contains(&scenario.entry.title),
            "Inbox did not render {} scenario title",
            scenario.name
        );
        assert!(
            buf.contains(&scenario.entry.body),
            "Inbox did not render {} scenario body",
            scenario.name
        );
        assert_eq!(
            selected_notification_read_action(&snapshot, 0),
            Some(format!("notification-read:{}", scenario.entry.id)),
            "{} mark-read intent drifted",
            scenario.name
        );
        assert_eq!(
            selected_notification_dismiss_action(&snapshot, 0),
            Some(format!("notification-dismiss:{}", scenario.entry.id)),
            "{} dismiss intent drifted",
            scenario.name
        );
        assert_eq!(
            state.activate(&snapshot),
            Some(scenario.primary_action_id.to_string()),
            "{} primary action drifted",
            scenario.name
        );
        assert!(
            buf.contains("Enter      run this inbox action and return here"),
            "{} Inbox selected action did not promise return behavior",
            scenario.name
        );
    }
}

#[test]
fn inbox_chat_guest_fixture_exposes_approve_and_deny_paths() {
    let scenario = chat_guest_inbox_fixture();
    let mut snapshot = sample_snapshot();
    scenario.apply(&mut snapshot);

    let approve = selected_notification_action(&snapshot, 0).expect("approve action");
    assert_eq!(approve.id, "room-approve-request:chat-guest-1");
    assert!(approve.ready);

    let deny = action_by_id(&snapshot, "room-deny-request:chat-guest-1").expect("deny action");
    assert_eq!(deny.label, "Deny Alice on Phone");
    assert!(deny.ready);

    let state = TuiState {
        tab: Tab::Inbox,
        ..TuiState::default()
    };
    assert_eq!(
        state.activate(&snapshot),
        Some("room-approve-request:chat-guest-1".to_string())
    );
    assert_eq!(
        selected_notification_dismiss_action(&snapshot, 0),
        Some("notification-dismiss:room-access-request:chat-guest-1".to_string())
    );
}

#[test]
fn inbox_selected_index_controls_mark_dismiss_and_open() {
    let wallet = wallet_signing_inbox_fixture();
    let generic = generic_capsule_inbox_fixture();
    let mut snapshot = sample_snapshot();
    wallet.apply(&mut snapshot);
    snapshot.notifications.entries.push(generic.entry.clone());
    snapshot.actions.push(generic.primary_action.clone());
    snapshot.notifications.unread_count = 2;
    snapshot.notifications.attention_count = 2;

    let state = TuiState {
        tab: Tab::Inbox,
        inbox_index: 1,
        ..TuiState::default()
    };

    assert_eq!(
        selected_notification_read_action(&snapshot, state.inbox_index),
        Some("notification-read:capsule-documents-ready".to_string())
    );
    assert_eq!(
        selected_notification_dismiss_action(&snapshot, state.inbox_index),
        Some("notification-dismiss:capsule-documents-ready".to_string())
    );
    assert_eq!(
        state.activate(&snapshot),
        Some("open-gui:documents".to_string())
    );
}

#[test]
fn tabs_keep_people_between_inbox_and_apps() {
    let mut state = TuiState::default();
    state.next_tab();
    assert_eq!(state.tab, Tab::Inbox);
    state.next_tab();
    assert_eq!(state.tab, Tab::People);
    state.next_tab();
    assert_eq!(state.tab, Tab::Apps);
    state.prev_tab();
    assert_eq!(state.tab, Tab::People);
    state.prev_tab();
    assert_eq!(state.tab, Tab::Inbox);
}

#[test]
fn shared_catalog_entries_stay_out_of_default_apps() {
    let mut snapshot = sample_snapshot();
    assert!(!app_entries(&snapshot)
        .iter()
        .any(|entry| entry.label == "Shared"));

    snapshot.shares.channel_count = 1;
    snapshot.shares.active_count = 1;

    assert!(!app_entries(&snapshot)
        .iter()
        .any(|entry| entry.label == "Shared"));
}

#[test]
fn blocked_home_action_becomes_next_step_not_primary_action() {
    let mut snapshot = sample_snapshot();
    snapshot.actions.insert(
        1,
        ActionInfo {
            id: "room-approve".to_string(),
            label: "Approve web guest".to_string(),
            description: "Approve a pending request.".to_string(),
            command: "home approve".to_string(),
            ready: false,
            reason: Some("open Inbox and review the request first".to_string()),
        },
    );
    let state = TuiState {
        tab: Tab::Home,
        ..TuiState::default()
    };

    assert_eq!(home_action_indices(&snapshot).len(), 1);
    assert_eq!(state.activate(&snapshot), Some("chat".to_string()));
    assert!(
        !render_home_actions(&snapshot, &home_action_indices(&snapshot), 1, 120)
            .join("\n")
            .contains("setup: open Inbox and review the request first")
    );
    assert_eq!(
        home_next_step(&snapshot, None).as_deref(),
        Some("Approve access: open Inbox and review the request first")
    );
}

#[test]
fn visible_width_ignores_ansi_escape_sequences() {
    assert_eq!(visible_text_width("\x1b[30;46;1m Home \x1b[0m"), 6);
}

#[test]
fn parse_escape_sequence_bytes_handles_partial_and_arrow_sequences() {
    assert_eq!(parse_escape_sequence_bytes(&[]), UiKey::None);
    assert_eq!(parse_escape_sequence_bytes(b"["), UiKey::None);
    assert_eq!(parse_escape_sequence_bytes(b"[A"), UiKey::Up);
    assert_eq!(parse_escape_sequence_bytes(b"[B"), UiKey::Down);
    assert_eq!(parse_escape_sequence_bytes(b"[C"), UiKey::Right);
    assert_eq!(parse_escape_sequence_bytes(b"[D"), UiKey::Left);
    assert_eq!(parse_escape_sequence_bytes(b"OA"), UiKey::Up);
    assert_eq!(parse_escape_sequence_bytes(b"[1;5A"), UiKey::Up);
    assert_eq!(parse_escape_sequence_bytes(b"[1;2D"), UiKey::Left);
    assert_eq!(parse_escape_sequence_bytes(b"[Z"), UiKey::Left);
    assert_eq!(parse_escape_sequence_bytes(b"[1;2Z"), UiKey::Left);
}

#[test]
fn standalone_escape_exits_tui() {
    assert_eq!(parse_escape_sequence_bytes(&[]), UiKey::None);
    assert_eq!(escape_sequence_key(&[]), UiKey::Quit);
    assert_eq!(escape_sequence_key(b"[Z"), UiKey::Left);
}

#[test]
fn parse_escape_sequence_bytes_handles_sgr_mouse_sequences() {
    assert_eq!(
        parse_escape_sequence_bytes(b"[<64;10;8M"),
        UiKey::Mouse(MouseEvent {
            button: 64,
            x: 10,
            y: 8,
            released: false,
        })
    );
    assert_eq!(
        parse_escape_sequence_bytes(b"[<0;44;4M"),
        UiKey::Mouse(MouseEvent {
            button: 0,
            x: 44,
            y: 4,
            released: false,
        })
    );
    let long_coordinate = b"[<0;129;43M";
    assert!(
        ESCAPE_SEQUENCE_MAX_BYTES >= long_coordinate.len(),
        "mouse coordinates must fit in the escape-sequence read buffer"
    );
    assert_eq!(
        parse_escape_sequence_bytes(long_coordinate),
        UiKey::Mouse(MouseEvent {
            button: 0,
            x: 129,
            y: 43,
            released: false,
        })
    );
    assert_eq!(
        parse_escape_sequence_bytes(b"[<0;44;4m"),
        UiKey::Mouse(MouseEvent {
            button: 0,
            x: 44,
            y: 4,
            released: true,
        })
    );
}

#[test]
fn parse_escape_sequence_bytes_handles_legacy_mouse_sequences() {
    let click = [b'[', b'M', 32, 44, 35];
    assert_eq!(
        parse_escape_sequence_bytes(&click),
        UiKey::Mouse(MouseEvent {
            button: 0,
            x: 12,
            y: 3,
            released: false,
        })
    );

    let release = [b'[', b'M', 35, 44, 35];
    assert_eq!(
        parse_escape_sequence_bytes(&release),
        UiKey::Mouse(MouseEvent {
            button: 3,
            x: 12,
            y: 3,
            released: true,
        })
    );
}

#[test]
fn escape_sequence_completion_waits_for_legacy_mouse_coordinates() {
    assert!(!is_escape_sequence_complete(b"["));
    assert!(!is_escape_sequence_complete(b"[M"));
    assert!(!is_escape_sequence_complete(b"[M  "));
    assert!(is_escape_sequence_complete(b"[M  #"));
    assert!(is_escape_sequence_complete(b"[<0;12;3M"));
    assert!(is_escape_sequence_complete(b"[A"));
}

#[test]
fn mouse_clicks_use_the_rendered_tab_row() {
    let snapshot = sample_snapshot();
    let mut state = TuiState::default();
    assert!(state.handle_mouse(
        MouseEvent {
            button: 0,
            x: 45,
            y: TUI_TAB_ROW,
            released: false,
        },
        120,
        &snapshot,
    ));
    assert_eq!(state.tab, Tab::Inbox);

    let mut state = TuiState::default();
    assert!(!state.handle_mouse(
        MouseEvent {
            button: 0,
            x: 45,
            y: TUI_TAB_ROW + 1,
            released: false,
        },
        120,
        &snapshot,
    ));
    assert_eq!(state.tab, Tab::Home);
}

#[test]
fn home_screen_stays_compact() {
    let snapshot = sample_snapshot();
    let screen = build_tui_screen(&snapshot, &TuiState::default(), 100, 32);
    assert!(!screen.contains("Start Here"));
    assert!(!screen.contains("-- Status --"));
    assert!(screen.starts_with("\x1b[H\x1b[J"));
    assert!(!screen.ends_with("\r\n"));
    assert!(screen.contains("1 Chat [ready]"));
    assert!(screen.contains("Next"));
    assert!(!screen.contains("MyWebSite [ready]"));
    assert!(!screen.contains("MyWebSite is empty."));
    assert!(!screen.contains("Updates [ready]"));
    assert!(!screen.contains("Shared [ready]"));
    assert!(screen.contains("Up/Down select"));
    assert!(screen.contains("q/Esc Desktop"));
    assert!(screen.contains("? help"));
    assert!(!screen.contains("opens Browser"));
    assert!(!screen.contains("hjkl"));
}

#[test]
fn tui_tabs_replace_redundant_banner() {
    let snapshot = sample_snapshot();
    let screen = build_tui_screen(
        &snapshot,
        &TuiState {
            tab: Tab::Apps,
            ..TuiState::default()
        },
        100,
        32,
    );

    assert!(!screen.contains("ElastOS Home"));
    assert!(!screen.contains("ElastOS Apps"));
    assert!(screen.contains("\x1b[30;46;1m Apps \x1b[0m"));
}

#[test]
fn tui_help_matches_keyboard_contract() {
    let snapshot = sample_snapshot();
    let controls = command_contract().controls;
    let screen = build_tui_screen(
        &snapshot,
        &TuiState {
            show_help: true,
            ..TuiState::default()
        },
        120,
        60,
    );

    let plain = screen.replace("\r\n", " ");
    for control in &controls {
        assert!(
            plain.contains(&control.key),
            "TUI help missing command-contract control key: {}",
            control.key
        );
        assert!(
            plain.contains(&control.description),
            "TUI help missing command-contract control description for {}",
            control.key
        );
    }
    assert_eq!(tui_control_help_lines().len(), controls.len());
    assert!(screen.contains("? close help"));
    assert!(!screen.contains("? help"));
    assert!(!screen.contains("hjkl"));
}

#[test]
fn default_tabs_stay_within_viewport_so_tabs_do_not_scroll_away() {
    let snapshot = sample_snapshot();
    let rows = 20usize;
    let screen = build_tui_screen(
        &snapshot,
        &TuiState {
            tab: Tab::Apps,
            ..TuiState::default()
        },
        100,
        rows,
    );
    let rendered_rows = screen.split("\r\n").count();
    let first_line = screen
        .strip_prefix("\x1b[H\x1b[J")
        .unwrap_or(&screen)
        .split("\r\n")
        .next()
        .unwrap_or_default();

    assert!(
        rendered_rows <= rows,
        "Apps tab rendered {rendered_rows} rows into a {rows}-row terminal"
    );
    assert!(first_line.contains("\x1b[30;46;1m Apps \x1b[0m"));
    assert!(!first_line.contains("alex"));
    assert!(!first_line.contains("Home CLI"));
}

#[test]
fn every_tui_page_keeps_tabs_inside_viewport() {
    let snapshot = sample_snapshot();
    let rows = 20usize;
    let states = [
        TuiState {
            tab: Tab::Home,
            ..TuiState::default()
        },
        TuiState {
            tab: Tab::Inbox,
            ..TuiState::default()
        },
        TuiState {
            tab: Tab::People,
            ..TuiState::default()
        },
        TuiState {
            tab: Tab::Apps,
            ..TuiState::default()
        },
        TuiState {
            tab: Tab::System,
            ..TuiState::default()
        },
    ];

    for state in states {
        let screen = build_tui_screen(&snapshot, &state, 100, rows);
        let rendered_rows = screen.split("\r\n").count();
        let first_line = screen
            .strip_prefix("\x1b[H\x1b[J")
            .unwrap_or(&screen)
            .split("\r\n")
            .next()
            .unwrap_or_default();

        assert!(
            rendered_rows <= rows,
            "TUI page {:?} rendered {rendered_rows} rows into a {rows}-row terminal",
            state.tab
        );
        assert!(
            first_line.contains("Home")
                && first_line.contains("Inbox")
                && first_line.contains("People")
                && first_line.contains("Apps")
                && first_line.contains("System")
                && !first_line.contains("Spaces"),
            "TUI page {:?} lost its tab row: {first_line:?}",
            state.tab
        );
    }
}

#[test]
fn tui_starts_at_tab_row_without_summary_header() {
    let snapshot = sample_snapshot();
    let screen = build_tui_screen(&snapshot, &TuiState::default(), 100, 20);
    let first_line = screen
        .strip_prefix("\x1b[H\x1b[J")
        .unwrap_or(&screen)
        .split("\r\n")
        .next()
        .unwrap_or_default();

    assert!(first_line.contains("\x1b[30;46;1m Home \x1b[0m"));
    assert!(!first_line.contains("alex"));
    assert!(!first_line.contains("Home CLI"));
    assert!(!first_line.contains("identity ready"));
    assert!(!first_line.contains("bootstrap ready"));
    assert!(!first_line.contains("site empty"));
}

#[test]
fn tui_lines_do_not_trigger_terminal_autowrap() {
    let snapshot = sample_snapshot();
    let cols = 100usize;
    let states = [
        TuiState {
            tab: Tab::Home,
            ..TuiState::default()
        },
        TuiState {
            tab: Tab::Apps,
            ..TuiState::default()
        },
    ];

    for state in states {
        let screen = build_tui_screen(&snapshot, &state, cols, 24);
        for line in screen
            .strip_prefix("\x1b[H\x1b[J")
            .unwrap_or(&screen)
            .split("\r\n")
        {
            assert!(
                visible_text_width(line) < cols,
                "TUI page {:?} emitted a full-width line that can trigger xterm autowrap: {:?}",
                state.tab,
                line
            );
        }
    }
}

#[test]
fn mywebsite_notice_surfaces_only_after_explicit_home_action() {
    let snapshot = sample_snapshot();
    let notice =
        "MyWebSite is empty. Stage a local directory with `elastos site stage <dir>`. Then reopen MyWebSite from Home to preview or go public.";
    let screen = build_tui_screen(
        &snapshot,
        &TuiState {
            notice: Some(notice.to_string()),
            ..TuiState::default()
        },
        100,
        32,
    );

    assert_eq!(
        alerts_lines(&snapshot, 100, Some(notice))
            .join("\n")
            .matches("MyWebSite is empty.")
            .count(),
        1
    );
    assert!(screen.contains("MyWebSite is empty."));
    assert!(screen.contains("Needs attention"));
}

#[test]
fn staged_site_summary_and_banner_stay_honest() {
    let mut snapshot = sample_snapshot();
    snapshot.site.local_url = None;
    snapshot.site.active_release = None;
    if let Some(action) = snapshot
        .actions
        .iter_mut()
        .find(|action| action.id == "site-local")
    {
        action.ready = false;
        action.reason =
            Some("missing site-provider -- run: elastos setup --profile demo".to_string());
    }

    assert_eq!(
        website_summary(&snapshot),
        "staged at localhost://MyWebSite"
    );
}

#[test]
fn mywebsite_tasks_show_staged_site_and_next_steps() {
    let mut snapshot = sample_snapshot();
    snapshot.site.local_url = None;
    if let Some(action) = snapshot
        .actions
        .iter_mut()
        .find(|action| action.id == "site-local")
    {
        action.ready = false;
        action.reason =
            Some("missing site-provider -- run: elastos setup --profile demo".to_string());
    }

    let screen = mywebsite_task_lines(&snapshot).join("\n");
    assert!(screen.contains("Status   staged at localhost://MyWebSite"));
    assert!(screen.contains("Stage    mywebsite stage <dir>"));
    assert!(screen.contains("Preview  mywebsite preview (blocked"));
    assert!(screen.contains("Open     mywebsite open"));
    assert!(!screen.contains("press Enter"));
}

#[test]
fn system_tab_stays_short_and_actionable() {
    let snapshot = sample_snapshot();
    let lines = compact_system_lines(&snapshot);
    assert!(lines.len() <= 5);
    assert!(lines.iter().any(|line| line.starts_with("Shell")));
    assert!(lines.iter().any(|line| line.starts_with("Switch")));
    assert!(lines.iter().any(|line| line.starts_with("Home")));
    assert!(lines.iter().any(|line| line.starts_with("Updates")));
    assert!(lines
        .iter()
        .any(|line| line.contains("system shell home-gui")));
    assert!(!lines.iter().any(|line| line.starts_with("Session")));
    assert!(!lines.iter().any(|line| line.starts_with("Profile")));
    assert!(!lines.iter().any(|line| line.starts_with("Diagnostics")));
    assert!(!lines.iter().any(|line| line.starts_with("Services")));
    assert!(!lines.iter().any(|line| line.starts_with("Offer")));
    assert!(!lines.iter().any(|line| line.starts_with("Root")));
    assert!(!lines.iter().any(|line| line.starts_with("ElastOS")));
    assert!(!lines.iter().any(|line| line.starts_with("Peers")));
    assert!(!lines.iter().any(|line| line.starts_with("Capsules")));
    assert!(!lines
        .iter()
        .any(|line| line.contains("browser Runtime PTY")));
    assert!(!lines.iter().any(|line| line.contains("managed")));
    assert!(!lines.iter().any(|line| line.contains("did:key")));
    assert!(!lines.iter().any(|line| line.contains("launch-token")));
    assert!(!lines.iter().any(|line| line.starts_with("API")));
}
