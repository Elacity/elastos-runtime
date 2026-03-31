use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{io::IsTerminal, io::Read, io::Write};

use elastos_server::sources::{default_data_dir, load_trusted_sources, TrustedSourcesConfig};
use sha2::{Digest, Sha256};

const CHAT_TOPIC: &str = "#general";
const PRESENCE_ATTACH_RETRY_BACKOFF: Duration = Duration::from_secs(12);

#[derive(Clone, Debug, Default)]
struct AttachedChatIdentity {
    did: String,
    nickname: Option<String>,
}

struct NativeChatCtx<'a> {
    client: &'a reqwest::Client,
    api: &'a str,
    token: &'a str,
    peer_cap: &'a str,
    did_cap: &'a str,
    self_did: &'a str,
    self_session_id: &'a str,
    nick: &'a str,
}

enum ChatSendOutcome {
    Delivered,
    LocalOnly,
    Error(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ChatPresenceAnnouncement {
    kind: String,
    room: String,
    did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    nick: String,
    ticket: String,
    ts: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeChatInputMode {
    Line,
    StdinTty,
    ControllingTty,
}

#[derive(Clone, Debug)]
enum ChatTerminalTarget {
    Stdout,
    #[cfg(unix)]
    ControllingTty,
    #[cfg(test)]
    Buffer(Arc<std::sync::Mutex<Vec<u8>>>),
}

#[derive(Clone, Debug)]
struct ChatTerminalUi {
    target: ChatTerminalTarget,
    current: Arc<std::sync::Mutex<String>>,
}

impl ChatTerminalUi {
    fn stdout() -> Self {
        Self {
            target: ChatTerminalTarget::Stdout,
            current: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    #[cfg(unix)]
    fn controlling_tty() -> Self {
        Self {
            target: ChatTerminalTarget::ControllingTty,
            current: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    #[cfg(test)]
    fn buffer(output: Arc<std::sync::Mutex<Vec<u8>>>) -> Self {
        Self {
            target: ChatTerminalTarget::Buffer(output),
            current: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    fn print_event(&self, line: &str) {
        let _ = self.with_output(|output, current| {
            let _ = write!(output, "\r\x1b[2K{}\r\n", line);
            if !current.is_empty() {
                let _ = write!(output, "> {}", current);
            }
            let _ = output.flush();
        });
    }

    fn push_char(&self, byte: u8) {
        let _ = self.with_output(|output, current| {
            current.push(byte as char);
            redraw_chat_input(output, current);
        });
    }

    fn backspace(&self) {
        let _ = self.with_output(|output, current| {
            if current.pop().is_some() {
                redraw_chat_input(output, current);
            }
        });
    }

    fn submit_line(&self) -> String {
        let mut line = String::new();
        let _ = self.with_output(|output, current| {
            line = current.trim().to_string();
            current.clear();
            let _ = write!(output, "\r\x1b[2K\r\n");
            let _ = output.flush();
        });
        line
    }

    fn cancel_line(&self) {
        let _ = self.with_output(|output, current| {
            current.clear();
            let _ = write!(output, "\r\x1b[2K\r\n");
            let _ = output.flush();
        });
    }

    fn with_output<T>(&self, f: impl FnOnce(&mut dyn Write, &mut String) -> T) -> Option<T> {
        let mut current = self.current.lock().ok()?;
        let mut output = self.open_output().ok()?;
        Some(f(&mut *output, &mut current))
    }

    fn open_output(&self) -> std::io::Result<Box<dyn Write>> {
        match self.target {
            ChatTerminalTarget::Stdout => Ok(Box::new(std::io::stdout())),
            #[cfg(unix)]
            ChatTerminalTarget::ControllingTty => {
                let tty = std::fs::OpenOptions::new()
                    .read(false)
                    .write(true)
                    .open("/dev/tty")?;
                Ok(Box::new(tty))
            }
            #[cfg(test)]
            ChatTerminalTarget::Buffer(ref output) => Ok(Box::new(TestBufferWriter {
                output: Arc::clone(output),
            })),
        }
    }
}

struct LoggingSuppressionGuard {
    previous: bool,
}

impl LoggingSuppressionGuard {
    fn enter() -> Self {
        Self {
            previous: crate::set_logging_suppressed(true),
        }
    }
}

impl Drop for LoggingSuppressionGuard {
    fn drop(&mut self) {
        crate::set_logging_suppressed(self.previous);
    }
}

#[cfg(test)]
struct TestBufferWriter {
    output: Arc<std::sync::Mutex<Vec<u8>>>,
}

#[cfg(test)]
impl Write for TestBufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut output) = self.output.lock() {
            output.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ChatBootstrap {
    ExplicitConnect(String),
    TrustedSourceSeed(String),
}

impl ChatBootstrap {
    fn gossip_join_mode(&self) -> &'static str {
        match self {
            // An explicit ticket is an intentional point-to-point bootstrap.
            Self::ExplicitConnect(_) => "direct",
            // A trusted source ticket provides automatic reachability bootstrap.
            // The chat room itself remains an iroh-gossip mesh that is extended
            // as peers rendezvous through Carrier.
            Self::TrustedSourceSeed(_) => "direct",
        }
    }

    fn status_mode_line(&self) -> &'static str {
        match self {
            Self::ExplicitConnect(_) => "Carrier mode: direct peer bootstrap.",
            Self::TrustedSourceSeed(_) => {
                "Carrier mode: trusted source reachability bootstrap with automatic Carrier rendezvous."
            }
        }
    }
}

fn new_chat_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!("{}:{}:{:p}", std::process::id(), now, &now);
    let digest = Sha256::digest(seed.as_bytes());
    hex::encode(digest)[..16].to_string()
}

fn native_room_consumer_id(room: &str, session_id: &str) -> String {
    format!("native-chat:{}:{}", room, session_id)
}

fn native_discovery_consumer_id(topic: &str, session_id: &str) -> String {
    format!("native-chat-discovery:{}:{}", topic, session_id)
}

pub async fn run_chat(nick: Option<String>, connect: Option<String>) -> anyhow::Result<()> {
    // Native chat client — connects to the running runtime's Carrier
    // via the provider API. No capsule, no VM, no separate Carrier node.
    //
    // One-terminal bootstrap: if no runtime is running, starts a managed
    // background runtime with a chat-safe policy. Requires `elastos setup`
    // to have provisioned the first-party PC2 core first.
    let data_dir = default_data_dir();
    let connect = resolve_chat_bootstrap(connect, &data_dir);
    let _logging_guard = LoggingSuppressionGuard::enter();

    let coords = crate::shell_cmd::ensure_runtime_for_chat(&data_dir).await?;
    run_native_chat_with_runtime(nick, connect, coords).await
}

pub(crate) async fn run_chat_from_pc2(
    nick: Option<String>,
    connect: Option<String>,
    coords: crate::shell_cmd::RuntimeCoords,
) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    let connect = resolve_chat_bootstrap(connect, &data_dir);
    let _logging_guard = LoggingSuppressionGuard::enter();

    run_native_chat_with_runtime(nick, connect, coords).await
}

async fn run_native_chat_with_runtime(
    requested_nick: Option<String>,
    connect: Option<ChatBootstrap>,
    coords: crate::shell_cmd::RuntimeCoords,
) -> anyhow::Result<()> {
    eprintln!("Connected to local runtime.");

    let tokens = crate::shell_cmd::attach_to_runtime(&coords).await?;
    let client = reqwest::Client::new();
    let api = &coords.api_url;
    let client_token = &tokens.client_token;
    let identity = load_attached_chat_identity(&client, api, client_token).await;
    let self_did = identity.did;
    let self_session_id = new_chat_session_id();
    let room_consumer_id = native_room_consumer_id(CHAT_TOPIC, &self_session_id);
    let nick = requested_nick
        .or(identity.nickname)
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "anon".into()));
    let peer_cap =
        request_attached_capability(&client, api, client_token, "elastos://peer/*", "execute")
            .await?;
    let did_cap =
        request_attached_capability(&client, api, client_token, "elastos://did/*", "execute")
            .await?;
    let using_bootstrap_rendezvous = connect.is_some();
    let discovery_topic = elastos_server::carrier::chat_discovery_topic(CHAT_TOPIC);
    let discovery_consumer_id = native_discovery_consumer_id(&discovery_topic, &self_session_id);
    let mut bootstrap_connected = 0usize;
    let input_mode = resolve_native_chat_input_mode();
    let tty_ui = match input_mode {
        NativeChatInputMode::Line => None,
        NativeChatInputMode::StdinTty => Some(ChatTerminalUi::stdout()),
        NativeChatInputMode::ControllingTty => {
            #[cfg(unix)]
            {
                Some(ChatTerminalUi::controlling_tty())
            }
            #[cfg(not(unix))]
            {
                None
            }
        }
    };

    if let Some(ref bootstrap) = connect {
        let (op, ticket, status_label) = match bootstrap {
            ChatBootstrap::ExplicitConnect(ticket) => ("connect", ticket.as_str(), "loaded"),
            ChatBootstrap::TrustedSourceSeed(ticket) => ("connect", ticket.as_str(), "loaded"),
        };
        match peer_provider_request(
            &client,
            api,
            client_token,
            &peer_cap,
            op,
            serde_json::json!({"ticket": ticket}),
        )
        .await
        {
            Ok(body) => {
                let added = body
                    .get("data")
                    .and_then(|d| d.get("added"))
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                bootstrap_connected = body
                    .get("data")
                    .and_then(|d| d.get("connected"))
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                eprintln!(
                    "Carrier bootstrap: {} {} peer endpoint(s), {} reachable now.",
                    status_label, added, bootstrap_connected
                );
            }
            Err(err) => {
                eprintln!("Carrier bootstrap failed: {}", err);
            }
        }
        eprintln!("{}", bootstrap.status_mode_line());
    } else {
        eprintln!("Carrier mode: DHT topic discovery.");
    }

    match peer_provider_request(
        &client,
        api,
        client_token,
        &peer_cap,
        "gossip_join",
        serde_json::json!({
            "topic": CHAT_TOPIC,
            "mode": connect
                .as_ref()
                .map(|bootstrap| bootstrap.gossip_join_mode())
                .unwrap_or("dht"),
        }),
    )
    .await
    {
        Ok(_) => {}
        Err(err) if is_already_joined_error(&err) => {}
        Err(err) => return Err(anyhow::anyhow!("Chat room join failed: {}", err)),
    }

    if using_bootstrap_rendezvous {
        match peer_provider_request(
            &client,
            api,
            client_token,
            &peer_cap,
            "gossip_join",
            serde_json::json!({
                "topic": &discovery_topic,
                "mode": "direct",
            }),
        )
        .await
        {
            Ok(_) => {}
            Err(err) if is_already_joined_error(&err) => {}
            Err(err) => {
                return Err(anyhow::anyhow!("Carrier rendezvous join failed: {}", err));
            }
        }
    }

    eprintln!("Chat room: {} joined.", CHAT_TOPIC);
    match connect.as_ref() {
        Some(ChatBootstrap::TrustedSourceSeed(_)) => {
            eprintln!(
                "Carrier reachability peers: {} connected.",
                bootstrap_connected
            );
            eprintln!("Chat room peers: 0 discovered yet.");
            eprintln!(
                "Delivery: local only until another participant is discovered and connected."
            );
        }
        _ => match list_connected_carrier_peers(&client, api, client_token, &peer_cap).await {
            Ok(peers) if peers.is_empty() => match connect.as_ref() {
                Some(ChatBootstrap::ExplicitConnect(_)) => {
                    eprintln!(
                        "Carrier peers: 0. Direct bootstrap loaded, but no active peers are connected yet."
                    );
                    eprintln!("Delivery: local only until the remote peer joins this room.");
                }
                Some(ChatBootstrap::TrustedSourceSeed(_)) => unreachable!(),
                None => {
                    eprintln!("Carrier peers: 0. Messages will stay local until a peer connects.");
                    eprintln!("Delivery: local only until a Carrier peer joins this room.");
                }
            },
            Ok(peers) => {
                eprintln!("Carrier peers: {} connected.", peers.len());
                match connect.as_ref() {
                    Some(ChatBootstrap::ExplicitConnect(_)) => {
                        eprintln!(
                            "Delivery: send a line to confirm the remote peer has joined this room."
                        );
                    }
                    Some(ChatBootstrap::TrustedSourceSeed(_)) => unreachable!(),
                    None => {
                        eprintln!(
                            "Delivery: send a line to confirm another DHT-discovered participant has joined this room."
                        );
                    }
                }
            }
            Err(err) => {
                eprintln!("Carrier peer status unavailable: {}", err);
                eprintln!("Delivery: unknown until peer status is available.");
            }
        },
    }

    eprintln!(
        "Chat as '{}' on {}. Type messages and press Enter.\nType /home to return to PC2, or /quit to exit.\n",
        nick, CHAT_TOPIC
    );

    let recv_client = client.clone();
    let recv_api = api.clone();
    let recv_token = client_token.clone();
    let recv_cap = peer_cap.clone();
    let recv_did_cap = did_cap.clone();
    let recv_did = self_did.clone();
    let recv_session_id = self_session_id.clone();
    let recv_consumer_id = room_consumer_id.clone();
    let recv_ui = tty_ui.clone();
    let attached_dids = Arc::new(tokio::sync::Mutex::new(HashSet::<String>::new()));
    let (quit_tx, quit_rx) = tokio::sync::watch::channel(false);
    let mut main_quit_rx = quit_rx.clone();
    let recv_attached_dids = Arc::clone(&attached_dids);
    tokio::spawn(async move {
        let mut known_nicks: HashMap<String, String> = HashMap::new();
        loop {
            tokio::select! {
                changed = main_quit_rx.changed() => {
                    if changed.is_ok() && *main_quit_rx.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
            if let Ok(resp) = recv_client
                .post(format!("{}/api/provider/peer/gossip_recv", recv_api))
                .header("Authorization", format!("Bearer {}", recv_token))
                .header("X-Capability-Token", &recv_cap)
                .json(&serde_json::json!({
                    "topic": CHAT_TOPIC,
                    "limit": 20,
                    "consumer_id": recv_consumer_id,
                }))
                .send()
                .await
            {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(messages) = body
                        .get("data")
                        .and_then(|d| d.get("messages"))
                        .and_then(|m| m.as_array())
                    {
                        for msg in messages {
                            if !should_render_incoming_message(msg, &recv_did, &recv_session_id) {
                                continue;
                            }

                            // Verify message signature — same contract as capsule chat
                            let sender_id = msg["sender_id"].as_str().unwrap_or("");
                            let sender_nick = msg["sender_nick"].as_str().unwrap_or("?");
                            let content = msg["content"].as_str().unwrap_or("");
                            let ts = msg["ts"].as_u64().unwrap_or(0);
                            let signature = msg["signature"].as_str().unwrap_or("");

                            let verified = verify_via_did_provider(
                                &recv_client,
                                &recv_api,
                                &recv_token,
                                &recv_did_cap,
                                sender_id,
                                ts,
                                content,
                                signature,
                            )
                            .await;

                            // Verification gate: skip unverified from unknown senders
                            let is_known = known_nicks.contains_key(sender_nick);
                            if !verified && !is_known {
                                eprintln!(
                                    "[chat] dropped unverified message from unknown sender '{}' (did={})",
                                    sender_nick, sender_id
                                );
                                continue;
                            }

                            // Only record nick→DID for verified senders
                            if verified && !sender_nick.is_empty() && !sender_id.is_empty() {
                                known_nicks
                                    .entry(sender_nick.to_string())
                                    .or_insert_with(|| sender_id.to_string());
                            }

                            // Only attach peers for verified messages
                            if verified {
                                if let Some(sid) = msg
                                    .get("sender_id")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                {
                                    recv_attached_dids
                                        .lock()
                                        .await
                                        .insert(attached_identity_key(sid, sender_session_id(msg)));
                                }
                            }

                            let badge = if verified {
                                "v"
                            } else if !signature.is_empty() {
                                "x"
                            } else {
                                "."
                            };
                            let rendered = format!("{} <{}> {}", badge, sender_nick, content);
                            if let Some(ui) = recv_ui.as_ref() {
                                ui.print_event(&rendered);
                            } else {
                                println!("{}", rendered);
                            }
                        }
                    }
                }
            }
        }
    });

    if using_bootstrap_rendezvous {
        let discover_client = client.clone();
        let discover_api = api.clone();
        let discover_token = client_token.clone();
        let discover_cap = peer_cap.clone();
        let discover_did_cap = did_cap.clone();
        let discover_did = self_did.clone();
        let discover_session_id = self_session_id.clone();
        let discover_consumer_id = discovery_consumer_id.clone();
        let discover_nick = nick.clone();
        let discover_topic = discovery_topic.clone();
        let discover_ui = tty_ui.clone();
        let discover_ticket = fetch_local_peer_ticket(&client, api, client_token, &peer_cap)
            .await
            .unwrap_or_default();
        let attach_retry_after =
            Arc::new(tokio::sync::Mutex::new(HashMap::<String, Instant>::new()));
        let mut discover_quit_rx = quit_rx.clone();
        tokio::spawn(async move {
            let mut announce_tick = tokio::time::interval(std::time::Duration::from_secs(3));
            let mut recv_tick = tokio::time::interval(std::time::Duration::from_millis(500));
            loop {
                tokio::select! {
                    changed = discover_quit_rx.changed() => {
                        if changed.is_ok() && *discover_quit_rx.borrow() {
                            break;
                        }
                    }
                    _ = announce_tick.tick() => {
                        if discover_ticket.is_empty() {
                            continue;
                        }
                        let _ = announce_chat_presence(
                            &discover_client,
                            &discover_api,
                            &discover_token,
                            &discover_cap,
                            &discover_did_cap,
                            &discover_did,
                            &discover_session_id,
                            &discover_nick,
                            &discover_ticket,
                            &discover_topic,
                        ).await;
                    }
                    _ = recv_tick.tick() => {
                        let _ = process_presence_messages(
                            &discover_client,
                            &discover_api,
                            &discover_token,
                            &discover_cap,
                            &discover_did_cap,
                            &discover_did,
                            &discover_session_id,
                            &discover_topic,
                            &discover_consumer_id,
                            Arc::clone(&attached_dids),
                            Arc::clone(&attach_retry_after),
                            discover_ui.clone(),
                        ).await;
                    }
                }
            }
        });
    }

    let stdin_lines = tokio::task::spawn_blocking({
        let client = client.clone();
        let api = api.clone();
        let token = client_token.clone();
        let peer_cap_clone = peer_cap.clone();
        let did_cap_clone = did_cap.clone();
        let self_did = self_did.clone();
        let nick = nick.clone();
        let tty_ui = tty_ui.clone();
        move || {
            let ctx = NativeChatCtx {
                client: &client,
                api: &api,
                token: &token,
                peer_cap: &peer_cap_clone,
                did_cap: &did_cap_clone,
                self_did: &self_did,
                self_session_id: &self_session_id,
                nick: &nick,
            };
            match input_mode {
                NativeChatInputMode::Line => native_chat_line_loop(&ctx),
                NativeChatInputMode::StdinTty => native_chat_tty_loop(
                    &ctx,
                    tty_ui
                        .as_ref()
                        .expect("stdin tty loop requires terminal ui"),
                ),
                NativeChatInputMode::ControllingTty => native_chat_controlling_tty_loop(
                    &ctx,
                    tty_ui
                        .as_ref()
                        .expect("controlling tty loop requires terminal ui"),
                )
                .unwrap_or_else(|| native_chat_line_loop(&ctx)),
            }
        }
    });

    let home_requested = stdin_lines.await?;
    let _ = quit_tx.send(true);
    if using_bootstrap_rendezvous {
        if let Err(err) =
            leave_chat_topic(&client, api, client_token, &peer_cap, &discovery_topic).await
        {
            eprintln!("[chat] cleanup failed: {}", err);
        }
    }
    if let Err(err) = leave_chat_topic(&client, api, client_token, &peer_cap, CHAT_TOPIC).await {
        eprintln!("[chat] cleanup failed: {}", err);
    }

    return_to_pc2_if_requested(home_requested)?;
    Ok(())
}

async fn leave_chat_topic(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    topic: &str,
) -> anyhow::Result<()> {
    match peer_provider_request(
        client,
        api,
        client_token,
        peer_cap,
        "gossip_leave",
        serde_json::json!({ "topic": topic }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(err)
            if err.to_string().contains("not joined")
                || err.to_string().contains("[not_joined]") =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

async fn fetch_local_peer_ticket(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
) -> anyhow::Result<String> {
    let body = peer_provider_request(
        client,
        api,
        client_token,
        peer_cap,
        "get_ticket",
        serde_json::json!({}),
    )
    .await?;
    body.get("data")
        .and_then(|d| d.get("ticket"))
        .and_then(|v| v.as_str())
        .map(|ticket| ticket.to_string())
        .ok_or_else(|| anyhow::anyhow!("peer ticket missing from Carrier provider"))
}
#[allow(clippy::too_many_arguments)]
async fn announce_chat_presence(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    did_cap: &str,
    self_did: &str,
    self_session_id: &str,
    nick: &str,
    ticket: &str,
    discovery_topic: &str,
) -> anyhow::Result<()> {
    let payload = ChatPresenceAnnouncement {
        kind: "chat_presence_v1".to_string(),
        room: CHAT_TOPIC.to_string(),
        did: self_did.to_string(),
        session_id: (!self_session_id.is_empty()).then_some(self_session_id.to_string()),
        nick: nick.to_string(),
        ticket: ticket.to_string(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let message = serde_json::to_string(&payload)?;
    let signature = sign_via_did_provider(
        client,
        api,
        client_token,
        did_cap,
        self_did,
        payload.ts,
        &message,
    )
    .await;
    let mut body = serde_json::json!({
        "topic": discovery_topic,
        "message": message,
        "sender": nick,
        "ts": payload.ts,
    });
    if !self_did.is_empty() {
        body["sender_id"] = serde_json::Value::String(self_did.to_string());
    }
    if !self_session_id.is_empty() {
        body["sender_session_id"] = serde_json::Value::String(self_session_id.to_string());
    }
    if let Some(sig) = signature {
        body["signature"] = serde_json::Value::String(sig);
    }
    let _ = peer_provider_request(client, api, client_token, peer_cap, "gossip_send", body).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_presence_messages(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    did_cap: &str,
    self_did: &str,
    self_session_id: &str,
    discovery_topic: &str,
    consumer_id: &str,
    attached_dids: Arc<tokio::sync::Mutex<HashSet<String>>>,
    attach_retry_after: Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
    tty_ui: Option<ChatTerminalUi>,
) -> anyhow::Result<()> {
    let body = peer_provider_request(
        client,
        api,
        client_token,
        peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": discovery_topic,
            "limit": 20,
            "consumer_id": consumer_id,
        }),
    )
    .await?;
    let Some(messages) = body
        .get("data")
        .and_then(|d| d.get("messages"))
        .and_then(|m| m.as_array())
    else {
        return Ok(());
    };
    for msg in messages {
        if !should_render_incoming_message(msg, self_did, self_session_id) {
            continue;
        }
        let Some(content) = msg.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(presence) = serde_json::from_str::<ChatPresenceAnnouncement>(content) else {
            continue;
        };
        if presence.kind != "chat_presence_v1"
            || presence.room != CHAT_TOPIC
            || presence.ticket.trim().is_empty()
            || is_same_chat_instance(
                self_did,
                self_session_id,
                &presence.did,
                presence.session_id.as_deref(),
            )
        {
            continue;
        }
        let sender_id = msg.get("sender_id").and_then(|v| v.as_str()).unwrap_or("");
        let ts = msg.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
        let signature = msg.get("signature").and_then(|v| v.as_str()).unwrap_or("");
        let verified = verify_via_did_provider(
            client,
            api,
            client_token,
            did_cap,
            sender_id,
            ts,
            content,
            signature,
        )
        .await;
        if !verified {
            continue;
        }
        let presence_key = attached_identity_key(&presence.did, presence.session_id.as_deref());
        if attached_dids.lock().await.contains(&presence_key) {
            continue;
        }
        let now = Instant::now();
        {
            let retry_after = attach_retry_after.lock().await;
            if presence_attach_retry_pending(&retry_after, &presence_key, now) {
                continue;
            }
        }

        match peer_provider_request(
            client,
            api,
            client_token,
            peer_cap,
            "remember_peer",
            serde_json::json!({"ticket": presence.ticket}),
        )
        .await
        {
            Ok(remember_body) => {
                let peer_ids: Vec<String> = remember_body
                    .get("data")
                    .and_then(|d| d.get("added"))
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if peer_ids.is_empty() {
                    {
                        let mut retry_after = attach_retry_after.lock().await;
                        schedule_presence_attach_retry(
                            &mut retry_after,
                            &presence_key,
                            Instant::now(),
                        );
                    }
                    render_chat_event(
                        tty_ui.as_ref(),
                        &format!(
                            "[chat] discovered '{}' and are waiting for a Carrier peer id.",
                            presence.nick
                        ),
                    );
                    continue;
                }
                let room_attached = attach_topic_direct_until_joined(
                    client,
                    api,
                    client_token,
                    peer_cap,
                    CHAT_TOPIC,
                    &peer_ids,
                )
                .await;
                let _ = attach_topic_direct_until_joined(
                    client,
                    api,
                    client_token,
                    peer_cap,
                    discovery_topic,
                    &peer_ids,
                )
                .await;
                match room_attached {
                    Ok(true) => {
                        attached_dids.lock().await.insert(presence_key.clone());
                        attach_retry_after.lock().await.remove(&presence_key);
                        render_chat_event(
                            tty_ui.as_ref(),
                            &format!(
                                "[chat] chat room peer attached: '{}' joined {} via Carrier.",
                                presence.nick, CHAT_TOPIC
                            ),
                        );
                    }
                    Ok(false) => {
                        {
                            let mut retry_after = attach_retry_after.lock().await;
                            schedule_presence_attach_retry(
                                &mut retry_after,
                                &presence_key,
                                Instant::now(),
                            );
                        }
                        render_chat_event(
                            tty_ui.as_ref(),
                            &format!(
                                "[chat] discovered '{}' and are waiting for room attach via Carrier.",
                                presence.nick
                            ),
                        );
                    }
                    Err(err) => {
                        {
                            let mut retry_after = attach_retry_after.lock().await;
                            schedule_presence_attach_retry(
                                &mut retry_after,
                                &presence_key,
                                Instant::now(),
                            );
                        }
                        render_chat_event(
                            tty_ui.as_ref(),
                            &format!(
                                "[chat] discovered '{}' but room attach failed: {}",
                                presence.nick, err
                            ),
                        );
                    }
                }
            }
            Err(err) => {
                {
                    let mut retry_after = attach_retry_after.lock().await;
                    schedule_presence_attach_retry(&mut retry_after, &presence_key, Instant::now());
                }
                render_chat_event(
                    tty_ui.as_ref(),
                    &format!(
                        "[chat] discovered '{}' but Carrier peer remember failed: {}",
                        presence.nick, err
                    ),
                );
            }
        }
    }
    Ok(())
}

async fn attach_room_peer_until_joined(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    topic: &str,
    peer_ids: &[String],
) -> anyhow::Result<bool> {
    for _ in 0..20 {
        let _ = peer_provider_request(
            client,
            api,
            client_token,
            peer_cap,
            "gossip_join_peers",
            serde_json::json!({
                "topic": topic,
                "peers": peer_ids,
            }),
        )
        .await;
        let topic_peers = list_topic_peers(client, api, client_token, peer_cap, topic).await?;
        if peer_ids
            .iter()
            .any(|peer_id| topic_peers.iter().any(|joined| joined == peer_id))
        {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    Ok(false)
}

async fn attach_topic_direct_until_joined(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    topic: &str,
    peer_ids: &[String],
) -> anyhow::Result<bool> {
    let _ = leave_chat_topic(client, api, client_token, peer_cap, topic).await;
    match peer_provider_request(
        client,
        api,
        client_token,
        peer_cap,
        "gossip_join",
        serde_json::json!({
            "topic": topic,
            "mode": "direct",
        }),
    )
    .await
    {
        Ok(_) => {}
        Err(err) if is_already_joined_error(&err) => {}
        Err(err) => {
            return Err(anyhow::anyhow!(
                "direct topic join failed for {}: {}",
                topic,
                err
            ));
        }
    }
    attach_room_peer_until_joined(client, api, client_token, peer_cap, topic, peer_ids).await
}

fn resolve_chat_bootstrap(
    explicit_connect: Option<String>,
    data_dir: &std::path::Path,
) -> Option<ChatBootstrap> {
    explicit_connect
        .map(ChatBootstrap::ExplicitConnect)
        .or_else(|| {
            let config = load_trusted_sources(data_dir).ok()?;
            bootstrap_ticket_from_config(&config).map(ChatBootstrap::TrustedSourceSeed)
        })
}

fn presence_attach_retry_pending(
    retry_after: &HashMap<String, Instant>,
    did: &str,
    now: Instant,
) -> bool {
    retry_after.get(did).is_some_and(|deadline| *deadline > now)
}

fn schedule_presence_attach_retry(
    retry_after: &mut HashMap<String, Instant>,
    did: &str,
    now: Instant,
) {
    retry_after.insert(did.to_string(), now + PRESENCE_ATTACH_RETRY_BACKOFF);
}

fn bootstrap_ticket_from_config(config: &TrustedSourcesConfig) -> Option<String> {
    let source = config.default_source()?;
    let ticket = source.connect_ticket.trim();
    if ticket.is_empty() {
        None
    } else {
        Some(ticket.to_string())
    }
}

async fn load_attached_chat_identity(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
) -> AttachedChatIdentity {
    let Ok(did_cap) =
        request_attached_capability(client, api, client_token, "elastos://did/*", "execute").await
    else {
        return AttachedChatIdentity::default();
    };

    let mut identity = AttachedChatIdentity::default();
    if let Ok(body) = did_provider_request(
        client,
        api,
        client_token,
        &did_cap,
        "get_did",
        serde_json::json!({}),
    )
    .await
    {
        if let Some(did) = body
            .get("data")
            .and_then(|d| d.get("did"))
            .and_then(|v| v.as_str())
        {
            identity.did = did.to_string();
        }
    }
    if let Ok(body) = did_provider_request(
        client,
        api,
        client_token,
        &did_cap,
        "get_nickname",
        serde_json::json!({}),
    )
    .await
    {
        identity.nickname = body
            .get("data")
            .and_then(|d| d.get("nickname"))
            .and_then(|v| v.as_str())
            .map(|nick| nick.trim().to_string())
            .filter(|nick| !nick.is_empty());
    }
    identity
}

async fn did_provider_request(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    did_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let resp = client
        .post(format!("{}/api/provider/did/{}", api, op))
        .header("Authorization", format!("Bearer {}", client_token))
        .header("X-Capability-Token", did_cap)
        .json(&body)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown did-provider error")
        );
    }
    Ok(body)
}

fn native_chat_line_loop(ctx: &NativeChatCtx<'_>) -> bool {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "/home" {
            return true;
        }
        if trimmed == "/quit" || trimmed == "/q" {
            return false;
        }
        report_chat_send_outcome(
            send_chat_message(
                ctx.client,
                ctx.api,
                ctx.token,
                ctx.peer_cap,
                ctx.did_cap,
                ctx.self_did,
                ctx.self_session_id,
                ctx.nick,
                &trimmed,
            ),
            ctx.nick,
            &trimmed,
            None,
        );
    }
    false
}

fn sender_session_id(msg: &serde_json::Value) -> Option<&str> {
    msg.get("sender_session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn attached_identity_key(did: &str, session_id: Option<&str>) -> String {
    match session_id.filter(|value| !value.is_empty()) {
        Some(session_id) => format!("{}|{}", did, session_id),
        None => did.to_string(),
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
    match sender_session_id.filter(|s| !s.is_empty()) {
        Some(sid) => sid == self_session_id,
        None => true,
    }
}

fn should_render_incoming_message(
    msg: &serde_json::Value,
    self_did: &str,
    self_session_id: &str,
) -> bool {
    if self_did.is_empty() {
        return true;
    }
    msg.get("sender_id")
        .and_then(|v| v.as_str())
        .map(|sender_id| {
            !is_same_chat_instance(self_did, self_session_id, sender_id, sender_session_id(msg))
        })
        .unwrap_or(true)
}

// Raw-mode host input lets Esc mean "return home" instead of printing literal ^[.
fn native_chat_tty_loop(ctx: &NativeChatCtx<'_>, ui: &ChatTerminalUi) -> bool {
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let _raw_mode = crate::shell_cmd::enable_host_raw_mode_pub();
    native_chat_tty_loop_from_io(ctx, &mut stdin, ui)
}

fn native_chat_tty_loop_from_io(
    ctx: &NativeChatCtx<'_>,
    input: &mut dyn Read,
    ui: &ChatTerminalUi,
) -> bool {
    loop {
        let mut byte = [0u8; 1];
        let read = match input.read(&mut byte) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if read == 0 {
            return false;
        }

        match byte[0] {
            b'\x1b' => {
                ui.cancel_line();
                return true;
            }
            b'\r' | b'\n' => {
                let line = ui.submit_line();
                if line.is_empty() {
                    continue;
                }
                if line == "/home" {
                    return true;
                }
                if line == "/quit" || line == "/q" {
                    return false;
                }
                report_chat_send_outcome(
                    send_chat_message(
                        ctx.client,
                        ctx.api,
                        ctx.token,
                        ctx.peer_cap,
                        ctx.did_cap,
                        ctx.self_did,
                        ctx.self_session_id,
                        ctx.nick,
                        &line,
                    ),
                    ctx.nick,
                    &line,
                    Some(ui),
                );
            }
            0x7f | 0x08 => {
                ui.backspace();
            }
            b if !b.is_ascii_control() => {
                ui.push_char(b);
            }
            _ => {}
        }
    }
}

#[cfg(unix)]
fn native_chat_controlling_tty_loop(ctx: &NativeChatCtx<'_>, ui: &ChatTerminalUi) -> Option<bool> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    if native_chat_debug_tty() {
        eprintln!("[chat-tty] attempting controlling-tty loop");
    }
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let tty_fd = tty.as_raw_fd();
    if native_chat_debug_tty() {
        eprintln!("[chat-tty] opened /dev/tty on fd {}", tty_fd);
    }
    if !foreground_process_group_owns_tty(tty_fd) {
        if native_chat_debug_tty() {
            eprintln!(
                "[chat-tty] skipping /dev/tty because foreground process group does not own it"
            );
        }
        return None;
    }
    let _raw_mode = enable_raw_mode_for_fd(tty_fd)?;
    if native_chat_debug_tty() {
        eprintln!(
            "[chat-tty] entering raw controlling-tty loop on fd {}",
            tty_fd
        );
    }
    Some(native_chat_tty_loop_from_io(ctx, &mut tty, ui))
}

#[cfg(unix)]
fn foreground_process_group_owns_tty(fd: std::os::fd::RawFd) -> bool {
    unsafe {
        let tty_pgrp = libc::tcgetpgrp(fd);
        let proc_pgrp = libc::getpgrp();
        let owns = tty_pgrp > 0 && tty_pgrp == proc_pgrp;
        if native_chat_debug_tty() {
            eprintln!(
                "[chat-tty] tty_pgrp={} proc_pgrp={} owns={}",
                tty_pgrp, proc_pgrp, owns
            );
        }
        owns
    }
}

#[cfg(not(unix))]
fn native_chat_controlling_tty_loop(
    _ctx: &NativeChatCtx<'_>,
    _ui: &ChatTerminalUi,
) -> Option<bool> {
    None
}

#[cfg(unix)]
struct FdTermiosGuard {
    fd: std::os::fd::RawFd,
    saved: libc::termios,
}

#[cfg(unix)]
impl Drop for FdTermiosGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

#[cfg(unix)]
fn enable_raw_mode_for_fd(fd: std::os::fd::RawFd) -> Option<FdTermiosGuard> {
    unsafe {
        let mut original: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut original) != 0 {
            return None;
        }

        let mut raw = original;
        raw.c_iflag &= !(libc::ICRNL
            | libc::IXON
            | libc::BRKINT
            | libc::INPCK
            | libc::ISTRIP
            | libc::IXOFF
            | libc::INLCR
            | libc::IGNCR
            | libc::PARMRK);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }

        Some(FdTermiosGuard {
            fd,
            saved: original,
        })
    }
}

fn native_chat_prefers_controlling_tty() -> bool {
    if native_chat_force_stdin() {
        return false;
    }
    native_chat_prefers_controlling_tty_for_surface(
        std::env::var("ELASTOS_PARENT_SURFACE").ok().as_deref(),
    )
}

fn resolve_native_chat_input_mode() -> NativeChatInputMode {
    if native_chat_force_stdin() {
        return if std::io::stdin().is_terminal() {
            NativeChatInputMode::StdinTty
        } else {
            NativeChatInputMode::Line
        };
    }

    if native_chat_prefers_controlling_tty() {
        if controlling_tty_is_available() {
            return NativeChatInputMode::ControllingTty;
        }
        return if std::io::stdin().is_terminal() {
            NativeChatInputMode::StdinTty
        } else {
            NativeChatInputMode::Line
        };
    }

    if std::io::stdin().is_terminal() {
        NativeChatInputMode::StdinTty
    } else if controlling_tty_is_available() {
        NativeChatInputMode::ControllingTty
    } else {
        NativeChatInputMode::Line
    }
}

fn native_chat_force_stdin() -> bool {
    matches!(
        std::env::var("ELASTOS_CHAT_FORCE_STDIN").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn native_chat_debug_tty() -> bool {
    matches!(
        std::env::var("ELASTOS_CHAT_DEBUG_TTY").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn native_chat_prefers_controlling_tty_for_surface(parent_surface: Option<&str>) -> bool {
    parent_surface == Some("pc2")
}

#[cfg(unix)]
fn controlling_tty_is_available() -> bool {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    let Ok(tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        return false;
    };
    foreground_process_group_owns_tty(tty.as_raw_fd())
}

#[cfg(not(unix))]
fn controlling_tty_is_available() -> bool {
    false
}

/// SHA-256 of "sender_id:ts:content" — delegates to shared chat protocol.
fn signing_payload_hex(sender_id: &str, ts: u64, content: &str) -> String {
    elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content)
}

/// Sign a message via the DID provider HTTP API. Returns hex signature or None.
async fn sign_via_did_provider(
    client: &reqwest::Client,
    api: &str,
    bearer: &str,
    did_cap: &str,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> Option<String> {
    let payload_hex = signing_payload_hex(sender_id, ts, content);
    let resp = client
        .post(format!("{}/api/provider/did/sign", api))
        .header("Authorization", format!("Bearer {}", bearer))
        .header("X-Capability-Token", did_cap)
        .json(&serde_json::json!({"data": payload_hex}))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("data")
        .and_then(|d| d.get("signature"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn verify_via_did_provider(
    client: &reqwest::Client,
    api: &str,
    bearer: &str,
    did_cap: &str,
    sender_id: &str,
    ts: u64,
    content: &str,
    signature: &str,
) -> bool {
    if signature.is_empty() || sender_id.is_empty() || ts == 0 {
        return false;
    }
    let payload_hex = signing_payload_hex(sender_id, ts, content);
    did_provider_request(
        client,
        api,
        bearer,
        did_cap,
        "verify",
        serde_json::json!({
            "did": sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    )
    .await
    .ok()
    .and_then(|body| {
        body.get("data")
            .and_then(|d| d.get("valid"))
            .and_then(|v| v.as_bool())
    })
    .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn send_chat_message(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    peer_cap: &str,
    did_cap: &str,
    self_did: &str,
    self_session_id: &str,
    nick: &str,
    message: &str,
) -> ChatSendOutcome {
    let rt = tokio::runtime::Handle::current();
    rt.block_on(async move {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let signature =
            sign_via_did_provider(client, api, token, did_cap, self_did, ts, message).await;
        let mut payload = serde_json::json!({
            "topic": CHAT_TOPIC,
            "message": message,
            "sender": nick,
            "ts": ts,
        });
        if !self_did.is_empty() {
            payload["sender_id"] = serde_json::Value::String(self_did.to_string());
        }
        if !self_session_id.is_empty() {
            payload["sender_session_id"] = serde_json::Value::String(self_session_id.to_string());
        }
        if let Some(sig) = signature {
            payload["signature"] = serde_json::Value::String(sig);
        }
        match client
            .post(format!("{}/api/provider/peer/gossip_send", api))
            .header("Authorization", format!("Bearer {}", token))
            .header("X-Capability-Token", peer_cap)
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body)
                    if body.get("status").and_then(|s| s.as_str()) == Some("ok")
                        && body.get("broadcast").and_then(|s| s.as_str()) == Some("local_only") =>
                {
                    ChatSendOutcome::LocalOnly
                }
                Ok(body) if body.get("status").and_then(|s| s.as_str()) == Some("ok") => {
                    ChatSendOutcome::Delivered
                }
                Ok(body) => ChatSendOutcome::Error(
                    body.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown chat send failure")
                        .to_string(),
                ),
                Err(err) => ChatSendOutcome::Error(err.to_string()),
            },
            Err(err) => ChatSendOutcome::Error(err.to_string()),
        }
    })
}

fn report_chat_send_outcome(
    outcome: ChatSendOutcome,
    nick: &str,
    message: &str,
    tty_ui: Option<&ChatTerminalUi>,
) {
    match outcome {
        ChatSendOutcome::Delivered => {
            render_chat_message(tty_ui, nick, message);
        }
        ChatSendOutcome::LocalOnly => {
            render_chat_message(tty_ui, nick, message);
            render_chat_event(
                tty_ui,
                "[chat] message stayed local; no other participant in this chat room is currently reachable.",
            );
        }
        ChatSendOutcome::Error(err) => {
            render_chat_event(tty_ui, &format!("[chat] send failed: {}", err));
        }
    }
}

fn render_chat_message(tty_ui: Option<&ChatTerminalUi>, sender: &str, content: &str) {
    let rendered = format!("<{}> {}", sender, content);
    if let Some(ui) = tty_ui {
        ui.print_event(&rendered);
    } else {
        println!("{}", rendered);
    }
}

fn render_chat_event(tty_ui: Option<&ChatTerminalUi>, line: &str) {
    if let Some(ui) = tty_ui {
        ui.print_event(line);
    } else {
        eprintln!("{}", line);
    }
}

fn redraw_chat_input(output: &mut dyn Write, current: &str) {
    let _ = write!(output, "\r\x1b[2K");
    if !current.is_empty() {
        let _ = write!(output, "> {}", current);
    }
    let _ = output.flush();
}

async fn peer_provider_request(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let resp = client
        .post(format!("{}/api/provider/peer/{}", api, op))
        .header("Authorization", format!("Bearer {}", token))
        .header("X-Capability-Token", cap)
        .json(&body)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown peer-provider error")
        );
    }
    Ok(body)
}

fn is_already_joined_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("already joined")
}

async fn list_connected_carrier_peers(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    cap: &str,
) -> anyhow::Result<Vec<String>> {
    let body =
        peer_provider_request(client, api, token, cap, "list_peers", serde_json::json!({})).await?;
    Ok(body
        .get("data")
        .and_then(|d| d.get("peers"))
        .and_then(|v| v.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default())
}

async fn list_topic_peers(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    cap: &str,
    topic: &str,
) -> anyhow::Result<Vec<String>> {
    let body = peer_provider_request(
        client,
        api,
        token,
        cap,
        "list_topic_peers",
        serde_json::json!({ "topic": topic }),
    )
    .await?;
    Ok(body
        .get("data")
        .and_then(|d| d.get("peers"))
        .and_then(|v| v.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default())
}

fn return_to_pc2_if_requested(home_requested: bool) -> anyhow::Result<()> {
    if !home_requested {
        return Ok(());
    }
    if std::env::var("ELASTOS_PARENT_SURFACE").ok().as_deref() == Some("pc2") {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let status = std::process::Command::new(exe).arg("pc2").status()?;
    if !status.success() {
        anyhow::bail!(
            "failed to return to PC2 (exit {})",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string())
        );
    }
    Ok(())
}

pub(crate) async fn request_attached_capability(
    client: &reqwest::Client,
    api: &str,
    client_token: &str,
    resource: &str,
    action: &str,
) -> anyhow::Result<String> {
    let resp = client
        .post(format!("{}/api/capability/request", api))
        .header("Authorization", format!("Bearer {}", client_token))
        .json(&serde_json::json!({
            "resource": resource,
            "action": action,
        }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;

    if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
        return Ok(token.to_string());
    }

    if body.get("status").and_then(|s| s.as_str()) == Some("denied") {
        anyhow::bail!(
            "Capability denied: {}",
            body.get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("denied")
        );
    }

    let request_id = body
        .get("request_id")
        .and_then(|r| r.as_str())
        .ok_or_else(|| anyhow::anyhow!("capability response missing request_id"))?;

    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let resp = client
            .get(format!("{}/api/capability/request/{}", api, request_id))
            .header("Authorization", format!("Bearer {}", client_token))
            .send()
            .await?;
        let status: serde_json::Value = resp.json().await?;

        if let Some(token) = status.get("token").and_then(|t| t.as_str()) {
            return Ok(token.to_string());
        }

        match status.get("status").and_then(|s| s.as_str()) {
            Some("denied") | Some("expired") => {
                anyhow::bail!(
                    "Capability {}: {}",
                    status
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("error"),
                    status
                        .get("reason")
                        .and_then(|r| r.as_str())
                        .unwrap_or("request failed")
                );
            }
            _ => {}
        }
    }

    anyhow::bail!("Capability request still pending after 3s")
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_server::sources::{TrustedSource, TrustedSourcesConfig};
    use std::io::Cursor;
    use std::sync::Mutex;

    #[test]
    fn bootstrap_ticket_prefers_default_source_ticket() {
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "default".to_string(),
            sources: vec![TrustedSource {
                name: "default".to_string(),
                publisher_dids: vec![],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: "seed-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };
        assert_eq!(
            bootstrap_ticket_from_config(&config),
            Some("seed-ticket".to_string())
        );
    }

    #[test]
    fn resolve_chat_bootstrap_prefers_explicit_connect_over_source_ticket() {
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "default".to_string(),
            sources: vec![TrustedSource {
                name: "default".to_string(),
                publisher_dids: vec![],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: "seed-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };

        let resolved =
            Some(ChatBootstrap::ExplicitConnect("manual-ticket".to_string())).or_else(|| {
                bootstrap_ticket_from_config(&config).map(ChatBootstrap::TrustedSourceSeed)
            });

        assert_eq!(
            resolved,
            Some(ChatBootstrap::ExplicitConnect("manual-ticket".to_string()))
        );
    }

    #[test]
    fn resolve_chat_bootstrap_marks_source_ticket_as_seed() {
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "default".to_string(),
            sources: vec![TrustedSource {
                name: "default".to_string(),
                publisher_dids: vec![],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: "seed-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };

        let resolved = None::<String>
            .map(ChatBootstrap::ExplicitConnect)
            .or_else(|| {
                bootstrap_ticket_from_config(&config).map(ChatBootstrap::TrustedSourceSeed)
            });

        assert_eq!(
            resolved,
            Some(ChatBootstrap::TrustedSourceSeed("seed-ticket".to_string()))
        );
    }

    #[test]
    fn explicit_connect_uses_direct_join_mode() {
        assert_eq!(
            ChatBootstrap::ExplicitConnect("manual-ticket".to_string()).gossip_join_mode(),
            "direct"
        );
    }

    #[test]
    fn source_seed_uses_direct_join_mode() {
        assert_eq!(
            ChatBootstrap::TrustedSourceSeed("seed-ticket".to_string()).gossip_join_mode(),
            "direct"
        );
    }

    #[test]
    fn native_room_consumer_id_is_session_scoped() {
        assert_eq!(
            native_room_consumer_id("#general", "abc123"),
            "native-chat:#general:abc123"
        );
        assert_ne!(
            native_room_consumer_id("#general", "abc123"),
            native_room_consumer_id("#general", "def456")
        );
    }

    #[test]
    fn native_discovery_consumer_id_is_session_scoped() {
        assert_eq!(
            native_discovery_consumer_id("__elastos_internal/chat-presence-v1/#general", "abc123"),
            "native-chat-discovery:__elastos_internal/chat-presence-v1/#general:abc123"
        );
        assert_ne!(
            native_discovery_consumer_id("__elastos_internal/chat-presence-v1/#general", "abc123"),
            native_discovery_consumer_id("__elastos_internal/chat-presence-v1/#general", "def456")
        );
    }

    #[test]
    fn bootstrap_ticket_ignores_empty_ticket() {
        let config = TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "default".to_string(),
            sources: vec![TrustedSource {
                name: "default".to_string(),
                publisher_dids: vec![],
                channel: "jetson-test".to_string(),
                discovery_uri: String::new(),
                connect_ticket: "   ".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        };
        assert_eq!(bootstrap_ticket_from_config(&config), None);
    }

    #[test]
    fn presence_attach_retry_backoff_suppresses_repeat_attempts() {
        let now = Instant::now();
        let mut retry_after = HashMap::new();

        assert!(!presence_attach_retry_pending(
            &retry_after,
            "did:key:peer",
            now
        ));

        schedule_presence_attach_retry(&mut retry_after, "did:key:peer", now);

        assert!(presence_attach_retry_pending(
            &retry_after,
            "did:key:peer",
            now + Duration::from_secs(1)
        ));
        assert!(!presence_attach_retry_pending(
            &retry_after,
            "did:key:peer",
            now + PRESENCE_ATTACH_RETRY_BACKOFF + Duration::from_secs(1)
        ));
    }

    #[test]
    fn tty_loop_quit_returns_false() {
        let client = reqwest::Client::new();
        let mut input = Cursor::new(b"/quit\r".to_vec());
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));
        let ctx = NativeChatCtx {
            client: &client,
            api: "http://127.0.0.1",
            token: "token",
            peer_cap: "cap",
            did_cap: "cap",
            self_did: "",
            self_session_id: "",
            nick: "nick",
        };

        let home_requested = native_chat_tty_loop_from_io(&ctx, &mut input, &ui);

        assert!(!home_requested);
    }

    #[test]
    fn tty_loop_home_returns_true() {
        let client = reqwest::Client::new();
        let mut input = Cursor::new(b"/home\r".to_vec());
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));
        let ctx = NativeChatCtx {
            client: &client,
            api: "http://127.0.0.1",
            token: "token",
            peer_cap: "cap",
            did_cap: "cap",
            self_did: "",
            self_session_id: "",
            nick: "nick",
        };

        let home_requested = native_chat_tty_loop_from_io(&ctx, &mut input, &ui);

        assert!(home_requested);
    }

    #[test]
    fn tty_loop_escape_returns_true() {
        let client = reqwest::Client::new();
        let mut input = Cursor::new(vec![0x1b]);
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));
        let ctx = NativeChatCtx {
            client: &client,
            api: "http://127.0.0.1",
            token: "token",
            peer_cap: "cap",
            did_cap: "cap",
            self_did: "",
            self_session_id: "",
            nick: "nick",
        };

        let home_requested = native_chat_tty_loop_from_io(&ctx, &mut input, &ui);

        assert!(home_requested);
    }

    #[test]
    fn tty_loop_ignores_leading_newlines_before_home() {
        let client = reqwest::Client::new();
        let mut input = Cursor::new(b"\r\n/home\r".to_vec());
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));
        let ctx = NativeChatCtx {
            client: &client,
            api: "http://127.0.0.1",
            token: "token",
            peer_cap: "cap",
            did_cap: "cap",
            self_did: "",
            self_session_id: "",
            nick: "nick",
        };

        let home_requested = native_chat_tty_loop_from_io(&ctx, &mut input, &ui);

        assert!(home_requested);
    }

    #[test]
    fn tty_loop_ignores_leading_newlines_before_quit() {
        let client = reqwest::Client::new();
        let mut input = Cursor::new(b"\r\n/quit\r".to_vec());
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));
        let ctx = NativeChatCtx {
            client: &client,
            api: "http://127.0.0.1",
            token: "token",
            peer_cap: "cap",
            did_cap: "cap",
            self_did: "",
            self_session_id: "",
            nick: "nick",
        };

        let home_requested = native_chat_tty_loop_from_io(&ctx, &mut input, &ui);

        assert!(!home_requested);
    }

    #[test]
    fn tty_ui_event_redraws_current_input_without_spacing_gap() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));

        ui.push_char(b'h');
        ui.push_char(b'i');
        ui.print_event("<peer> hello");

        let rendered = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(rendered.contains("\r\x1b[2K<peer> hello\r\n> hi"));
    }

    #[test]
    fn delivered_send_echoes_local_message() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));

        report_chat_send_outcome(ChatSendOutcome::Delivered, "anders", "hello", Some(&ui));

        let rendered = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(rendered.contains("<anders> hello"));
    }

    #[test]
    fn local_only_send_echoes_message_and_notice() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let ui = ChatTerminalUi::buffer(Arc::clone(&output));

        report_chat_send_outcome(ChatSendOutcome::LocalOnly, "anders", "hello", Some(&ui));

        let rendered = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(rendered.contains("<anders> hello"));
        assert!(rendered.contains("message stayed local"));
    }

    #[test]
    fn pc2_parent_surface_prefers_controlling_tty() {
        assert!(native_chat_prefers_controlling_tty_for_surface(Some("pc2")));
        assert!(!native_chat_prefers_controlling_tty_for_surface(Some(
            "shell"
        )));
        assert!(!native_chat_prefers_controlling_tty_for_surface(None));
    }

    #[test]
    fn force_stdin_env_disables_controlling_tty_preference() {
        unsafe { std::env::set_var("ELASTOS_CHAT_FORCE_STDIN", "1") };
        assert!(!native_chat_prefers_controlling_tty());
        unsafe { std::env::remove_var("ELASTOS_CHAT_FORCE_STDIN") };
    }

    #[test]
    fn incoming_message_from_same_nick_but_different_did_is_rendered() {
        let msg = serde_json::json!({
            "sender_id": "did:key:peer",
            "sender_nick": "anders",
            "content": "hello",
        });
        assert!(should_render_incoming_message(
            &msg,
            "did:key:self",
            "session-self"
        ));
    }

    #[test]
    fn incoming_message_from_same_did_and_same_session_is_not_rendered() {
        let msg = serde_json::json!({
            "sender_id": "did:key:self",
            "sender_session_id": "session-self",
            "sender_nick": "anders",
            "content": "hello",
        });
        assert!(!should_render_incoming_message(
            &msg,
            "did:key:self",
            "session-self"
        ));
    }

    #[test]
    fn incoming_message_from_same_did_but_different_session_is_rendered() {
        let msg = serde_json::json!({
            "sender_id": "did:key:self",
            "sender_session_id": "session-irc",
            "sender_nick": "wsl-irc",
            "content": "hello",
        });
        assert!(should_render_incoming_message(
            &msg,
            "did:key:self",
            "session-native"
        ));
    }
}
