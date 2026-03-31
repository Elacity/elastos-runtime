//! ElastOS P2P Chat — mIRC/IRSSI-style TUI client (microVM / native).
//!
//! For the WASM/stdio variant, see `main_stdio.rs`.

#[path = "carrier.rs"]
mod api; // SDK-based Carrier transport
mod app;
mod command;
mod session;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, Message};
use command::Command;

/// DM marker prefix for wire messages: \x01DM:<recipient_pubkey>\x01
const DM_PREFIX: &str = "\x01DM:";
const DM_DELIM: char = '\x01';
const CHAT_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const CHAT_RETURN_HOME_EXIT_CODE: i32 = 73;
const PRESENCE_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
const PRESENCE_RETRY_BACKOFF: Duration = Duration::from_secs(3);

fn forwarded_command_payload() -> Option<String> {
    if let Ok(payload) = std::env::var("ELASTOS_COMMAND") {
        if !payload.is_empty() {
            return Some(payload);
        }
    }

    if let Ok(payload_b64) = std::env::var("ELASTOS_COMMAND_B64") {
        if payload_b64.is_empty() {
            return None;
        }
        use base64::Engine as _;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload_b64) {
            if let Ok(decoded) = String::from_utf8(bytes) {
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }

    None
}

/// CLI arguments (minimal, no clap dependency).
struct Args {
    nick: String,
    /// True if --nick was explicitly provided (takes precedence over identity provider)
    nick_explicit: bool,
    connect: Option<String>,
    /// Don't persist messages to storage
    no_history: bool,
    /// Don't broadcast history to new peers
    no_sync: bool,
    /// Max messages to persist per channel (default: 1000)
    history_limit: usize,
}

fn parse_args() -> Result<Args> {
    let args: Vec<String> = std::env::args().collect();
    let mut nick = String::new();
    let mut nick_explicit = false;
    let mut connect = None;
    let mut no_history = false;
    let mut no_sync = false;
    let mut history_limit = 1000usize;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--nick" | "-n" => {
                i += 1;
                if i < args.len() {
                    nick = args[i].clone();
                    nick_explicit = true;
                }
            }
            "--connect" | "-c" => {
                i += 1;
                if i < args.len() {
                    connect = Some(args[i].clone());
                }
            }
            "--no-history" => {
                no_history = true;
            }
            "--no-sync" => {
                no_sync = true;
            }
            "--history-limit" => {
                i += 1;
                if i < args.len() {
                    history_limit = args[i].parse().unwrap_or(1000);
                }
            }
            _ => {}
        }
        i += 1;
    }

    if nick.is_empty() {
        nick = whoami::fallible::username().unwrap_or_else(|_| "anon".to_string());
    }

    // Supervisor-mode launch payload: shell forwards CLI flags via ELASTOS_COMMAND.
    // The payload may or may not have "command":"chat" — the shell sends just the
    // capsule config, so we accept nick/connect/etc. fields directly.
    if let Some(payload) = forwarded_command_payload() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
            if let Some(n) = v.get("nick").and_then(|n| n.as_str()) {
                if !n.is_empty() {
                    nick = n.to_string();
                    nick_explicit = true;
                }
            }
            if let Some(c) = v.get("connect").and_then(|c| c.as_str()) {
                connect = Some(c.to_string());
            }
            if let Some(b) = v.get("no_history").and_then(|b| b.as_bool()) {
                no_history = b;
            }
            if let Some(b) = v.get("no_sync").and_then(|b| b.as_bool()) {
                no_sync = b;
            }
            if let Some(limit) = v.get("history_limit").and_then(|h| h.as_u64()) {
                history_limit = limit as usize;
            }
        }
    }

    Ok(Args {
        nick,
        nick_explicit,
        connect,
        no_history,
        no_sync,
        history_limit,
    })
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let mut app = App::new(&args.nick);
    app.max_messages = args.history_limit;

    // Check if we have a session token (running under runtime)
    let standalone = std::env::var("ELASTOS_TOKEN")
        .unwrap_or_default()
        .is_empty();

    if !standalone {
        // Acquire capabilities from runtime
        app.set_status("Acquiring capabilities...");
        let mut peer_error: Option<String> = None;
        let mut storage_error: Option<String> = None;
        let mut did_error: Option<String> = None;

        eprintln!("[chat] calling acquire_capability(did)...");
        match session::resolve_identity(&app.nickname, args.nick_explicit) {
            Ok(identity) => {
                eprintln!("[chat] DID capability acquired, calling get_did...");
                app.identity_token = identity.token;
                app.pubkey = identity.did;
                app.nickname = identity.nickname;
                eprintln!("[chat] get_did done");
            }
            Err(e) => {
                did_error = Some(e.to_string());
                eprintln!("Warning: DID capability failed: {}", e);
            }
        }

        match session::acquire_peer_token() {
            Ok(token) => app.peer_token = token,
            Err(e) => {
                peer_error = Some(e.to_string());
                eprintln!("Warning: peer capability failed: {}", e);
            }
        }

        match session::acquire_storage_token() {
            Ok(token) => app.storage_token = token,
            Err(e) => {
                storage_error = Some(e.to_string());
                eprintln!("Warning: storage capability failed: {}", e);
            }
        }

        app.set_status("");

        // Join channels and start P2P via the built-in Carrier peer provider.
        if !app.peer_token.is_empty() {
            app.p2p_connecting = true;

            // Load persisted channels (default: just #general)
            let channels: Vec<String> = if !app.storage_token.is_empty() && !args.no_history {
                api::load_json(&app.storage_token, "chat/channels.json")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| vec!["#general".to_string()])
            } else {
                vec!["#general".to_string()]
            };

            // Load known nicks
            if !app.storage_token.is_empty() && !args.no_history {
                if let Ok(Some(nicks)) = api::load_json::<std::collections::HashMap<String, String>>(
                    &app.storage_token,
                    "chat/known_nicks.json",
                ) {
                    app.known_nicks = nicks;
                }
            }

            // Join each persisted channel, load history, subscribe gossip
            for channel in &channels {
                app.join_channel(channel);

                if !app.storage_token.is_empty() && !args.no_history {
                    if let Ok(history) = api::load_history(&app.storage_token, channel) {
                        if !history.is_empty() {
                            let count = history.len();
                            app.append_messages(channel, history);
                            app.system_message_to(
                                channel,
                                &format!("Loaded {} messages from history", count),
                            );
                        }
                    }
                }

                // Only gossip-subscribe #channels, not @dm channels
                if channel.starts_with('#') {
                    match session::join_topic(&app.peer_token, channel) {
                        Ok(_) => app
                            .system_message_to(channel, &format!("Joined {} P2P channel", channel)),
                        Err(e) if e.to_string().contains("[already_joined]") => app
                            .system_message_to(
                                channel,
                                &format!("Joined {} P2P channel (shared runtime)", channel),
                            ),
                        Err(e) => app.system_message_to(
                            channel,
                            &format!("Failed to join gossip for {}: {}", channel, e),
                        ),
                    }
                    ensure_room_discovery_subscription(&mut app, channel);
                }
            }

            app.active_channel = 0;
            app.p2p_connecting = false; // Gossip joined — show peer count, not "connecting..."
            app.system_message(&format!(
                "Welcome to ElastOS Chat v{}! Type /help for commands.",
                CHAT_VERSION
            ));
            if app.peer_count == 0 {
                app.system_message("Looking for peers... (this can take up to 60s on first run)");
            }
        } else {
            // No peer token - local mode only
            app.join_channel("#general");
            app.system_message(&format!(
                "Welcome to ElastOS Chat v{}! (local mode)",
                CHAT_VERSION
            ));
            if let Some(err) = peer_error {
                app.system_message(&format!("P2P unavailable: {}", err));
                app.system_message(
                    "Retry later with /ticket, /peers, or /connect when the runtime is ready.",
                );
            }
            if let Some(err) = did_error {
                app.system_message(&format!("Identity unavailable: {}", err));
            }
            if let Some(err) = storage_error {
                app.system_message(&format!("History storage unavailable: {}", err));
            }
        }
    } else {
        // Standalone mode
        app.join_channel("#general");
        app.system_message(&format!(
            "Welcome to ElastOS Chat v{}! (standalone mode)",
            CHAT_VERSION
        ));
        app.system_message("Type /help for commands.");
    }

    // Set up terminal — use crossterm for rendering only, raw stdin for input.
    // crossterm's event system has internal escape-sequence buffering that
    // causes delayed input on serial consoles. We read raw bytes via poll()+
    // blocking read() which avoids both crossterm's parser and O_NONBLOCK
    // (which interacts poorly with some serial tty drivers).
    terminal::enable_raw_mode()?;

    // Drain stale input from boot/init
    raw_input::drain_stdin();

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);

    // Serial consoles (VM guests) return (0,0) for TIOCGWINSZ.
    // Fall back to COLUMNS/LINES env vars (set by init from boot args).
    let size = terminal::size().unwrap_or((0, 0));
    let (cols, rows) = if size.0 > 0 && size.1 > 0 {
        size
    } else {
        let c = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(80);
        let r = std::env::var("LINES")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(24);
        (c, r)
    };
    let mut terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, cols, rows)),
        },
    )?;

    // Run the TUI loop
    let result = run_loop(&mut terminal, &mut app, &args);

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if result.is_ok() && app.return_home_requested {
        signal_home_exit();
    }

    result
}

fn request_home_exit(app: &mut App, args: &Args) {
    save_channels(app, args);
    save_known_nicks(app, args);
    app.return_home_requested = true;
    app.should_quit = true;
}

fn signal_home_exit() -> ! {
    if std::env::var("ELASTOS_PARENT_SURFACE").ok().as_deref() == Some("pc2") {
        std::process::exit(0);
    }
    std::process::exit(CHAT_RETURN_HOME_EXIT_CODE);
}

fn ensure_peer_capability(app: &mut App, _args: &Args) -> bool {
    if !app.peer_token.is_empty() {
        return true;
    }

    match session::acquire_peer_token() {
        Ok(token) => {
            app.peer_token = token;
            let channels: Vec<String> = if app.channels.is_empty() {
                vec!["#general".to_string()]
            } else {
                app.channel_names()
                    .into_iter()
                    .filter(|c| c.starts_with('#'))
                    .collect()
            };

            for channel in &channels {
                if !app.channels.iter().any(|c| c.name == *channel) {
                    app.join_channel(channel);
                }
                let _ = session::join_topic(&app.peer_token, channel);
                ensure_room_discovery_subscription(app, channel);
            }

            app.p2p_connecting = true;
            app.system_message("P2P capability acquired.");
            true
        }
        Err(e) => {
            app.system_message(&format!("P2P unavailable: {}", e));
            false
        }
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    args: &Args,
) -> Result<()> {
    let poll_interval = Duration::from_millis(500);
    let mut last_poll = std::time::Instant::now();

    let connect_ticket = args.connect.clone();
    let mut initial_connect = true;

    loop {
        // Render
        terminal.draw(|f| ui::render(f, app))?;

        if app.should_quit {
            break;
        }

        // Connect to peer on first iteration if ticket provided
        if initial_connect {
            initial_connect = false;
            if let Some(ticket) = &connect_ticket {
                connect_to_peer(app, args, ticket);
            }
        }

        // Read raw bytes from stdin and parse into key events.
        // Bypasses crossterm's event system which has buffering issues on
        // serial consoles (crosvm 16550 UART → "press twice" symptom).
        if raw_input::has_pending_input() || raw_input::poll_stdin(50) {
            for key in raw_input::read_keys() {
                let modifiers = if key.ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                };
                handle_key(app, KeyEvent::new(key.code, modifiers), args)?;
            }
        }

        // Poll for new messages periodically (only when connected)
        if !app.peer_token.is_empty() && last_poll.elapsed() >= poll_interval {
            last_poll = std::time::Instant::now();
            poll_messages(app, args);
            poll_peers(app, args);
            poll_presence(app);
        }
    }

    Ok(())
}

/// Handle keyboard input in the main chat view.
fn handle_key(app: &mut App, key: KeyEvent, args: &Args) -> Result<()> {
    // Ctrl+C or Ctrl+Q to quit
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => {
                save_channels(app, args);
                save_known_nicks(app, args);
                app.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('n') => {
                app.next_channel();
                return Ok(());
            }
            KeyCode::Char('p') => {
                app.prev_channel();
                return Ok(());
            }
            KeyCode::Char('b') => {
                app.cursor_left();
                return Ok(());
            }
            KeyCode::Char('f') => {
                app.cursor_right();
                return Ok(());
            }
            KeyCode::Char('u') => {
                app.kill_to_start();
                return Ok(());
            }
            KeyCode::Char('w') => {
                app.delete_word_backward();
                return Ok(());
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Enter => {
            // Clear previous status on new command
            app.status.clear();

            let input = app.take_input();
            if input.is_empty() {
                return Ok(());
            }

            match command::parse(&input) {
                Command::Message(text) => {
                    send_message(app, args, &text);
                }
                Command::Join(channel) => {
                    app.join_channel(&channel);

                    // Load history from storage
                    if !app.storage_token.is_empty() && !args.no_history {
                        match api::load_history(&app.storage_token, &channel) {
                            Ok(history) if !history.is_empty() => {
                                let count = history.len();
                                app.append_messages(&channel, history);
                                app.system_message(&format!(
                                    "Joined {} ({} messages from history)",
                                    channel, count
                                ));
                            }
                            _ => {
                                app.system_message(&format!("Joined {}", channel));
                            }
                        }
                    } else {
                        app.system_message(&format!("Joined {}", channel));
                    }

                    // Subscribe via gossip
                    if !app.peer_token.is_empty() {
                        match session::join_topic_mode(
                            &app.peer_token,
                            &channel,
                            app.direct_peer_mode.then_some("direct"),
                        ) {
                            Ok(_) => {}
                            Err(e) if e.to_string().contains("[already_joined]") => {}
                            Err(e) => {
                                app.system_message(&format!(
                                    "Failed to join gossip for {}: {}",
                                    channel, e
                                ));
                            }
                        }
                        if channel.starts_with('#') {
                            ensure_room_discovery_subscription(app, &channel);
                        }
                    }

                    save_channels(app, args);
                }
                Command::Part => {
                    if let Some(name) = app.part_channel() {
                        // Unsubscribe via gossip
                        if !app.peer_token.is_empty() && name.starts_with('#') {
                            let _ = session::leave_topic(&app.peer_token, &name);
                        }
                        save_channels(app, args);
                    }
                }
                Command::Nick(new_nick) => {
                    app.nickname = new_nick.clone();
                    app.system_message(&format!("Nickname changed to {}", new_nick));
                    // Update via DID provider
                    if !app.identity_token.is_empty() {
                        let _ = session::set_nickname(&app.identity_token, &new_nick);
                    }
                }
                Command::Connect(ticket) => {
                    connect_to_peer(app, args, &ticket);
                }
                Command::Ticket => {
                    show_ticket(app, args);
                }
                Command::Peers => {
                    show_peers(app, args);
                }
                Command::List => {
                    let names = app.channel_names();
                    if names.is_empty() {
                        app.system_message("No channels joined.");
                    } else {
                        app.system_message(&format!("Channels: {}", names.join(", ")));
                    }
                }
                Command::Help => {
                    for line in command::help_text().lines() {
                        app.system_message(line);
                    }
                }
                Command::Home => {
                    request_home_exit(app, args);
                }
                Command::Quit => {
                    save_channels(app, args);
                    save_known_nicks(app, args);
                    app.should_quit = true;
                }
                Command::Error(msg) => {
                    app.system_message(&msg);
                }
                Command::Msg(to, text) => {
                    let recipient_pubkey = app.known_nicks.get(&to).cloned();
                    match recipient_pubkey {
                        Some(pubkey) => {
                            let dm_channel = format!("@{}", to);
                            // Create/join the DM channel and load history
                            if !app.channels.iter().any(|c| c.name == dm_channel) {
                                // Warn on first-ever DM channel
                                let has_dm = app.channels.iter().any(|c| c.name.starts_with('@'));
                                app.join_channel(&dm_channel);
                                if !has_dm {
                                    app.system_message_to(
                                        &dm_channel,
                                        "Note: DMs are sent over shared gossip channels with content markers. They are NOT end-to-end encrypted.",
                                    );
                                }
                                if !app.storage_token.is_empty() && !args.no_history {
                                    if let Ok(history) =
                                        api::load_history(&app.storage_token, &dm_channel)
                                    {
                                        if !history.is_empty() {
                                            app.append_messages(&dm_channel, history);
                                        }
                                    }
                                }
                                save_channels(app, args);
                            } else {
                                // Switch to the existing DM channel
                                app.join_channel(&dm_channel);
                            }
                            send_dm(app, args, &to, &pubkey, &text);
                        }
                        None => {
                            app.system_message(&format!(
                                "Unknown nick '{}'. They must send a message first so we learn their pubkey.",
                                to
                            ));
                        }
                    }
                }
            }
        }
        KeyCode::Char(c) => {
            app.insert_char(c);
        }
        KeyCode::Backspace => {
            app.backspace();
        }
        KeyCode::Delete => {
            app.delete();
        }
        KeyCode::Left => {
            app.cursor_left();
        }
        KeyCode::Right => {
            app.cursor_right();
        }
        KeyCode::Home => {
            app.cursor = 0;
        }
        KeyCode::End => {
            app.cursor = app.input.len();
        }
        KeyCode::Tab => {
            app.next_channel();
        }
        KeyCode::BackTab => {
            app.prev_channel();
        }
        KeyCode::Esc => {
            request_home_exit(app, args);
        }
        _ => {}
    }
    Ok(())
}

/// Persist the joined channel list to storage.
fn save_channels(app: &App, args: &Args) {
    if app.storage_token.is_empty() || args.no_history {
        return;
    }
    let names = app.channel_names();
    let _ = api::save_json(&app.storage_token, "chat/channels.json", &names);
}

/// Persist the nick->pubkey mapping to storage.
fn save_known_nicks(app: &App, args: &Args) {
    if app.storage_token.is_empty() || args.no_history {
        return;
    }
    let _ = api::save_json(
        &app.storage_token,
        "chat/known_nicks.json",
        &app.known_nicks,
    );
}

/// Send a DM to a recipient, piggybacking on a shared gossip channel.
fn send_dm(app: &mut App, args: &Args, recipient_nick: &str, recipient_pubkey: &str, text: &str) {
    let dm_channel = format!("@{}", recipient_nick);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Sign the DM (signs over the wire content including DM marker)
    let wire_content = format!("{}{}{}{}", DM_PREFIX, recipient_pubkey, DM_DELIM, text);
    let signature = sign_message(app, args, &app.pubkey.clone(), ts, &wire_content);

    // Display locally in the @nick channel
    let msg = Message {
        sender_id: app.pubkey.clone(),
        sender_session_id: Some(app.session_id.clone()),
        sender_nick: app.nickname.clone(),
        content: text.to_string(),
        ts,
        display_ts: Some(ts),
        signature: signature.clone(),
        verified: Some(true),
    };
    app.append_messages(&dm_channel, vec![msg.clone()]);

    // Persist to DM history
    if !app.storage_token.is_empty() && !args.no_history {
        let _ = api::append_message(&app.storage_token, &dm_channel, &msg);
    }

    // Send via gossip on the first available #channel with DM marker
    if !app.peer_token.is_empty() {
        let gossip_topic = app
            .channels
            .iter()
            .find(|c| c.name.starts_with('#'))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "#general".to_string());

        let result = session::send_gossip(
            &app.peer_token,
            &gossip_topic,
            &app.nickname,
            &app.pubkey,
            Some(&app.session_id),
            ts,
            &wire_content,
            signature.as_deref(),
        );
        if let Err(e) = result {
            app.set_status(&format!("DM send failed: {}", e));
        }
    }
}

/// Send a chat message to the active channel via gossip.
fn send_message(app: &mut App, args: &Args, text: &str) {
    let channel = app.active_channel_name();
    if channel.is_empty() {
        app.set_status("No channel selected. Use /join #channel first.");
        return;
    }

    // If active channel is a DM channel (@nick), redirect to send_dm
    if let Some(recipient_nick) = channel.strip_prefix('@') {
        let recipient_nick = recipient_nick.to_string();
        if let Some(pubkey) = app.known_nicks.get(&recipient_nick).cloned() {
            send_dm(app, args, &recipient_nick, &pubkey, text);
        } else {
            app.set_status(&format!(
                "Unknown pubkey for {}. They must send a message first.",
                recipient_nick
            ));
        }
        return;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Sign the message if we have an identity token
    let signature = sign_message(app, args, &app.pubkey.clone(), ts, text);

    // Add to local display immediately
    let msg = Message {
        sender_id: app.pubkey.clone(),
        sender_session_id: Some(app.session_id.clone()),
        sender_nick: app.nickname.clone(),
        content: text.to_string(),
        ts,
        display_ts: Some(ts),
        signature: signature.clone(),
        verified: Some(true), // own messages are trusted
    };
    app.append_messages(&channel, vec![msg.clone()]);

    // Persist to storage
    if !app.storage_token.is_empty() && !args.no_history {
        let _ = api::append_message(&app.storage_token, &channel, &msg);
    }

    // Send via Carrier gossip — pass sender_id so the wire message matches our local copy
    if app.peer_token.is_empty() && !ensure_peer_capability(app, args) {
        app.set_status("Send queued locally only; P2P is not available yet.");
        return;
    }
    if !app.peer_token.is_empty() {
        let result = session::send_gossip(
            &app.peer_token,
            &channel,
            &app.nickname,
            &app.pubkey,
            Some(&app.session_id),
            ts,
            text,
            signature.as_deref(),
        );
        if let Err(e) = result {
            app.set_status(&format!("Send failed: {}", e));
        }
    }
}

/// Sign a message payload via did-provider. Returns hex signature or None.
fn sign_message(
    app: &App,
    _args: &Args,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> Option<String> {
    if app.identity_token.is_empty() {
        return None;
    }
    session::sign_message(&app.identity_token, sender_id, ts, content)
        .ok()
        .flatten()
}

/// Verify a message's signature via did-provider. Returns true if valid.
fn verify_message(app: &App, _args: &Args, msg: &Message) -> bool {
    if app.identity_token.is_empty() || msg.sender_id.is_empty() {
        return false;
    }
    session::verify_message(&app.identity_token, msg).unwrap_or(false)
}

fn local_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Poll for new gossip messages on all joined #channels.
fn poll_messages(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() {
        return;
    }

    // Only poll #channels (DMs arrive via #channel gossip)
    let channels: Vec<String> = app
        .channel_names()
        .into_iter()
        .filter(|c| c.starts_with('#'))
        .collect();

    let mut nicks_changed = false;

    for topic in &channels {
        match session::recv_messages(&app.peer_token, topic, 50, "chat", None) {
            Ok(mut msgs) => {
                // Verify signatures on received messages
                for msg in &mut msgs {
                    if msg.verified.is_none() {
                        msg.verified = Some(verify_message(app, args, msg));
                    }
                }

                for msg in msgs {
                    if is_own_message_instance(app, &msg) {
                        continue;
                    }

                    // Verification gate: skip unverified messages from unknown senders.
                    // Only record nick->DID for verified messages to prevent TOFU poisoning.
                    let is_known = app.known_nicks.contains_key(&msg.sender_nick);
                    if msg.verified == Some(false) && !is_known {
                        continue;
                    }

                    // Track nick -> pubkey ONLY for verified senders
                    if !msg.sender_nick.is_empty()
                        && msg.sender_nick != "*"
                        && !msg.sender_id.is_empty()
                        && msg.verified != Some(false)
                    {
                        if let Some(existing) = app.known_nicks.get(&msg.sender_nick) {
                            if existing != &msg.sender_id {
                                app.system_message_to(
                                    &app.channels
                                        .first()
                                        .map(|c| c.name.clone())
                                        .unwrap_or_default(),
                                    &format!(
                                        "Warning: ignoring nick '{}' from different pubkey",
                                        msg.sender_nick
                                    ),
                                );
                                continue;
                            }
                        } else if app.known_nicks.len() < 10_000 {
                            app.known_nicks
                                .insert(msg.sender_nick.clone(), msg.sender_id.clone());
                            nicks_changed = true;
                        }
                    }

                    // Check for DM marker — also subject to verification gate
                    if msg.content.starts_with(DM_PREFIX) {
                        // Skip unverified DMs entirely
                        if msg.verified == Some(false) {
                            continue;
                        }
                        // Parse: \x01DM:<pubkey>\x01<actual message>
                        let after_prefix = &msg.content[DM_PREFIX.len()..];
                        if let Some(delim_pos) = after_prefix.find(DM_DELIM) {
                            let recipient_pubkey = &after_prefix[..delim_pos];
                            let actual_text = &after_prefix[delim_pos + 1..];

                            if recipient_pubkey == app.pubkey {
                                // DM addressed to us
                                let dm_channel = format!("@{}", msg.sender_nick);

                                // Auto-create DM channel if needed
                                if !app.channels.iter().any(|c| c.name == dm_channel) {
                                    let has_dm =
                                        app.channels.iter().any(|c| c.name.starts_with('@'));
                                    app.join_channel(&dm_channel);
                                    if !has_dm {
                                        app.system_message_to(
                                            &dm_channel,
                                            "Note: DMs are sent over shared gossip channels with content markers. They are NOT end-to-end encrypted.",
                                        );
                                    }
                                    // Load DM history
                                    if !app.storage_token.is_empty() && !args.no_history {
                                        if let Ok(history) =
                                            api::load_history(&app.storage_token, &dm_channel)
                                        {
                                            if !history.is_empty() {
                                                app.append_messages(&dm_channel, history);
                                            }
                                        }
                                    }
                                    save_channels(app, args);
                                }

                                let dm_msg = Message {
                                    sender_id: msg.sender_id.clone(),
                                    sender_session_id: msg.sender_session_id.clone(),
                                    sender_nick: msg.sender_nick.clone(),
                                    content: actual_text.to_string(),
                                    ts: msg.ts,
                                    display_ts: Some(local_now_secs()),
                                    signature: msg.signature.clone(),
                                    verified: msg.verified,
                                };

                                // Persist DM to storage
                                if !app.storage_token.is_empty() && !args.no_history {
                                    let _ = api::append_message(
                                        &app.storage_token,
                                        &dm_channel,
                                        &dm_msg,
                                    );
                                }
                                app.append_messages(&dm_channel, vec![dm_msg]);
                            }
                            // DM to someone else: silently drop
                        }
                        continue;
                    }

                    // Regular message — persist and display
                    let mut msg = msg;
                    msg.display_ts = Some(local_now_secs());
                    if !msg.sender_id.is_empty() && msg.verified != Some(false) {
                        let key =
                            room_peer_key(topic, &msg.sender_id, msg.sender_session_id.as_deref());
                        app.attached_room_peers.insert(key.clone());
                        app.attach_retry_after.remove(&key);
                    }
                    if !app.storage_token.is_empty() && !args.no_history {
                        let _ = api::append_message(&app.storage_token, topic, &msg);
                    }
                    app.append_messages(topic, vec![msg]);
                }
            }
            Err(_) => {
                // Silently ignore poll errors (network hiccup)
            }
        }
    }

    if nicks_changed {
        save_known_nicks(app, args);
    }
}

/// Poll peer count. If new peers joined, broadcast history.
fn poll_peers(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() {
        return;
    }

    let old_count = app.peer_count;

    if let Ok(peers) = session::list_peers(&app.peer_token) {
        app.peer_count = peers.len();
    }

    // First peer connected — clear connecting state
    if app.p2p_connecting && app.peer_count > 0 {
        app.p2p_connecting = false;
        app.system_message(&format!("Connected to {} peer(s)", app.peer_count));
    }

    // New peer joined — share recent history so they catch up
    if app.peer_count > old_count && !args.no_sync {
        broadcast_history(app, args);
    }
}

fn room_peer_key(room: &str, did: &str, session_id: Option<&str>) -> String {
    match session_id.filter(|value| !value.is_empty()) {
        Some(session_id) => format!("{}|{}|{}", room, did, session_id),
        None => format!("{}|{}", room, did),
    }
}

fn is_same_chat_instance(
    self_did: &str,
    self_session_id: &str,
    sender_did: &str,
    sender_session_id: Option<&str>,
) -> bool {
    if self_did.is_empty() || sender_did != self_did {
        return false;
    }
    match sender_session_id.filter(|value| !value.is_empty()) {
        Some(sender_session_id) => sender_session_id == self_session_id,
        None => true,
    }
}

fn is_own_message_instance(app: &App, msg: &Message) -> bool {
    is_same_chat_instance(
        &app.pubkey,
        &app.session_id,
        &msg.sender_id,
        msg.sender_session_id.as_deref(),
    )
}

fn presence_consumer_id(room: &str) -> String {
    format!("chat-presence:{}", room)
}

fn ensure_room_discovery_subscription(app: &mut App, room: &str) {
    if app.peer_token.is_empty() || !room.starts_with('#') {
        return;
    }
    let discovery_topic = session::chat_discovery_topic(room);
    let _ = session::join_topic_mode(
        &app.peer_token,
        &discovery_topic,
        app.direct_peer_mode.then_some("direct"),
    );
}

fn attach_topic_direct(app: &mut App, topic: &str, peers: &[String]) -> bool {
    let _ = session::leave_topic(&app.peer_token, topic);
    if session::join_topic_mode(&app.peer_token, topic, Some("direct")).is_err() {
        return false;
    }
    session::attach_room_peer_until_joined(&app.peer_token, topic, peers).unwrap_or(false)
}

fn poll_presence(app: &mut App) {
    if app.peer_token.is_empty() {
        return;
    }

    let rooms: Vec<String> = app
        .channel_names()
        .into_iter()
        .filter(|room| room.starts_with('#'))
        .collect();
    if rooms.is_empty() {
        return;
    }

    let now = Instant::now();
    let should_announce = app
        .last_presence_announce
        .map(|last| now.duration_since(last) >= PRESENCE_ANNOUNCE_INTERVAL)
        .unwrap_or(true);
    let ticket = if should_announce {
        session::get_ticket(&app.peer_token).ok().flatten()
    } else {
        None
    };

    for room in rooms {
        ensure_room_discovery_subscription(app, &room);

        if let Some(ticket) = ticket.as_deref() {
            let _ = session::announce_presence(
                &app.identity_token,
                &app.peer_token,
                &room,
                &app.nickname,
                &app.pubkey,
                Some(&app.session_id),
                ticket,
            );
        }

        let consumer_id = presence_consumer_id(&room);
        let presences = match session::recv_presence_announcements(
            &app.identity_token,
            &app.peer_token,
            &room,
            &consumer_id,
            None,
        ) {
            Ok(presences) => presences,
            Err(_) => continue,
        };

        for presence in presences {
            if presence.did.is_empty()
                || is_same_chat_instance(
                    &app.pubkey,
                    &app.session_id,
                    &presence.did,
                    presence.session_id.as_deref(),
                )
            {
                continue;
            }

            let key = room_peer_key(&room, &presence.did, presence.session_id.as_deref());
            if app.attached_room_peers.contains(&key) {
                continue;
            }
            if app
                .attach_retry_after
                .get(&key)
                .is_some_and(|deadline| *deadline > now)
            {
                continue;
            }

            match session::remember_peer(&app.peer_token, &presence.ticket) {
                Ok(peer_ids) if !peer_ids.is_empty() => {
                    let room_attached = attach_topic_direct(app, &room, &peer_ids);
                    let discovery_topic = session::chat_discovery_topic(&room);
                    let _ = attach_topic_direct(app, &discovery_topic, &peer_ids);
                    if room_attached {
                        app.direct_peer_mode = true;
                        app.attached_room_peers.insert(key.clone());
                        app.attach_retry_after.remove(&key);
                        app.system_message_to(
                            &room,
                            &format!("{} reached {} via Carrier", presence.nick, room),
                        );
                        if let Ok(peers) = session::list_peers(&app.peer_token) {
                            app.peer_count = peers.len();
                        }
                    } else {
                        app.attach_retry_after
                            .insert(key, now + PRESENCE_RETRY_BACKOFF);
                    }
                }
                _ => {
                    app.attach_retry_after
                        .insert(key, now + PRESENCE_RETRY_BACKOFF);
                }
            }
        }
    }

    if should_announce {
        app.last_presence_announce = Some(now);
    }
}

/// Broadcast recent channel history via gossip so new peers can catch up.
/// Dedup on the receiving side prevents duplicates.
fn broadcast_history(app: &App, _args: &Args) {
    if app.peer_token.is_empty() {
        return;
    }

    for ch in app.channels.iter().filter(|c| c.name.starts_with('#')) {
        let recent: Vec<_> = ch
            .messages
            .iter()
            .rev()
            .take(50)
            .rev()
            .filter(|m| m.sender_nick != "*") // skip system messages
            .collect();

        for msg in recent {
            let _ = session::send_gossip(
                &app.peer_token,
                &ch.name,
                &msg.sender_nick,
                &msg.sender_id,
                msg.sender_session_id.as_deref(),
                msg.ts,
                &msg.content,
                msg.signature.as_deref(),
            );
        }
    }
}

/// Connect to a peer via ticket.
fn connect_to_peer(app: &mut App, args: &Args, ticket: &str) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app, args) {
        app.system_message("Not connected to P2P network.");
        return;
    }

    match session::connect_peer(&app.peer_token, ticket) {
        Ok(_) => {
            app.direct_peer_mode = true;
            app.system_message("Connected to peer via ticket");
            attach_joined_channels_direct(app);
        }
        Err(e) => {
            app.system_message(&format!("Connect failed: {}", e));
        }
    }
}

fn attach_joined_channels_direct(app: &mut App) {
    let mut peers = Vec::new();
    for _ in 0..20 {
        match session::list_peers(&app.peer_token) {
            Ok(current) if !current.is_empty() => {
                peers = current;
                break;
            }
            Ok(_) => {}
            Err(e) => {
                app.system_message(&format!("Peer list failed after connect: {}", e));
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    if peers.is_empty() {
        app.system_message("Connected to peer, but no reachable room peers yet.");
        return;
    }

    for channel in app
        .channel_names()
        .into_iter()
        .filter(|channel| channel.starts_with('#'))
    {
        let room_attached = attach_topic_direct(app, &channel, &peers);
        let discovery_topic = session::chat_discovery_topic(&channel);
        let _ = attach_topic_direct(app, &discovery_topic, &peers);
        if !room_attached {
            app.system_message(&format!("Waiting for room attach on {}", channel));
        }
    }
}

/// Show the user's ticket for sharing.
fn show_ticket(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app, args) {
        app.system_message("Not connected to P2P network.");
        return;
    }

    match session::get_ticket(&app.peer_token) {
        Ok(Some(ticket)) => {
            app.system_message(&format!("Your ticket: {}", ticket));
        }
        Ok(None) => {
            app.system_message("Could not get ticket.");
        }
        Err(e) => {
            app.system_message(&format!("Ticket error: {}", e));
        }
    }
}

// ── Raw stdin reader ────────────────────────────────────────────────
//
// crossterm's event system has internal buffering and escape-sequence
// timeout logic designed for PTY terminals. On serial consoles (crosvm
// 16550 UART), this causes a one-event delay ("press twice to register").
//
// This module reads raw bytes from stdin via libc::read() in non-blocking
// mode and parses common escape sequences directly. It bypasses crossterm's
// input layer entirely while still using ratatui/crossterm for rendering.

mod raw_input {
    use crossterm::event::KeyCode;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// A parsed key event from raw stdin bytes.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RawKey {
        pub code: KeyCode,
        pub ctrl: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ParseResult {
        Key(usize),
        Skip(usize),
        Incomplete,
    }

    #[derive(Debug)]
    struct PendingInput {
        bytes: Vec<u8>,
        last_read: Option<Instant>,
    }

    fn pending_input() -> &'static Mutex<PendingInput> {
        static PENDING: OnceLock<Mutex<PendingInput>> = OnceLock::new();
        PENDING.get_or_init(|| {
            Mutex::new(PendingInput {
                bytes: Vec::new(),
                last_read: None,
            })
        })
    }

    fn debug_log(message: &str) {
        static LOG_PATH: OnceLock<Option<String>> = OnceLock::new();
        let path = LOG_PATH.get_or_init(|| {
            std::env::var("ELASTOS_CHAT_INPUT_LOG")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
        let Some(path) = path else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}", message);
        }
    }

    fn log_bytes(prefix: &str, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let rendered = bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        debug_log(&format!("{} {}", prefix, rendered));
    }

    fn log_key(key: &RawKey) {
        debug_log(&format!("key {:?} ctrl={}", key.code, key.ctrl));
    }

    pub fn has_pending_input() -> bool {
        !pending_input().lock().unwrap().bytes.is_empty()
    }

    /// Wait for stdin to become readable, up to `timeout_ms` milliseconds.
    /// Returns true if data is available.
    pub fn poll_stdin(timeout_ms: i32) -> bool {
        unsafe {
            let mut pfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            libc::poll(&mut pfd, 1, timeout_ms) > 0 && (pfd.revents & libc::POLLIN) != 0
        }
    }

    /// Read available bytes using poll(0) + blocking read().
    /// No O_NONBLOCK — avoids serial tty driver quirks.
    fn read_available(buf: &mut Vec<u8>) {
        let mut tmp = [0u8; 64];
        // First read: we know data is available (caller checked with poll).
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                tmp.as_mut_ptr() as *mut libc::c_void,
                tmp.len(),
            )
        };
        if n > 0 {
            buf.extend_from_slice(&tmp[..n as usize]);
            log_bytes("read", &tmp[..n as usize]);
        }
        // Drain any remaining bytes that arrived during the first read.
        while poll_stdin(0) {
            let n = unsafe {
                libc::read(
                    libc::STDIN_FILENO,
                    tmp.as_mut_ptr() as *mut libc::c_void,
                    tmp.len(),
                )
            };
            if n <= 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n as usize]);
            log_bytes("read", &tmp[..n as usize]);
        }
    }

    /// Drain any stale bytes on stdin (boot output, echo, etc.).
    pub fn drain_stdin() {
        let mut tmp = [0u8; 256];
        while poll_stdin(0) {
            let n = unsafe {
                libc::read(
                    libc::STDIN_FILENO,
                    tmp.as_mut_ptr() as *mut libc::c_void,
                    tmp.len(),
                )
            };
            if n <= 0 {
                break;
            }
        }
    }

    /// Read and parse all pending keystrokes from stdin.
    pub fn read_keys() -> Vec<RawKey> {
        let mut pending = pending_input().lock().unwrap();
        if poll_stdin(0) {
            read_available(&mut pending.bytes);
            pending.last_read = Some(Instant::now());
        }

        // Some terminals split escape sequences across multiple reads. Give the
        // trailing bytes a short grace window before we decide whether to treat
        // the prefix as an actual key or an incomplete sequence.
        for _ in 0..3 {
            if !has_incomplete_suffix(&pending.bytes) || !poll_stdin(5) {
                break;
            }
            read_available(&mut pending.bytes);
            pending.last_read = Some(Instant::now());
        }

        let stale_incomplete_suffix = has_incomplete_suffix(&pending.bytes)
            && pending
                .last_read
                .is_some_and(|last_read| last_read.elapsed() >= Duration::from_secs(2));

        if stale_incomplete_suffix {
            if pending.bytes == [0x1B] {
                log_bytes("flush-esc", &pending.bytes);
                pending.bytes.clear();
                return vec![RawKey {
                    code: KeyCode::Esc,
                    ctrl: false,
                }];
            }
            discard_incomplete_suffix(&mut pending.bytes);
        }

        parse_buffer(&mut pending.bytes)
    }

    /// Parse as many complete keys as possible from the buffer, leaving any
    /// incomplete trailing bytes in place for the next read.
    fn parse_buffer(buf: &mut Vec<u8>) -> Vec<RawKey> {
        let mut keys = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let mut key = RawKey {
                code: KeyCode::Null,
                ctrl: false,
            };
            match parse_one(&buf[i..], &mut key) {
                ParseResult::Key(consumed) => {
                    keys.push(key);
                    i += consumed;
                }
                ParseResult::Skip(consumed) => {
                    i += consumed;
                }
                ParseResult::Incomplete => break,
            }
        }

        if i > 0 {
            buf.drain(..i);
        }

        for key in &keys {
            log_key(key);
        }

        keys
    }

    fn discard_incomplete_suffix(bytes: &mut Vec<u8>) {
        if bytes.is_empty() {
            return;
        }

        let original_len = bytes.len();
        let mut idx = bytes.len();
        while idx > 0 {
            idx -= 1;
            if bytes[idx] == 0x1B {
                log_bytes("discard", &bytes[idx..]);
                bytes.truncate(idx);
                return;
            }
        }

        if matches!(bytes.last(), Some(0xC2..=0xF4)) {
            log_bytes("discard", &bytes[original_len - 1..]);
            bytes.truncate(original_len - 1);
        }
    }

    fn parse_one(bytes: &[u8], key: &mut RawKey) -> ParseResult {
        let b = bytes[0];
        match b {
            0x1B => parse_escape(bytes, key),
            0x9B => match parse_csi(&bytes[1..], key) {
                ParseResult::Key(consumed) => ParseResult::Key(consumed + 1),
                ParseResult::Skip(consumed) => ParseResult::Skip(consumed + 1),
                ParseResult::Incomplete => ParseResult::Incomplete,
            },
            0x08 | 0x7F => {
                *key = RawKey {
                    code: KeyCode::Backspace,
                    ctrl: false,
                };
                ParseResult::Key(1)
            }
            0x0A | 0x0D => {
                let consumed = if bytes.get(1).copied() == Some(other_newline(b)) {
                    2
                } else {
                    1
                };
                *key = RawKey {
                    code: KeyCode::Enter,
                    ctrl: false,
                };
                ParseResult::Key(consumed)
            }
            0x01..=0x1A => {
                let (code, ctrl) = match b {
                    0x01 => (KeyCode::Home, false),     // Ctrl+A -> Home
                    0x03 => (KeyCode::Char('c'), true), // Ctrl+C
                    0x04 => (KeyCode::Delete, false),   // Ctrl+D -> Delete
                    0x05 => (KeyCode::End, false),      // Ctrl+E -> End
                    0x09 => (KeyCode::Tab, false),      // Tab
                    0x0E => (KeyCode::Char('n'), true), // Ctrl+N
                    0x10 => (KeyCode::Char('p'), true), // Ctrl+P
                    0x11 => (KeyCode::Char('q'), true), // Ctrl+Q
                    _ => (KeyCode::Char((b + b'a' - 1) as char), true),
                };
                *key = RawKey { code, ctrl };
                ParseResult::Key(1)
            }
            0x20..=0x7E => {
                *key = RawKey {
                    code: KeyCode::Char(b as char),
                    ctrl: false,
                };
                ParseResult::Key(1)
            }
            0x80..=0xFF => parse_utf8(bytes, key),
            _ => ParseResult::Skip(1),
        }
    }

    fn parse_escape(bytes: &[u8], key: &mut RawKey) -> ParseResult {
        if bytes.len() == 1 {
            return ParseResult::Incomplete;
        }

        match bytes[1] {
            b'[' => match parse_csi(&bytes[2..], key) {
                ParseResult::Key(consumed) => ParseResult::Key(consumed + 2),
                ParseResult::Skip(consumed) => ParseResult::Skip(consumed + 2),
                ParseResult::Incomplete => ParseResult::Incomplete,
            },
            b'O' => {
                if bytes.len() < 3 {
                    return ParseResult::Incomplete;
                }
                let code = match bytes[2] {
                    b'A' => Some(KeyCode::Up),
                    b'B' => Some(KeyCode::Down),
                    b'C' => Some(KeyCode::Right),
                    b'D' => Some(KeyCode::Left),
                    b'H' => Some(KeyCode::Home),
                    b'F' => Some(KeyCode::End),
                    _ => None,
                };
                if let Some(code) = code {
                    *key = RawKey { code, ctrl: false };
                    ParseResult::Key(3)
                } else {
                    ParseResult::Skip(3)
                }
            }
            _ => {
                *key = RawKey {
                    code: KeyCode::Esc,
                    ctrl: false,
                };
                ParseResult::Key(1)
            }
        }
    }

    /// Parse a CSI sequence (after ESC [).
    fn parse_csi(bytes: &[u8], key: &mut RawKey) -> ParseResult {
        // Collect numeric parameters
        let mut params = Vec::new();
        let mut num = 0u32;
        let mut has_num = false;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'0'..=b'9' => {
                    num = num * 10 + (b - b'0') as u32;
                    has_num = true;
                    i += 1;
                }
                b';' => {
                    params.push(if has_num { num } else { 0 });
                    num = 0;
                    has_num = false;
                    i += 1;
                }
                // Final byte
                _ => {
                    if has_num {
                        params.push(num);
                    }
                    break;
                }
            }
        }

        if i >= bytes.len() {
            return ParseResult::Incomplete;
        }

        let final_byte = bytes[i];
        i += 1;

        let code = match final_byte {
            b'A' => KeyCode::Up,
            b'B' => KeyCode::Down,
            b'C' => KeyCode::Right,
            b'D' => KeyCode::Left,
            b'H' => KeyCode::Home,
            b'F' => KeyCode::End,
            b'Z' => KeyCode::BackTab,
            b'~' => match params.first() {
                Some(1) | Some(7) => KeyCode::Home,
                Some(2) => KeyCode::Insert,
                Some(3) => KeyCode::Delete,
                Some(4) | Some(8) => KeyCode::End,
                Some(5) => KeyCode::PageUp,
                Some(6) => KeyCode::PageDown,
                _ => return ParseResult::Skip(i),
            },
            _ => return ParseResult::Skip(i),
        };

        *key = RawKey { code, ctrl: false };
        ParseResult::Key(i)
    }

    fn parse_utf8(bytes: &[u8], key: &mut RawKey) -> ParseResult {
        let width = utf8_width(bytes[0]);
        if width == 0 {
            return ParseResult::Skip(1);
        }
        if bytes.len() < width {
            return ParseResult::Incomplete;
        }
        match std::str::from_utf8(&bytes[..width]) {
            Ok(s) => match s.chars().next() {
                Some(ch) if !ch.is_control() => {
                    *key = RawKey {
                        code: KeyCode::Char(ch),
                        ctrl: false,
                    };
                    ParseResult::Key(width)
                }
                _ => ParseResult::Skip(width),
            },
            Err(_) => ParseResult::Skip(width),
        }
    }

    fn utf8_width(first: u8) -> usize {
        match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 0,
        }
    }

    fn other_newline(b: u8) -> u8 {
        if b == 0x0D {
            0x0A
        } else {
            0x0D
        }
    }

    fn has_incomplete_suffix(bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }

        match bytes[bytes.len() - 1] {
            0x1B => return true,
            0xC2..=0xF4 => return true, // possible start of incomplete UTF-8
            _ => {}
        }

        if bytes.len() >= 2 && bytes[bytes.len() - 2] == 0x1B {
            return matches!(bytes[bytes.len() - 1], b'[' | b'O');
        }

        let mut idx = bytes.len();
        while idx > 0 {
            idx -= 1;
            if bytes[idx] == 0x1B {
                if idx + 1 >= bytes.len() {
                    return true;
                }
                return match bytes[idx + 1] {
                    b'[' => bytes[idx + 2..]
                        .iter()
                        .all(|b| b.is_ascii_digit() || *b == b';'),
                    b'O' => idx + 2 >= bytes.len(),
                    _ => false,
                };
            }
        }

        false
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse_all(bytes: &[u8]) -> (Vec<RawKey>, Vec<u8>) {
            let mut buf = bytes.to_vec();
            let keys = parse_buffer(&mut buf);
            (keys, buf)
        }

        #[test]
        fn test_ascii_input_parses() {
            let (keys, rest) = parse_all(b"ab");
            assert_eq!(
                keys,
                vec![
                    RawKey {
                        code: KeyCode::Char('a'),
                        ctrl: false
                    },
                    RawKey {
                        code: KeyCode::Char('b'),
                        ctrl: false
                    },
                ]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_crlf_is_single_enter() {
            let (keys, rest) = parse_all(&[0x0D, 0x0A]);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Enter,
                    ctrl: false
                }]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_ctrl_h_maps_to_backspace() {
            let (keys, rest) = parse_all(&[0x08]);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Backspace,
                    ctrl: false
                }]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_incomplete_csi_is_buffered_until_complete() {
            let mut buf = vec![0x1B, b'['];
            let keys = parse_buffer(&mut buf);
            assert!(keys.is_empty());
            assert_eq!(buf, vec![0x1B, b'[']);

            buf.push(b'A');
            let keys = parse_buffer(&mut buf);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Up,
                    ctrl: false
                }]
            );
            assert!(buf.is_empty());
        }

        #[test]
        fn test_utf8_input_parses() {
            let (keys, rest) = parse_all("é".as_bytes());
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Char('é'),
                    ctrl: false
                }]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_incomplete_suffix_detection() {
            assert!(has_incomplete_suffix(&[0x1B]));
            assert!(has_incomplete_suffix(&[0x1B, b'[']));
            assert!(has_incomplete_suffix(&[0x1B, b'[', b'1', b';']));
            assert!(has_incomplete_suffix("é".as_bytes().get(..1).unwrap()));
            assert!(!has_incomplete_suffix(&[0x1B, b'[', b'A']));
            assert!(!has_incomplete_suffix(b"abc"));
        }

        #[test]
        fn test_ss3_arrow_keys_parse() {
            let (keys, rest) = parse_all(&[0x1B, b'O', b'D']);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Left,
                    ctrl: false
                }]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_c1_csi_arrow_keys_parse() {
            let (keys, rest) = parse_all(&[0x9B, b'C']);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Right,
                    ctrl: false
                }]
            );
            assert!(rest.is_empty());
        }

        #[test]
        fn test_stale_escape_prefix_is_discarded() {
            let mut buf = vec![0x1B, b'['];
            discard_incomplete_suffix(&mut buf);
            let keys = parse_buffer(&mut buf);
            assert!(keys.is_empty());
            assert!(buf.is_empty());
        }

        #[test]
        fn test_single_escape_is_buffered() {
            let mut buf = vec![0x1B];
            let keys = parse_buffer(&mut buf);
            assert!(keys.is_empty());
            assert_eq!(buf, vec![0x1B]);
        }

        #[test]
        fn test_fragmented_delete_sequence_parses_after_followup_bytes() {
            let mut buf = vec![0x1B];
            let keys = parse_buffer(&mut buf);
            assert!(keys.is_empty());
            assert_eq!(buf, vec![0x1B]);

            buf.extend_from_slice(b"[3~");
            let keys = parse_buffer(&mut buf);
            assert_eq!(
                keys,
                vec![RawKey {
                    code: KeyCode::Delete,
                    ctrl: false
                }]
            );
            assert!(buf.is_empty());
        }
    }
}

/// Show connected peers.
fn show_peers(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app, args) {
        app.system_message("Not connected to P2P network.");
        return;
    }

    match session::list_peers(&app.peer_token) {
        Ok(peers) if peers.is_empty() => {
            app.system_message("No peers connected.");
        }
        Ok(peers) => {
            app.system_message(&format!("{} peer(s):", peers.len()));
            for peer in peers {
                app.system_message(&format!("  {}", peer));
            }
        }
        Err(e) => {
            app.system_message(&format!("Peers error: {}", e));
        }
    }
}
