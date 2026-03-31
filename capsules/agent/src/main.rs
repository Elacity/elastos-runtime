//! ElastOS AI Agent — headless P2P chat agent that responds via LLM.
//!
//! Joins gossip channels and responds to @mentions (or all messages with --respond-all).

mod api;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AGENT_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const COOLDOWN: Duration = Duration::from_secs(2);
const MAX_INPUT_CHARS: usize = 2000;
const DEDUP_CAPACITY: usize = 1000;
const WARMUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const WARMUP_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    sender_id: String,
    sender_nick: String,
    content: String,
    ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

/// DM marker prefix (same as chat capsule)
const DM_PREFIX: &str = "\x01DM:";

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

struct Args {
    nick: String,
    channel: String,
    backend: String,
    respond_all: bool,
    connect: Option<String>,
    api_url: String,
    session_token: String,
}

fn parse_args() -> Result<Args> {
    let api_url =
        std::env::var("ELASTOS_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    let session_token = std::env::var("ELASTOS_TOKEN").unwrap_or_default();

    let args: Vec<String> = std::env::args().collect();
    let mut nick = "agent".to_string();
    let mut channel = "#general".to_string();
    let mut backend = "local".to_string();
    let mut respond_all = false;
    let mut connect = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--nick" | "-n" => {
                i += 1;
                if i < args.len() {
                    nick = args[i].clone();
                }
            }
            "--channel" => {
                i += 1;
                if i < args.len() {
                    channel = args[i].clone();
                }
            }
            "--backend" => {
                i += 1;
                if i < args.len() {
                    backend = args[i].clone();
                }
            }
            "--connect" | "-c" => {
                i += 1;
                if i < args.len() {
                    connect = Some(args[i].clone());
                }
            }
            "--respond-all" => {
                respond_all = true;
            }
            _ => {}
        }
        i += 1;
    }

    // Supervisor-mode launch payload: shell forwards CLI flags via ELASTOS_COMMAND.
    if let Some(payload) = forwarded_command_payload() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
            if v.get("command").and_then(|c| c.as_str()) == Some("agent") {
                if let Some(n) = v.get("nick").and_then(|n| n.as_str()) {
                    if !n.is_empty() {
                        nick = n.to_string();
                    }
                }
                if let Some(ch) = v.get("channel").and_then(|c| c.as_str()) {
                    if !ch.is_empty() {
                        channel = ch.to_string();
                    }
                }
                if let Some(b) = v.get("backend").and_then(|b| b.as_str()) {
                    if !b.is_empty() {
                        backend = b.to_string();
                    }
                }
                connect = v
                    .get("connect")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());
                if let Some(b) = v.get("respond_all").and_then(|b| b.as_bool()) {
                    respond_all = b;
                }
            }
        }
    }

    Ok(Args {
        nick,
        channel,
        backend,
        respond_all,
        connect,
        api_url,
        session_token,
    })
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn dedup_key(sender_id: &str, sender_nick: &str, ts: u64, content: &str) -> String {
    elastos_common::chat_protocol::dedup_key(sender_id, sender_nick, ts, content)
}

fn signing_payload_hex(sender_id: &str, ts: u64, content: &str) -> String {
    elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content)
}

/// Sign a message via the DID provider. Returns the hex signature or None on failure.
fn sign_message(args: &Args, did_token: &str, sender_id: &str, ts: u64, content: &str) -> Option<String> {
    let payload_hex = signing_payload_hex(sender_id, ts, content);
    match api::provider_call(
        &args.api_url,
        &args.session_token,
        did_token,
        "did",
        "sign",
        &serde_json::json!({"data": payload_hex}),
    ) {
        Ok(resp) => resp
            .get("data")
            .and_then(|d| d.get("signature"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        Err(e) => {
            eprintln!("Warning: failed to sign message: {}", e);
            None
        }
    }
}

/// Verify a message signature via the DID provider. Returns true if verified.
fn verify_message(args: &Args, did_token: &str, msg: &Message) -> bool {
    let signature = match &msg.signature {
        Some(sig) if !sig.is_empty() => sig,
        _ => return false, // unsigned
    };
    if msg.sender_id.is_empty() {
        return false;
    }
    let payload_hex = signing_payload_hex(&msg.sender_id, msg.ts, &msg.content);
    match api::provider_call(
        &args.api_url,
        &args.session_token,
        did_token,
        "did",
        "verify",
        &serde_json::json!({
            "did": msg.sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    ) {
        Ok(resp) => resp
            .get("data")
            .and_then(|d| d.get("valid"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Check if a message should trigger an AI response.
///
/// Returns true if `respond_all` is set, or if the message contains `@nick` (case-insensitive).
fn should_trigger(content: &str, nick: &str, respond_all: bool) -> bool {
    if respond_all {
        return true;
    }
    let content_lower = content.to_lowercase();
    let nick_lower = nick.to_lowercase();
    content_lower.contains(&format!("@{}", nick_lower))
}

/// Check if a message is from this exact agent instance (should be skipped).
///
/// In shared-runtime mode, multiple capsules can share the same DID.
/// We therefore require both DID and nick to match to avoid dropping
/// messages from other local clients (e.g. chat user "alice").
fn is_own_message(sender_id: &str, sender_nick: &str, own_did: &str, own_nick: &str) -> bool {
    !own_did.is_empty() && sender_id == own_did && sender_nick.eq_ignore_ascii_case(own_nick)
}

/// Check if a message is a DM (private message, agent ignores these).
fn is_dm(content: &str) -> bool {
    content.starts_with(DM_PREFIX)
}

fn build_system_prompt(nick: &str) -> String {
    format!(
        "You are {}, a helpful AI assistant in an ElastOS P2P chat. \
         Keep responses concise (1-3 sentences). Be friendly and helpful. \
         You're replying inside a decentralized chat runtime, so answer as a chat participant rather than as a formal support bot.",
        nick
    )
}

fn call_ai(
    args: &Args,
    ai_token: &str,
    system_prompt: &str,
    user_content: &str,
) -> Result<String> {
    let truncated = if user_content.len() > MAX_INPUT_CHARS {
        &user_content[..MAX_INPUT_CHARS]
    } else {
        user_content
    };

    let resp = api::provider_call(
        &args.api_url,
        &args.session_token,
        ai_token,
        "ai",
        "chat_completions",
        &serde_json::json!({
            "backend": args.backend,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": truncated},
            ]
        }),
    )?;

    // Extract response text from OpenAI-compatible format
    let text = resp
        .get("data")
        .and_then(|d| d.get("choices"))
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("[empty response]")
        .to_string();

    Ok(text)
}

/// Check if an AI error is a transient model-loading state (should retry).
fn is_warmup_error(err: &str) -> bool {
    err.contains("[model_loading]") || err.contains("[local_unavailable]")
}

/// Call AI with automatic retry during model warmup.
/// On first transient failure, sends a warmup notice to chat, then retries
/// until the model is ready or timeout is reached.
fn call_ai_with_warmup(
    args: &Args,
    ai_token: &str,
    peer_token: &str,
    did_token: &str,
    system_prompt: &str,
    user_content: &str,
    own_did: &str,
) -> String {
    match call_ai(args, ai_token, system_prompt, user_content) {
        Ok(text) => return text,
        Err(e) => {
            let err_str = e.to_string();
            if !is_warmup_error(&err_str) {
                eprintln!("AI error: {}", err_str);
                return "[AI unavailable, try again]".to_string();
            }
            eprintln!("Model loading, starting warmup retry loop...");
        }
    }

    // Transient error — send warmup notice and retry
    let warmup_msg = "Local model warming up (first run can take a few minutes)...";
    eprintln!("<{}> {}", args.nick, warmup_msg);
    gossip_send(args, peer_token, did_token, &args.channel, warmup_msg, own_did);

    let start = Instant::now();
    loop {
        std::thread::sleep(WARMUP_RETRY_INTERVAL);

        if start.elapsed() > WARMUP_TIMEOUT {
            eprintln!("Warmup timeout after {}s", WARMUP_TIMEOUT.as_secs());
            return "[AI model failed to load — check logs, run: elastos setup --with llama-server]".to_string();
        }

        match call_ai(args, ai_token, system_prompt, user_content) {
            Ok(text) => {
                eprintln!(
                    "Model ready after {:.0}s warmup",
                    start.elapsed().as_secs_f64()
                );
                return text;
            }
            Err(e) => {
                let err_str = e.to_string();
                if is_warmup_error(&err_str) {
                    eprintln!(
                        "Still warming up ({:.0}s elapsed)...",
                        start.elapsed().as_secs_f64()
                    );
                    continue;
                }
                // Non-transient error during retry
                eprintln!("AI error during warmup retry: {}", err_str);
                return "[AI unavailable, try again]".to_string();
            }
        }
    }
}

fn gossip_send(
    args: &Args,
    peer_token: &str,
    did_token: &str,
    topic: &str,
    content: &str,
    own_did: &str,
) {
    let ts = now_ts();
    let signature = sign_message(args, did_token, own_did, ts, content);
    if let Err(e) = api::provider_call(
        &args.api_url,
        &args.session_token,
        peer_token,
        "peer",
        "gossip_send",
        &serde_json::json!({
            "topic": topic,
            "message": content,
            "sender": args.nick,
            "sender_id": own_did,
            "ts": ts,
            "signature": signature,
        }),
    ) {
        eprintln!("gossip_send failed: {}", e);
    }
}

fn main() -> Result<()> {
    eprintln!("agent: starting v{}", AGENT_VERSION);
    let args = parse_args()?;

    if args.session_token.is_empty() {
        eprintln!("Error: ELASTOS_TOKEN not set. Agent must run under the runtime.");
        std::process::exit(1);
    }

    eprintln!("Agent '{}' starting (backend={}, channel={})", args.nick, args.backend, args.channel);

    // Acquire capabilities
    eprintln!("Acquiring capabilities...");

    let did_token = api::acquire_capability(
        &args.api_url,
        &args.session_token,
        "elastos://did/*",
        "execute",
    )?;

    let peer_token = api::acquire_capability(
        &args.api_url,
        &args.session_token,
        "elastos://peer/*",
        "execute",
    )?;

    let ai_resource = if args.respond_all {
        "elastos://ai/*".to_string()
    } else {
        format!("elastos://ai/{}/*", args.backend)
    };
    let ai_token = api::acquire_capability(
        &args.api_url,
        &args.session_token,
        &ai_resource,
        "execute",
    )?;

    // Get agent persona DID (separate identity from the user)
    let (own_did, owner_did) = match api::provider_call(
        &args.api_url,
        &args.session_token,
        &did_token,
        "did",
        "get_persona_did",
        &serde_json::json!({"name": args.nick}),
    ) {
        Ok(resp) => {
            let did = resp
                .get("data")
                .and_then(|d| d.get("did"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let owner = resp
                .get("data")
                .and_then(|d| d.get("owner_did"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (did, owner)
        }
        Err(e) => {
            eprintln!("Warning: could not get persona DID: {}", e);
            (String::new(), String::new())
        }
    };

    eprintln!(
        "DID: {} (owner: {})",
        if own_did.is_empty() { "(none)" } else { &own_did },
        if owner_did.is_empty() { "(none)" } else { &owner_did },
    );

    // Join gossip channel (already_joined is OK — shared runtime)
    match api::provider_call(
        &args.api_url,
        &args.session_token,
        &peer_token,
        "peer",
        "gossip_join",
        &serde_json::json!({"topic": &args.channel}),
    ) {
        Ok(_) => eprintln!("Joined {}", args.channel),
        Err(e) if e.to_string().contains("[already_joined]") => {
            eprintln!("Joined {} (shared runtime)", args.channel);
        }
        Err(e) => {
            eprintln!("Failed to join gossip: {}", e);
            std::process::exit(1);
        }
    }

    // Connect to peer via ticket if provided (deterministic bootstrap)
    if let Some(ref ticket) = args.connect {
        match api::provider_call(
            &args.api_url,
            &args.session_token,
            &peer_token,
            "peer",
            "connect",
            &serde_json::json!({"ticket": ticket}),
        ) {
            Ok(_) => eprintln!("Connected to peer via ticket"),
            Err(e) => eprintln!("Warning: connect failed: {}", e),
        }
    }

    // Announce presence (signed)
    gossip_send(&args, &peer_token, &did_token, &args.channel, &format!("{} joined the chat", args.nick), &own_did);

    // Install Ctrl+C handler (safe signal registration)
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    signal_hook::flag::register(signal_hook::consts::SIGINT, running.clone())
        .expect("Failed to register SIGINT handler");

    let system_prompt = build_system_prompt(&args.nick);
    let mut seen: HashSet<String> = HashSet::new();
    let mut last_response = Instant::now() - COOLDOWN; // allow immediate first response
    let consumer_id = format!("agent:{}:{}", args.nick.to_lowercase(), std::process::id());

    eprintln!("Agent running. Waiting for messages...");

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        // Poll gossip
        match api::provider_call(
            &args.api_url,
            &args.session_token,
            &peer_token,
            "peer",
            "gossip_recv",
            &serde_json::json!({"topic": &args.channel, "limit": 50, "consumer_id": &consumer_id}),
        ) {
            Ok(resp) => {
                if let Some(messages) = resp
                    .get("data")
                    .and_then(|d| d.get("messages"))
                    .and_then(|m| m.as_array())
                {
                    for msg_val in messages {
                        let msg: Message = match serde_json::from_value(msg_val.clone()) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };

                        // Skip own messages
                        if is_own_message(&msg.sender_id, &msg.sender_nick, &own_did, &args.nick) {
                            continue;
                        }

                        // Dedup
                        let key = dedup_key(&msg.sender_id, &msg.sender_nick, msg.ts, &msg.content);
                        if seen.contains(&key) {
                            continue;
                        }
                        if seen.len() >= DEDUP_CAPACITY {
                            seen.clear();
                        }
                        seen.insert(key);

                        // Skip DMs
                        if is_dm(&msg.content) {
                            continue;
                        }

                        // Verify message signature before acting
                        let verified = verify_message(&args, &did_token, &msg);
                        if !verified {
                            // Log unverified messages but don't act on them —
                            // prevents forged @mentions from burning LLM tokens
                            if msg.signature.is_some() {
                                eprintln!("[unverified] <{}> {} (signature invalid)", msg.sender_nick, msg.content);
                            }
                            // Unsigned messages from older clients: still show but don't trigger AI
                            continue;
                        }

                        // Check trigger: @mention or --respond-all
                        let triggered = should_trigger(&msg.content, &args.nick, args.respond_all);

                        if !triggered {
                            continue;
                        }

                        // Cooldown check
                        if last_response.elapsed() < COOLDOWN {
                            continue;
                        }

                        eprintln!("<{}> {}", msg.sender_nick, msg.content);

                        // Call AI (with warmup retry on model loading)
                        let response_text = call_ai_with_warmup(
                            &args,
                            &ai_token,
                            &peer_token,
                            &did_token,
                            &system_prompt,
                            &msg.content,
                            &own_did,
                        );

                        eprintln!("<{}> {}", args.nick, response_text);

                        // Send signed response via gossip
                        gossip_send(&args, &peer_token, &did_token, &args.channel, &response_text, &own_did);
                        last_response = Instant::now();
                    }
                }
            }
            Err(_) => {
                // Silently ignore poll errors
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Graceful shutdown (signed)
    eprintln!("Shutting down...");
    gossip_send(&args, &peer_token, &did_token, &args.channel, &format!("{} left the chat", args.nick), &own_did);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Trigger detection ---

    #[test]
    fn test_trigger_mention() {
        assert!(should_trigger("hey @bot what's up?", "bot", false));
        assert!(should_trigger("@BOT answer me", "bot", false));
        assert!(should_trigger("hello @Bot!", "bot", false));
    }

    #[test]
    fn test_trigger_no_mention() {
        assert!(!should_trigger("hello everyone", "bot", false));
        assert!(!should_trigger("talking about bots", "bot", false));
        assert!(!should_trigger("@alice hi", "bot", false));
    }

    #[test]
    fn test_trigger_respond_all() {
        assert!(should_trigger("hello everyone", "bot", true));
        assert!(should_trigger("no mention at all", "bot", true));
    }

    // --- Self-message skip ---

    #[test]
    fn test_is_own_message() {
        assert!(is_own_message(
            "did:key:z6Mk123",
            "bot",
            "did:key:z6Mk123",
            "bot"
        ));
        assert!(is_own_message(
            "did:key:z6Mk123",
            "BOT",
            "did:key:z6Mk123",
            "bot"
        ));
        assert!(!is_own_message(
            "did:key:z6Mk123",
            "alice",
            "did:key:z6Mk123",
            "bot"
        ));
        assert!(!is_own_message(
            "did:key:z6Mk123",
            "bot",
            "did:key:z6MkOTHER",
            "bot"
        ));
    }

    #[test]
    fn test_is_own_message_empty_did() {
        // If own_did is empty, never skip (can't identify self)
        assert!(!is_own_message("did:key:z6Mk123", "bot", "", "bot"));
        assert!(!is_own_message("", "", "", "bot"));
    }

    // --- DM detection ---

    #[test]
    fn test_is_dm() {
        assert!(is_dm("\x01DM:z6Mk123\x01secret message"));
        assert!(!is_dm("normal message"));
        assert!(!is_dm("@bot hello"));
    }

    // --- Dedup ---

    #[test]
    fn test_dedup_key_deterministic() {
        let k1 = dedup_key("did:key:zAlice", "alice", 1000, "hello");
        let k2 = dedup_key("did:key:zAlice", "alice", 1000, "hello");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_dedup_key_differs() {
        let k1 = dedup_key("did:key:zShared", "alice", 1000, "hello");
        let k2 = dedup_key("did:key:zShared", "alice", 1001, "hello");
        let k3 = dedup_key("did:key:zOther", "alice", 1000, "hello");
        let k4 = dedup_key("did:key:zShared", "alice", 1000, "world");
        let k5 = dedup_key("did:key:zShared", "bot", 1000, "hello");
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
        assert_ne!(k1, k5);
    }

    #[test]
    fn test_dedup_capacity_clears() {
        let mut seen: HashSet<String> = HashSet::new();
        for i in 0..DEDUP_CAPACITY {
            seen.insert(dedup_key("x", "nick", i as u64, "msg"));
        }
        assert_eq!(seen.len(), DEDUP_CAPACITY);
        // Next insert after capacity would trigger clear in main loop
        if seen.len() >= DEDUP_CAPACITY {
            seen.clear();
        }
        assert_eq!(seen.len(), 0);
    }

    // --- System prompt ---

    #[test]
    fn test_system_prompt_contains_nick() {
        let prompt = build_system_prompt("claude");
        assert!(prompt.contains("claude"));
        assert!(prompt.contains("AI assistant"));
    }

    // --- Warmup error detection ---

    #[test]
    fn test_is_warmup_error() {
        assert!(is_warmup_error("[model_loading] Local AI model is still loading."));
        assert!(is_warmup_error("[local_unavailable] Connection refused"));
        assert!(!is_warmup_error("[timeout] Request timed out after 120s"));
        assert!(!is_warmup_error("[api_error] 500 Internal Server Error"));
        assert!(!is_warmup_error("some random error"));
    }

    // --- Message serialization ---

    #[test]
    fn test_message_roundtrip() {
        let msg = Message {
            sender_id: "did:key:z6Mk123".to_string(),
            sender_nick: "alice".to_string(),
            content: "hello @bot".to_string(),
            ts: 1709500000,
            signature: Some("deadbeef".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sender_id, msg.sender_id);
        assert_eq!(parsed.sender_nick, msg.sender_nick);
        assert_eq!(parsed.content, msg.content);
        assert_eq!(parsed.ts, msg.ts);
        assert_eq!(parsed.signature, msg.signature);
    }

    #[test]
    fn test_message_without_signature() {
        // Backward compatibility: messages without signature field should parse
        let json = r#"{"sender_id":"did:key:z6Mk123","sender_nick":"alice","content":"hi","ts":1000}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.signature.is_none());
    }

    // --- Signing payload ---

    #[test]
    fn test_signing_payload_deterministic() {
        let h1 = signing_payload_hex("did:key:zAlice", 1000, "hello");
        let h2 = signing_payload_hex("did:key:zAlice", 1000, "hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_signing_payload_differs() {
        let h1 = signing_payload_hex("did:key:zAlice", 1000, "hello");
        let h2 = signing_payload_hex("did:key:zAlice", 1001, "hello");
        let h3 = signing_payload_hex("did:key:zBob", 1000, "hello");
        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
    }
}
