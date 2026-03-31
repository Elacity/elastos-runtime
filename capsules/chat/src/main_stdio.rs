//! ElastOS P2P Chat — full-screen ANSI stdio variant (WASM / non-KVM).
//!
//! Same app core, commands, and wire format as the TUI variant.
//! Uses inherited stdin/stdout with ANSI rendering instead of ratatui/crossterm.
//! Single-threaded for WASI compatibility.

mod ansi_ui;
#[path = "carrier.rs"]
mod api;
mod app;
mod command;
mod session;
mod term_input;

use std::time::{Duration, Instant};

use anyhow::Result;
use app::{App, Message};
use command::Command;
use term_input::{Key, RawKey};

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

#[derive(Debug, Clone)]
struct Args {
    nick: String,
    nick_explicit: bool,
    connect: Option<String>,
    no_history: bool,
    no_sync: bool,
    history_limit: usize,
}

fn parse_args() -> Args {
    let mut nick = std::env::var("ELASTOS_NICK").unwrap_or_else(|_| "anon".into());
    let mut nick_explicit = std::env::var("ELASTOS_NICK")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let mut connect = std::env::var("ELASTOS_CONNECT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut no_history = false;
    let mut no_sync = false;
    let mut history_limit = 1000usize;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
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
                    let value = args[i].trim();
                    if !value.is_empty() {
                        connect = Some(value.to_string());
                    }
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

    if let Some(payload) = forwarded_command_payload() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
            if let Some(n) = v.get("nick").and_then(|n| n.as_str()) {
                if !n.is_empty() {
                    nick = n.to_string();
                    nick_explicit = true;
                }
            }
            if let Some(c) = v.get("connect").and_then(|c| c.as_str()) {
                let value = c.trim();
                if !value.is_empty() {
                    connect = Some(value.to_string());
                }
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

    Args {
        nick,
        nick_explicit,
        connect,
        no_history,
        no_sync,
        history_limit,
    }
}

fn main() -> Result<()> {
    let args = parse_args();
    let mut app = App::new(&args.nick);
    app.max_messages = args.history_limit;

    match session::resolve_identity(&app.nickname, args.nick_explicit) {
        Ok(identity) => {
            app.nickname = identity.nickname;
            app.pubkey = identity.did;
            app.identity_token = identity.token;
        }
        Err(e) => {
            app.system_message(&format!("Identity unavailable: {}", e));
        }
    }

    match session::acquire_peer_token() {
        Ok(token) => {
            app.peer_token = token;
        }
        Err(e) => {
            app.system_message(&format!("P2P unavailable: {}", e));
        }
    }

    let channels = vec!["#general".to_string()];
    for channel in &channels {
        app.join_channel(channel);
        if !app.peer_token.is_empty() && channel.starts_with('#') {
            let _ = session::join_topic(&app.peer_token, channel);
            ensure_room_discovery_subscription(&mut app, channel);
        }
    }
    app.active_channel = 0;

    app.system_message_to(
        "#general",
        &format!("Welcome to ElastOS IRC v{}! Type /help for commands.", CHAT_VERSION),
    );

    if !app.peer_token.is_empty() {
        app.p2p_connecting = true;
        if let Some(ticket) = args.connect.as_deref() {
            connect_to_peer(&mut app, &args, ticket);
        }
        if let Ok(Some(ticket)) = session::get_ticket(&app.peer_token) {
            app.system_message(&format!("Ticket: {}", ticket));
        }
        if app.peer_count == 0 {
            app.system_message("Looking for peers... (this can take up to 60s on first run)");
        }
    }

    term_input::drain_stdin();
    let mut ui = ansi_ui::AnsiUi::enter()?;
    let mut last_poll = Instant::now();

    loop {
        ui.render(&app)?;

        if app.should_quit {
            break;
        }

        if term_input::has_pending_input() || term_input::poll_stdin(50) {
            for key in term_input::read_keys() {
                handle_key(&mut app, key, &args)?;
                if app.should_quit {
                    break;
                }
            }
        }

        if !app.peer_token.is_empty() && last_poll.elapsed() >= Duration::from_millis(500) {
            last_poll = Instant::now();
            poll_messages(&mut app, &args);
            poll_peers(&mut app, &args);
            poll_presence(&mut app);
        }
    }

    drop(ui);

    if app.return_home_requested {
        signal_home_exit();
    }

    Ok(())
}

fn signal_home_exit() -> ! {
    if std::env::var("ELASTOS_PARENT_SURFACE").ok().as_deref() == Some("pc2") {
        std::process::exit(0);
    }
    std::process::exit(CHAT_RETURN_HOME_EXIT_CODE);
}

fn request_home_exit(app: &mut App, args: &Args) {
    save_channels(app, args);
    save_known_nicks(app, args);
    app.return_home_requested = true;
    app.should_quit = true;
}

fn ensure_peer_capability(app: &mut App) -> bool {
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
            true
        }
        Err(e) => {
            app.system_message(&format!("P2P unavailable: {}", e));
            false
        }
    }
}

fn ensure_storage_capability(app: &mut App) -> bool {
    if !app.storage_token.is_empty() {
        return true;
    }
    match session::acquire_storage_token() {
        Ok(token) => {
            app.storage_token = token;
            true
        }
        Err(e) => {
            app.set_status(&format!("History storage unavailable: {}", e));
            false
        }
    }
}

fn handle_key(app: &mut App, key: RawKey, args: &Args) -> Result<()> {
    if key.ctrl {
        match key.code {
            Key::Char('c') | Key::Char('q') => {
                save_channels(app, args);
                save_known_nicks(app, args);
                app.should_quit = true;
                return Ok(());
            }
            Key::Char('n') => {
                app.next_channel();
                return Ok(());
            }
            Key::Char('p') => {
                app.prev_channel();
                return Ok(());
            }
            Key::Char('b') => {
                app.cursor_left();
                return Ok(());
            }
            Key::Char('f') => {
                app.cursor_right();
                return Ok(());
            }
            Key::Char('u') => {
                app.kill_to_start();
                return Ok(());
            }
            Key::Char('w') => {
                app.delete_word_backward();
                return Ok(());
            }
            _ => {}
        }
    }

    match key.code {
        Key::Enter => {
            app.status.clear();
            let input = app.take_input();
            if input.is_empty() {
                return Ok(());
            }
            match command::parse(&input) {
                Command::Message(text) => send_message(app, args, &text),
                Command::Join(channel) => {
                    app.join_channel(&channel);
                    if !args.no_history && ensure_storage_capability(app) {
                        if let Ok(history) = api::load_history(&app.storage_token, &channel) {
                            if !history.is_empty() {
                                app.append_messages(&channel, history);
                            }
                        }
                    }
                    if !app.peer_token.is_empty() && channel.starts_with('#') {
                        let _ = session::join_topic_mode(
                            &app.peer_token,
                            &channel,
                            app.direct_peer_mode.then_some("direct"),
                        );
                        ensure_room_discovery_subscription(app, &channel);
                    }
                    app.system_message(&format!("Joined {}", channel));
                    save_channels(app, args);
                }
                Command::Part => {
                    if let Some(name) = app.part_channel() {
                        if !app.peer_token.is_empty() && name.starts_with('#') {
                            let _ = session::leave_topic(&app.peer_token, &name);
                        }
                        save_channels(app, args);
                    }
                }
                Command::Nick(new_nick) => {
                    app.nickname = new_nick.clone();
                    if !app.identity_token.is_empty() {
                        let _ = session::set_nickname(&app.identity_token, &new_nick);
                    }
                    app.system_message(&format!("Nickname changed to {}", new_nick));
                }
                Command::Connect(ticket) => connect_to_peer(app, args, &ticket),
                Command::Ticket => show_ticket(app, args),
                Command::Peers => show_peers(app, args),
                Command::List => {
                    let names = app.channel_names();
                    app.system_message(&format!("Channels: {}", names.join(", ")));
                }
                Command::Help => {
                    for line in command::help_text().lines() {
                        app.system_message(line);
                    }
                }
                Command::Home => request_home_exit(app, args),
                Command::Quit => {
                    save_channels(app, args);
                    save_known_nicks(app, args);
                    app.should_quit = true;
                }
                Command::Msg(to, text) => {
                    let dm_text = format!("@{}: {}", to, text);
                    send_message(app, args, &dm_text);
                }
                Command::Error(msg) => app.system_message(&msg),
            }
        }
        Key::Char(c) => app.insert_char(c),
        Key::Backspace => app.backspace(),
        Key::Delete => app.delete(),
        Key::Left => app.cursor_left(),
        Key::Right => app.cursor_right(),
        Key::Home => app.cursor = 0,
        Key::End => app.cursor = app.input.len(),
        Key::Tab => app.next_channel(),
        Key::BackTab => app.prev_channel(),
        Key::Esc => request_home_exit(app, args),
        _ => {}
    }

    Ok(())
}

fn save_channels(app: &App, args: &Args) {
    if app.storage_token.is_empty() || args.no_history {
        return;
    }
    let _ = api::save_json(
        &app.storage_token,
        "chat/channels.json",
        &app.channel_names(),
    );
}

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

fn sign_message(app: &App, sender_id: &str, ts: u64, content: &str) -> Option<String> {
    if app.identity_token.is_empty() {
        return None;
    }
    session::sign_message(&app.identity_token, sender_id, ts, content)
        .ok()
        .flatten()
}

fn verify_message(app: &App, msg: &Message) -> bool {
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

fn send_message(app: &mut App, args: &Args, text: &str) {
    let channel = app.active_channel_name();
    if channel.is_empty() {
        app.set_status("No channel selected. Use /join #channel first.");
        return;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let signature = sign_message(app, &app.pubkey.clone(), ts, text);
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
    app.append_messages(&channel, vec![msg.clone()]);

    if !args.no_history && ensure_storage_capability(app) {
        let _ = api::append_message(&app.storage_token, &channel, &msg);
    }

    if app.peer_token.is_empty() && !ensure_peer_capability(app) {
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

fn poll_messages(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() {
        return;
    }

    let channels: Vec<String> = app
        .channel_names()
        .into_iter()
        .filter(|c| c.starts_with('#'))
        .collect();

    let mut nicks_changed = false;
    for topic in &channels {
        match session::recv_messages(&app.peer_token, topic, 50, "chat-wasm", None) {
            Ok(mut msgs) => {
                for msg in &mut msgs {
                    if msg.verified.is_none() {
                        msg.verified = Some(verify_message(app, msg));
                    }
                }
                for msg in msgs {
                    if is_own_message_instance(app, &msg) {
                        continue;
                    }

                    // Verification gate: skip unverified messages from unknown senders
                    let is_known = app.known_nicks.contains_key(&msg.sender_nick);
                    if msg.verified == Some(false) && !is_known {
                        continue;
                    }

                    let mut msg = msg;
                    msg.display_ts = Some(local_now_secs());

                    // Only attach peers for verified messages
                    if !msg.sender_id.is_empty() && msg.verified != Some(false) {
                        let key =
                            room_peer_key(topic, &msg.sender_id, msg.sender_session_id.as_deref());
                        app.attached_room_peers.insert(key.clone());
                        app.attach_retry_after.remove(&key);
                    }

                    // Only record nick->DID for verified senders
                    if !msg.sender_nick.is_empty()
                        && msg.sender_nick != "*"
                        && !msg.sender_id.is_empty()
                        && msg.verified != Some(false)
                        && !app.known_nicks.contains_key(&msg.sender_nick)
                    {
                        app.known_nicks
                            .insert(msg.sender_nick.clone(), msg.sender_id.clone());
                        nicks_changed = true;
                    }
                    if !args.no_history && ensure_storage_capability(app) {
                        let _ = api::append_message(&app.storage_token, topic, &msg);
                    }
                    app.append_messages(topic, vec![msg]);
                }
            }
            Err(_) => {}
        }
    }

    if nicks_changed {
        save_known_nicks(app, args);
    }
}

fn poll_peers(app: &mut App, args: &Args) {
    if app.peer_token.is_empty() {
        return;
    }
    let old_count = app.peer_count;
    if let Ok(peers) = session::list_peers(&app.peer_token) {
        app.peer_count = peers.len();
    }
    if app.p2p_connecting && app.peer_count > 0 {
        app.p2p_connecting = false;
        app.system_message(&format!("Connected to {} peer(s)", app.peer_count));
    }
    if app.peer_count > old_count && !args.no_sync {
        broadcast_history(app);
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
    format!("chat-wasm-presence:{}", room)
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
                        app.system_message(&format!("{} reached {} via Carrier", presence.nick, room));
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

fn broadcast_history(app: &App) {
    if app.peer_token.is_empty() {
        return;
    }
    for channel in app.channels.iter().filter(|c| c.name.starts_with('#')) {
        let recent: Vec<_> = channel
            .messages
            .iter()
            .rev()
            .take(50)
            .rev()
            .filter(|m| m.sender_nick != "*")
            .cloned()
            .collect();
        for msg in recent {
            let _ = session::send_gossip(
                &app.peer_token,
                &channel.name,
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

fn connect_to_peer(app: &mut App, _args: &Args, ticket: &str) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app) {
        app.system_message("Not connected to P2P network.");
        return;
    }
    match session::connect_peer(&app.peer_token, ticket) {
        Ok(_) => {
            app.direct_peer_mode = true;
            app.system_message("Connected to peer via ticket");
            attach_joined_channels_direct(app);
        }
        Err(e) => app.system_message(&format!("Connect failed: {}", e)),
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

fn show_ticket(app: &mut App, _args: &Args) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app) {
        app.system_message("Not connected to P2P network.");
        return;
    }
    match session::get_ticket(&app.peer_token) {
        Ok(Some(ticket)) => app.system_message(&format!("Your ticket: {}", ticket)),
        Ok(None) => app.system_message("Could not get ticket."),
        Err(e) => app.system_message(&format!("Ticket error: {}", e)),
    }
}

fn show_peers(app: &mut App, _args: &Args) {
    if app.peer_token.is_empty() && !ensure_peer_capability(app) {
        app.system_message("Not connected to P2P network.");
        return;
    }
    match session::list_peers(&app.peer_token) {
        Ok(peers) if peers.is_empty() => app.system_message("No peers connected."),
        Ok(peers) => {
            app.system_message(&format!("{} peer(s):", peers.len()));
            for peer in peers {
                app.system_message(&format!("  {}", peer));
            }
        }
        Err(e) => app.system_message(&format!("Peers error: {}", e)),
    }
}
